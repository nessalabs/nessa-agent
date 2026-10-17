//! Test-only owners for the one-thread responsiveness probe. Cancellation drops
//! only the operation's borrow; initialization, sockets and join handles stay
//! with `Owners` until physical drain, before the runtime is retired.
//!
//! ```text
//! timed operation --borrows--> Owners --owns--> fixture initialization
//!                              Owners --owns--> fixture / stop / listener
//!                              Owners --owns--> connections / device thread
//! completed or cancelled operation --> original owners' drain --> outcome
//! ```
use super::support::{pending, Fixture, WAIT};
use futures_util::FutureExt;
use nessa_auth::{
    adapters::pairing::{
        FilePairingState, GatewayTrust, NativeIdentity, NativeTransport, OsEntropy,
    },
    domain::pairing::ConsentClass,
};
use nessa_protocol::pairing::{EnrollmentChannel, NativeFrameError};
use nessa_server::device_pairing::infrastructure::{
    NativeEnrollmentConnections, PairingRuntimeError,
};
use std::{
    any::Any,
    future::Future,
    io::Result as IoResult,
    net::TcpStream,
    panic::AssertUnwindSafe,
    path::Path,
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    thread::{JoinHandle as ThreadJoinHandle, Result as ThreadResult},
    time::{Duration, Instant},
};
use tokio::{
    runtime::{Builder, Runtime},
    sync::oneshot,
    task::JoinHandle,
    time::Instant as TokioInstant,
};

type Initialization = Pin<Box<dyn Future<Output = Fixture>>>;
type Work<'a> = Pin<Box<dyn Future<Output = ()> + 'a>>;
type Panic = Box<dyn Any + Send>;

pub struct Phase {
    name: &'static str,
    started: Instant,
}
impl Phase {
    pub fn enter(name: &'static str) -> Self {
        eprintln!("one-thread-phase enter={name}");
        Self {
            name,
            started: Instant::now(),
        }
    }
}
impl Drop for Phase {
    fn drop(&mut self) {
        eprintln!(
            "one-thread-phase leave={} elapsed={:?}",
            self.name,
            self.started.elapsed()
        );
    }
}

pub struct Owners {
    initializing: Option<Initialization>,
    pub fixture: Option<Fixture>,
    pub stop: Option<oneshot::Sender<()>>,
    pub listener: Option<JoinHandle<IoResult<()>>>,
    pub connections: Option<Arc<NativeEnrollmentConnections>>,
    pub device: Option<ThreadJoinHandle<()>>,
}
impl Owners {
    fn new(initializing: Initialization) -> Self {
        Self {
            initializing: Some(initializing),
            fixture: None,
            stop: None,
            listener: None,
            connections: None,
            device: None,
        }
    }
    pub async fn initialize(&mut self) {
        if self.fixture.is_none() {
            self.fixture = Some(self.initializing.as_mut().unwrap().await);
            self.initializing.take();
        }
    }
    async fn cleanup(&mut self, panicked: bool) -> Cleanup {
        let _phase = Phase::enter("retained-cleanup");
        if self.fixture.is_none() {
            if panicked {
                // A constructor panic has unwound its original future; polling
                // it again is invalid. Startup's physical worker returns before
                // Fixture unwraps its result. No listener/device exists yet.
                self.initializing.take();
            } else {
                // A timeout is cancellation of the observer, not initialization.
                // Finish the same future, retaining its private directory and
                // startup worker until their real acknowledgement arrives.
                self.initialize().await;
            }
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        let listener = match self.listener.as_mut() {
            Some(listener) => Some(listener.await),
            None => None,
        };
        self.listener.take();
        if let Some(connections) = &self.connections {
            connections.shutdown().await;
        }
        if let Some(fixture) = &self.fixture {
            fixture.gateway.shutdown().await;
        }
        // The server has physically drained, closing the device socket. Keep
        // its actual OS-thread handle through join, even if it panicked. This
        // does not queue another job onto the one-thread blocking pool.
        let device = self.device.take().map(|device| device.join());
        let cleanup = Cleanup { listener, device };
        eprintln!(
            "one-thread-phase cleanup listener_failed={} device_failed={}",
            matches!(cleanup.listener, Some(Err(_)) | Some(Ok(Err(_)))),
            matches!(cleanup.device, Some(Err(_)))
        );
        cleanup
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Completion {
    Finished,
    TimedOut,
}
pub struct Cleanup {
    listener: Option<Result<IoResult<()>, tokio::task::JoinError>>,
    device: Option<ThreadResult<()>>,
}
impl Cleanup {
    pub fn assert_success(self) {
        if let Some(listener) = self.listener {
            listener.unwrap().unwrap();
        }
        if let Some(device) = self.device {
            device.unwrap();
        }
    }
}
pub struct Attempt {
    pub completion: Result<Completion, Panic>,
    pub cleanup: Cleanup,
    // Retain the private fixture through outcome inspection as well as drain.
    // Tests reopen its actual private store after device join.
    pub owners: Owners,
}

pub fn runtime() -> Runtime {
    Builder::new_multi_thread()
        .worker_threads(2)
        .max_blocking_threads(1)
        .enable_all()
        .build()
        .unwrap()
}

pub async fn run(
    budget: Duration,
    work: impl for<'a> FnOnce(&'a mut Owners) -> Work<'a>,
) -> Attempt {
    run_initializing(Box::pin(Fixture::new()), TokioInstant::now() + budget, work).await
}
async fn run_initializing(
    initializing: Initialization,
    deadline: TokioInstant,
    work: impl for<'a> FnOnce(&'a mut Owners) -> Work<'a>,
) -> Attempt {
    run_owned(Owners::new(initializing), deadline, work).await
}
async fn run_owned(
    mut owners: Owners,
    deadline: TokioInstant,
    work: impl for<'a> FnOnce(&'a mut Owners) -> Work<'a>,
) -> Attempt {
    // Keep Tokio's original timeout implementation, including its work-first
    // polling and cooperative scheduling. Only ownership moves out of it.
    let completion =
        AssertUnwindSafe(async { tokio::time::timeout_at(deadline, work(&mut owners)).await })
            .catch_unwind()
            .await
            .map(|result| match result {
                Ok(()) => Completion::Finished,
                Err(_) => Completion::TimedOut,
            });
    let outcome = match &completion {
        Ok(Completion::Finished) => "finished",
        Ok(Completion::TimedOut) => "timed-out",
        Err(_) => "panicked",
    };
    eprintln!("one-thread-phase outcome={outcome}");
    let cleanup = owners.cleanup(completion.is_err()).await;
    Attempt {
        completion,
        cleanup,
        owners,
    }
}

