//! The supervisor's policy over a scripted service and host, on a timer the
//! test moves: what it starts or takes back, when it is ready, what it asks
//! the host to do with the process, and how long it waits between attempts.
use super::*;
use crate::timer::manual::ManualTimer;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Mutex,
};
use tokio::sync::mpsc;

/// A service whose readiness and preparation the test scripts.
#[derive(Default)]
struct Scripted {
    /// Preparations that fail before one succeeds.
    failing_prepares: AtomicUsize,
    /// What `ready` answers once asked this many times.
    ready_after: Mutex<Option<usize>>,
    asked: AtomicUsize,
    launches: AtomicUsize,
    resumed: Mutex<Vec<Launch>>,
}

impl Service<&'static str> for Scripted {
    fn name(&self) -> &'static str {
        "scripted"
    }
    fn ready_within(&self) -> Duration {
        Duration::from_secs(10)
    }
    #[allow(deprecated, reason = "Rust 1.89 MSRV; try_update requires Rust 1.95")]
    fn prepare(&self) -> PortFuture<'_, Result<(), ServiceFailure>> {
        let failing = self
            .failing_prepares
            .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |left| {
                left.checked_sub(1)
            })
            .is_ok();
        Box::pin(async move {
            if failing {
                Err(ServiceFailure::Unavailable("download"))
            } else {
                Ok(())
            }
        })
    }
    fn key(&self) -> Result<String, ServiceFailure> {
        Ok("key".into())
    }
    fn launch(&self) -> Result<Launch, ServiceFailure> {
        let index = self.launches.fetch_add(1, Ordering::SeqCst);
        Ok(launch(&format!("launch-{index}")))
    }
    fn resume(&self, launch: &Launch) {
        self.resumed.lock().unwrap().push(launch.clone());
    }
    fn ready(&self) -> PortFuture<'_, Option<&'static str>> {
        let asked = self.asked.fetch_add(1, Ordering::SeqCst) + 1;
        let after = *self.ready_after.lock().unwrap();
        Box::pin(async move { after.filter(|after| asked >= *after).map(|_| "address") })
    }
}

fn launch(program: &str) -> Launch {
    Launch {
        program: program.into(),
        args: vec![],
        environment: vec![],
        directory: "/".into(),
    }
}

/// What the host was asked.
#[derive(Debug, PartialEq, Eq)]
enum Asked {
    Run(String),
    Adopt,
    Halt(Halt),
}

/// A host whose processes run until the test ends them or the supervisor
/// halts them. `adoptable` is a process an earlier run left.
struct Host {
    asked: mpsc::UnboundedSender<Asked>,
    adoptable: Mutex<Option<Launch>>,
    /// Each running process's exit, sent by the test.
    exits: Mutex<Vec<oneshot::Sender<()>>>,
}

impl Host {
    fn new(adoptable: Option<Launch>) -> (Arc<Self>, mpsc::UnboundedReceiver<Asked>) {
        let (asked, receiver) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                asked,
                adoptable: Mutex::new(adoptable),
                exits: Mutex::default(),
            }),
            receiver,
        )
    }

    /// The most recent process exits on its own.
    fn exit(&self) {
        let exit = self.exits.lock().unwrap().pop().expect("a process running");
        let _ = exit.send(());
    }

    fn hosted(
        &self,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> PortFuture<'static, HostedEnd> {
        let (exit, exited) = oneshot::channel();
        self.exits.lock().unwrap().push(exit);
        let asked = self.asked.clone();
        let _ = started.send(());
        Box::pin(async move {
            tokio::select! {
                _ = exited => HostedEnd::Exited,
                halt = halt => {
                    let _ = asked.send(Asked::Halt(halt));
                    match halt {
                        Halt::End => HostedEnd::Stopped,
                        Halt::Leave => HostedEnd::Left,
                    }
                }
            }
        })
    }
}

impl ProcessHost for Host {
    fn run(
        &self,
        _key: String,
        launch: Launch,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> PortFuture<'static, HostedEnd> {
        let _ = self
            .asked
            .send(Asked::Run(launch.program.display().to_string()));
        self.hosted(started, halt)
    }

    fn adopt(
        &self,
        key: String,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> PortFuture<'static, Option<Adopted>> {
        assert_eq!(key, "key");
        let _ = self.asked.send(Asked::Adopt);
        let adopted = self
            .adoptable
            .lock()
            .unwrap()
            .take()
            .map(|launch| (launch, self.hosted(started, halt)));
        Box::pin(async move { adopted })
    }
}

struct Running {
    service: Arc<Scripted>,
    host: Arc<Host>,
    asked: mpsc::UnboundedReceiver<Asked>,
    timer: Arc<ManualTimer>,
    status: watch::Receiver<ServiceStatus<&'static str>>,
    stop: watch::Sender<bool>,
    task: tokio::task::JoinHandle<()>,
}

fn start(service: Scripted, adoptable: Option<Launch>) -> Running {
    let service = Arc::new(service);
    let (host, asked) = Host::new(adoptable);
    let timer = Arc::new(ManualTimer::default());
    let (status, watching) = watch::channel(ServiceStatus::Starting);
    let (stop, stopped) = watch::channel(false);
    let supervisor = ServiceSupervisor {
        service: service.clone(),
        host: host.clone(),
        timer: timer.clone(),
        status,
        backoff: Backoff::default(),
    };
    Running {
        service,
        host,
        asked,
        timer,
        status: watching,
        stop,
        task: tokio::spawn(supervisor.run(stopped)),
    }
}