async fn blocked_peer(
    owners: &mut Owners,
    ready: oneshot::Sender<()>,
    panic_device: bool,
) -> oneshot::Receiver<()> {
    owners.initialize().await;
    let fixture = owners.fixture.as_ref().unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    owners.stop = Some(stop);
    owners.listener = Some(listener);
    owners.connections = Some(connections);
    let (_, store) = pending(fixture.directory.path(), "cleanup-device");
    let pin = fixture.gateway.identity().public_spki();
    let (connected, connection) = oneshot::channel();
    let (reply, replied) = oneshot::channel();
    owners.device = Some(std::thread::spawn(move || {
        // Hold actual private-state ownership until this thread is joined.
        let _store = store;
        let _reply = reply;
        let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let stream = TcpStream::connect(address).unwrap();
        stream.set_read_timeout(Some(WAIT)).unwrap();
        let transport =
            NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut channel = EnrollmentChannel::new(transport);
        connected.send(()).unwrap();
        if panic_device {
            panic!("controlled device failure before reply");
        }
        assert!(matches!(
            channel.receive_envelope(),
            Err(NativeFrameError::Io(_))
        ));
    }));
    // Completed TLS is the admission evidence, not a sleep or socket connect.
    connection.await.unwrap();
    ready.send(()).unwrap();
    replied
}

async fn assert_gateway_closed(owners: &Owners) {
    let fixture = owners.fixture.as_ref().unwrap();
    assert!(matches!(
        fixture
            .gateway
            .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
            .await,
        Err(PairingRuntimeError::ShuttingDown)
    ));
}

#[test]
fn timeout_stops_listener_and_joins_device() {
    runtime().block_on(async {
        let (ready, _ready) = oneshot::channel();
        let mut owners = Owners::new(Box::pin(Fixture::new()));
        let _reply = blocked_peer(&mut owners, ready, false).await;
        let attempt = run_owned(
            owners,
            TokioInstant::now() - Duration::from_secs(1),
            |_owners| Box::pin(std::future::pending()),
        )
        .await;
        assert_eq!(attempt.completion.unwrap(), Completion::TimedOut);
        assert!(matches!(attempt.cleanup.listener, Some(Ok(Ok(())))));
        assert!(matches!(attempt.cleanup.device, Some(Ok(()))));
        let connections = attempt.owners.connections.as_ref().unwrap();
        assert_eq!(connections.wake_report().unwrap().iter().count(), 1);
        assert_gateway_closed(&attempt.owners).await;
        let root = attempt
            .owners
            .fixture
            .as_ref()
            .unwrap()
            .directory
            .path()
            .join("cleanup-device");
        let reopened = FilePairingState::open(&root, Path::new("state")).unwrap();
        drop(reopened);
    });
}