impl Running {
    async fn next(&mut self) -> Asked {
        tokio::time::timeout(Duration::from_secs(5), self.asked.recv())
            .await
            .expect("the host was asked")
            .expect("the supervisor is running")
    }

    async fn status_is(&mut self, wanted: ServiceStatus<&'static str>) {
        let seen = tokio::time::timeout(
            Duration::from_secs(5),
            self.status.wait_for(|status| *status == wanted),
        )
        .await
        .is_ok();
        assert!(
            seen,
            "status never {wanted:?}; it is {:?}",
            *self.status.borrow()
        );
    }

    /// Stop it, and see the run end.
    async fn stop(mut self) -> Self {
        self.stop.send_replace(true);
        (&mut self.task).await.unwrap();
        self
    }
}

#[tokio::test]
async fn a_ready_service_is_published_and_left_running_when_stopped() {
    let mut running = start(
        Scripted {
            ready_after: Mutex::new(Some(1)),
            ..Scripted::default()
        },
        None,
    );
    assert_eq!(running.next().await, Asked::Adopt);
    assert_eq!(running.next().await, Asked::Run("launch-0".into()));
    running.status_is(ServiceStatus::Ready("address")).await;
    let mut running = running.stop().await;
    assert_eq!(running.next().await, Asked::Halt(Halt::Leave));
}

#[tokio::test]
async fn a_service_not_ready_in_time_is_ended_and_started_again() {
    let mut running = start(Scripted::default(), None);
    assert_eq!(running.next().await, Asked::Adopt);
    assert_eq!(running.next().await, Asked::Run("launch-0".into()));
    // The readiness deadline is the first wait; the polls sleep after it.
    let deadline = running
        .timer
        .started(|_, wait| wait.limit() == Duration::from_secs(10))
        .await;
    running.timer.advance_to(deadline.deadline);
    assert_eq!(running.next().await, Asked::Halt(Halt::End));
    running
        .status_is(ServiceStatus::Failed(ServiceFailure::NotReady))
        .await;
    // Then the backoff's first wait, and a fresh start: never a take-back.
    let backoff = running
        .timer
        .started(|_, wait| wait.limit() == Duration::from_secs(1))
        .await;
    running.timer.advance_to(backoff.deadline);
    assert_eq!(running.next().await, Asked::Run("launch-1".into()));
    running.stop().await;
}

#[tokio::test]
async fn an_exit_after_ready_starts_it_again_after_the_backoff() {
    let mut running = start(
        Scripted {
            ready_after: Mutex::new(Some(1)),
            ..Scripted::default()
        },
        None,
    );
    assert_eq!(running.next().await, Asked::Adopt);
    assert_eq!(running.next().await, Asked::Run("launch-0".into()));
    running.status_is(ServiceStatus::Ready("address")).await;
    running.host.exit();
    running.status_is(ServiceStatus::Starting).await;
    let backoff = running
        .timer
        .started(|_, wait| wait.limit() == Duration::from_secs(1))
        .await;
    running.timer.advance_to(backoff.deadline);
    assert_eq!(running.next().await, Asked::Run("launch-1".into()));
    running.stop().await;
}

#[tokio::test]
async fn the_first_start_takes_back_a_left_process_and_later_ones_never_do() {
    let mut running = start(
        Scripted {
            ready_after: Mutex::new(Some(1)),
            ..Scripted::default()
        },
        Some(launch("left")),
    );
    assert_eq!(running.next().await, Asked::Adopt);
    running.status_is(ServiceStatus::Ready("address")).await;
    assert_eq!(*running.service.resumed.lock().unwrap(), [launch("left")]);
    assert_eq!(running.service.launches.load(Ordering::SeqCst), 0);
    running.host.exit();
    running.status_is(ServiceStatus::Starting).await;
    let backoff = running
        .timer
        .started(|_, wait| wait.limit() == Duration::from_secs(1))
        .await;
    running.timer.advance_to(backoff.deadline);
    assert_eq!(running.next().await, Asked::Run("launch-0".into()));
    running.stop().await;
}

#[tokio::test]
async fn a_program_that_cannot_be_prepared_is_failed_and_tried_again() {
    let mut running = start(
        Scripted {
            failing_prepares: AtomicUsize::new(2),
            ready_after: Mutex::new(Some(1)),
            ..Scripted::default()
        },
        None,
    );
    running
        .status_is(ServiceStatus::Failed(ServiceFailure::Unavailable(
            "download",
        )))
        .await;
    // One second, then two: the wait doubles.
    let first = running.timer.started(|index, _| index == 0).await;
    assert_eq!(first.limit(), Duration::from_secs(1));
    running.timer.advance_to(first.deadline);
    let second = running.timer.started(|index, _| index == 1).await;
    assert_eq!(second.limit(), Duration::from_secs(2));
    running.timer.advance_to(second.deadline);
    assert_eq!(running.next().await, Asked::Adopt);
    assert_eq!(running.next().await, Asked::Run("launch-0".into()));
    running.status_is(ServiceStatus::Ready("address")).await;
    running.stop().await;
}

#[tokio::test]
async fn a_stop_while_preparing_starts_nothing() {
    let running = start(
        Scripted {
            failing_prepares: AtomicUsize::new(usize::MAX),
            ..Scripted::default()
        },
        None,
    );
    let mut running = running.stop().await;
    assert!(running.asked.try_recv().is_err(), "nothing was started");
}