#[test]
fn panic_stops_listener_and_joins_device() {
    runtime().block_on(async {
        let (ready, _ready) = oneshot::channel();
        let attempt = run(WAIT, |owners| {
            Box::pin(async move {
                let _reply = blocked_peer(owners, ready, false).await;
                panic!("controlled caller assertion failure");
            })
        })
        .await;
        assert!(attempt.completion.is_err());
        assert!(matches!(attempt.cleanup.listener, Some(Ok(Ok(())))));
        assert!(matches!(attempt.cleanup.device, Some(Ok(()))));
        assert_gateway_closed(&attempt.owners).await;
    });
}

#[test]
fn device_panic_still_drains_original_gateway() {
    runtime().block_on(async {
        let (ready, _ready) = oneshot::channel();
        let attempt = run(WAIT, |owners| {
            Box::pin(async move {
                let reply = blocked_peer(owners, ready, true).await;
                // Match the original caller's failure when the device panics
                // before sending its reply. The cleanup keeps both facts.
                reply.await.expect("device reply");
            })
        })
        .await;
        assert!(attempt.completion.is_err());
        assert!(matches!(attempt.cleanup.listener, Some(Ok(Ok(())))));
        assert!(matches!(attempt.cleanup.device, Some(Err(_))));
        assert_gateway_closed(&attempt.owners).await;
    });
}

#[test]
fn timeout_before_fixture_completion_retains_initialization() {
    runtime().block_on(async {
        let starts = Arc::new(AtomicUsize::new(0));
        let observed = starts.clone();
        let polls = Arc::new(AtomicUsize::new(0));
        let polled = polls.clone();
        let (release, released) = oneshot::channel();
        let mut initializing = Box::pin(async move {
            observed.fetch_add(1, Ordering::SeqCst);
            released.await.unwrap();
            Fixture::new().await
        });
        let initializing = Box::pin(std::future::poll_fn(move |cx| {
            polled.fetch_add(1, Ordering::SeqCst);
            initializing.as_mut().poll(cx)
        }));
        let attempt = run_initializing(
            initializing,
            TokioInstant::now() - Duration::from_secs(1),
            |owners| {
                Box::pin(async move {
                    owners.initialize().await;
                    std::future::pending::<()>().await;
                })
            },
        );
        tokio::pin!(attempt);
        assert!(futures_util::poll!(attempt.as_mut()).is_pending());
        assert_eq!(starts.load(Ordering::SeqCst), 1);
        // One poll by the operation, then another by retained cleanup after
        // the genuinely expired Tokio timer: the same initialization survives.
        assert!(polls.load(Ordering::SeqCst) >= 2);
        release.send(()).unwrap();
        let attempt = attempt.await;
        assert_eq!(attempt.completion.unwrap(), Completion::TimedOut);
        assert_gateway_closed(&attempt.owners).await;
        assert_eq!(starts.load(Ordering::SeqCst), 1);
    });
}

#[test]
fn finished_work_wins_a_ready_deadline_and_still_drains() {
    runtime().block_on(async {
        // Both futures are ready before select polls either one. Constructing
        // the real fixture here makes initialization ready in that first poll.
        let fixture = Fixture::new().await;
        let attempt = run_initializing(
            Box::pin(std::future::ready(fixture)),
            TokioInstant::now() - Duration::from_secs(1),
            |owners| {
                Box::pin(async move {
                    owners.initialize().await;
                })
            },
        )
        .await;
        assert_eq!(attempt.completion.unwrap(), Completion::Finished);
        attempt.cleanup.assert_success();
        assert_gateway_closed(&attempt.owners).await;
    });
}

#[test]
fn initialization_panic_is_not_polled_again_or_reported_as_a_fixture() {
    runtime().block_on(async {
        let attempt = run_initializing(
            Box::pin(async { panic!("controlled fixture constructor panic") }),
            TokioInstant::now() + WAIT,
            |owners| {
                Box::pin(async move {
                    owners.initialize().await;
                })
            },
        )
        .await;
        assert!(attempt.completion.is_err());
        assert!(attempt.owners.fixture.is_none());
        assert!(attempt.owners.initializing.is_none());
        assert!(attempt.cleanup.listener.is_none());
        assert!(attempt.cleanup.device.is_none());
    });
}

#[test]
fn finished_work_does_not_hide_device_join_failure() {
    runtime().block_on(async {
        let attempt = run(WAIT, |owners| {
            Box::pin(async move {
                owners.initialize().await;
                owners.device = Some(std::thread::spawn(|| {
                    panic!("device failed after replying")
                }));
            })
        })
        .await;
        assert_eq!(attempt.completion.unwrap(), Completion::Finished);
        assert_gateway_closed(&attempt.owners).await;
        assert!(matches!(attempt.cleanup.device, Some(Err(_))));
        assert!(
            std::panic::catch_unwind(AssertUnwindSafe(|| attempt.cleanup.assert_success()))
                .is_err()
        );
    });
}
