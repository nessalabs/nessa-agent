//! Keeping a helper service up: make its program ready, start it (or take
//! back the one the last run left running), wait until it says it is ready,
//! publish what it reports, and start it again when it ends. One
//! [`ServiceSupervisor`] per service; the gateway runs as many as it has
//! helpers (today the web tunnel's program).
//!
//! ```text
//! run ──▶ prepare (retried with backoff while it fails)
//!     ──▶ ProcessHost::adopt(key): the last run's process, still the same ──▶ Service::resume(its launch)
//!         else ProcessHost::run(key, launch)
//!     ──▶ Starting ──▶ ready(): Ready(value)
//!                  ──▶ not ready in time: ended (Halt::End), started again
//!         it exits ──▶ Starting, wait, start again
//!     stop asked ──▶ Halt::Leave: the host leaves it running or ends it, before run returns
//! ```
//!
//! Arrows are calls and status changes, in order. The service says what to
//! run, which process counts as the same one (its key) and how to tell it
//! is ready; the host owns the process (spawn, signals, reaping, and whether
//! it outlives this run); this owns only the policy between them. Every
//! wait is on the injected timer. A service that will not come up is
//! `Failed` and tried again; nothing here ends the gateway.
use crate::timer::{within, Timer};
use std::{ffi::OsString, future::Future, path::PathBuf, pin::Pin, sync::Arc, time::Duration};
use tokio::sync::{oneshot, watch};

/// A future the ports return.
pub(crate) type PortFuture<'a, T> = Pin<Box<dyn Future<Output = T> + Send + 'a>>;

/// How often a starting service is asked whether it is ready.
pub(crate) const READY_POLL: Duration = Duration::from_millis(500);

/// Where a service stands.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum ServiceStatus<T> {
    /// Not ready yet (or starting again).
    Starting,
    /// Ready, with what it reported.
    Ready(T),
    /// The last attempt failed for this reason. It is tried again.
    Failed(ServiceFailure),
}

/// Why a service is not up.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum ServiceFailure {
    /// Its program could not be made ready; the service names why.
    Unavailable(&'static str),
    /// The program could not be started.
    Start,
    /// It started but was not ready in time.
    NotReady,
}

impl ServiceFailure {
    /// The wire's word for it.
    pub(crate) fn as_str(self) -> &'static str {
        match self {
            Self::Unavailable(reason) => reason,
            Self::Start => "start",
            Self::NotReady => "not_ready",
        }
    }
}

/// What to run: the program, its arguments and nothing else it inherits.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Launch {
    pub(crate) program: PathBuf,
    pub(crate) args: Vec<OsString>,
    /// The whole environment: nothing of the gateway's own is passed on.
    pub(crate) environment: Vec<(OsString, OsString)>,
    pub(crate) directory: PathBuf,
}

/// One helper service.
pub(crate) trait Service<T>: Send + Sync {
    /// Its name, for the log.
    fn name(&self) -> &'static str;
    /// How long it may take to become ready after it starts.
    fn ready_within(&self) -> Duration;
    /// Make its program ready to start (downloaded and checked, say). Cheap
    /// once it is.
    fn prepare(&self) -> PortFuture<'_, Result<(), ServiceFailure>>;
    /// What a process must have been started for to be taken back by a
    /// later run: everything that would make it a different process (the
    /// program, the setting), and nothing that changes between starts.
    fn key(&self) -> Result<String, ServiceFailure>;
    /// What to run this time. Asked before each start.
    fn launch(&self) -> Result<Launch, ServiceFailure>;
    /// A process started with `launch` under this service's key was taken
    /// back: ask it, from now on, the way that launch says.
    fn resume(&self, launch: &Launch);
    /// What a started process reports once it is ready, `None` before.
    fn ready(&self) -> PortFuture<'_, Option<T>>;
}

/// How a hosted process ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum HostedEnd {
    /// It could not be started.
    NotStarted,
    /// It exited on its own.
    Exited,
    /// It was stopped, as asked.
    Stopped,
    /// It was left running, as asked, for a later run to take back.
    Left,
}

/// What a host is asked to do with a process it runs.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Halt {
    /// End it: it is being replaced.
    End,
    /// This run is ending. A host that keeps processes across runs leaves
    /// it running; any other ends it.
    Leave,
}

/// A process taken back from an earlier run: how it was started, and its
/// run from here (as [`ProcessHost::run`] returns it).
pub(crate) type Adopted = (Launch, PortFuture<'static, HostedEnd>);

/// Owns the processes: starts one, and ends it or leaves it when asked.
pub(crate) trait ProcessHost: Send + Sync {
    /// Run `launch`, started for `key`, until it exits or `halt` resolves,
    /// then end it (and anything it started) or leave it, as `halt` says.
    /// Ended means confirmed gone before this returns. `started` is told
    /// once it is running.
    fn run(
        &self,
        key: String,
        launch: Launch,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> PortFuture<'static, HostedEnd>;

    /// The process an earlier run left running for `key`, when it is still
    /// that same process; run from here like [`Self::run`]. A host that
    /// keeps nothing across runs has none.
    fn adopt(
        &self,
        key: String,
        started: oneshot::Sender<()>,
        halt: PortFuture<'static, Halt>,
    ) -> PortFuture<'static, Option<Adopted>> {
        let _ = (key, started, halt);
        Box::pin(async { None })
    }
}

/// How long to wait between attempts.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Backoff {
    /// After the first attempt that fails.
    pub(crate) first: Duration,
    /// The longest wait; each further failure doubles the wait up to it.
    pub(crate) most: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            most: Duration::from_secs(60),
        }
    }
}

/// What keeping one service up is given.
pub(crate) struct ServiceSupervisor<T> {
    pub(crate) service: Arc<dyn Service<T>>,
    pub(crate) host: Arc<dyn ProcessHost>,
    pub(crate) timer: Arc<dyn Timer>,
    /// Where its status is published.
    pub(crate) status: watch::Sender<ServiceStatus<T>>,
    pub(crate) backoff: Backoff,
}

/// One hosted process, and how to halt it.
struct Hosted {
    run: PortFuture<'static, HostedEnd>,
    halt: oneshot::Sender<Halt>,
}

impl Hosted {
    /// Ask it to halt, and wait for it to.
    async fn halt(self, halt: Halt) {
        let _ = self.halt.send(halt);
        self.run.await;
    }
}

/// The halt future a host is given: what the supervisor asks for, or
/// `End` when the supervisor is gone without asking.
fn halt_channel() -> (oneshot::Sender<Halt>, PortFuture<'static, Halt>) {
    let (halt, halted) = oneshot::channel();
    (halt, Box::pin(async move { halted.await.unwrap_or(Halt::End) }))
}

/// How waiting for a started process to be ready ended.
enum Readiness<T> {
    Stop,
    Ended(HostedEnd),
    Late,
    Ready(T),
}

impl<T: Clone + Send + Sync + 'static> ServiceSupervisor<T> {
    /// Keep the service up until `stop` is `true` (or its sender is gone).
    /// The wait after a failure doubles from `backoff.first` to
    /// `backoff.most`, and starts over after a run that stayed ready at
    /// least `backoff.most`. A process running when stop is asked is stopped
    /// and gone before this returns.
    pub(crate) async fn run(self, mut stop: watch::Receiver<bool>) {
        let name = self.service.name();
        self.status.send_replace(ServiceStatus::Starting);
        let mut wait = self.backoff.first;
        loop {
            let prepared = tokio::select! {
                biased;
                () = stopped(&mut stop) => return,
                prepared = self.service.prepare() => prepared,
            };
            match prepared {
                Ok(()) => break,
                Err(failure) => {
                    tracing::warn!(service = name, failure = failure.as_str(), ?wait, "service not ready to start");
                    self.status.send_replace(ServiceStatus::Failed(failure));
                }
            }
            if !self.pause(&mut stop, &mut wait).await {
                return;
            }
        }
        // Only the first start may take back a process left by the last run:
        // any later one replaces a process this run ended or saw exit.
        let mut adopting = true;
        loop {
            let (mut hosted, running) = match self.start(std::mem::take(&mut adopting)).await {
                Ok(started) => started,
                Err(failure) => {
                    self.status.send_replace(ServiceStatus::Failed(failure));
                    if !self.pause(&mut stop, &mut wait).await {
                        return;
                    }
                    continue;
                }
            };
            let readiness = tokio::select! {
                biased;
                () = stopped(&mut stop) => Readiness::Stop,
                end = &mut hosted.run => Readiness::Ended(end),
                ready = self.ready_after(running) => ready.map_or(Readiness::Late, Readiness::Ready),
            };
            let ready = match readiness {
                Readiness::Stop => {
                    hosted.halt(Halt::Leave).await;
                    return;
                }
                Readiness::Ended(HostedEnd::NotStarted) => {
                    tracing::warn!(service = name, ?wait, "service did not start");
                    self.status.send_replace(ServiceStatus::Failed(ServiceFailure::Start));
                    None
                }
                Readiness::Ended(_) => None,
                Readiness::Late => {
                    tracing::warn!(service = name, "service was not ready in time");
                    self.status.send_replace(ServiceStatus::Failed(ServiceFailure::NotReady));
                    hosted.halt(Halt::End).await;
                    None
                }
                Readiness::Ready(ready) => Some((ready, hosted)),
            };
            if let Some((ready, mut hosted)) = ready {
                tracing::info!(service = name, "service ready");
                let since = self.timer.now();
                self.status.send_replace(ServiceStatus::Ready(ready));
                let asked = tokio::select! {
                    biased;
                    () = stopped(&mut stop) => true,
                    _ = &mut hosted.run => false,
                };
                if asked {
                    hosted.halt(Halt::Leave).await;
                    return;
                }
                if self.timer.now().since(since) >= self.backoff.most {
                    wait = self.backoff.first;
                }
                tracing::warn!(service = name, ?wait, "service ended; starting it again");
                self.status.send_replace(ServiceStatus::Starting);
            }
            if !self.pause(&mut stop, &mut wait).await {
                return;
            }
        }
    }

    /// Take back the process the last run left, when `adopting` and there is
    /// one still the same; else start a new one.
    /// Also returns the receiver told once it is running.
    async fn start(
        &self,
        adopting: bool,
    ) -> Result<(Hosted, oneshot::Receiver<()>), ServiceFailure> {
        let key = self.service.key()?;
        if adopting {
            let (started, running) = oneshot::channel();
            let (halt, halted) = halt_channel();
            if let Some((launch, run)) = self.host.adopt(key.clone(), started, halted).await {
                tracing::info!(service = self.service.name(), "took back the running service");
                self.service.resume(&launch);
                return Ok((Hosted { run, halt }, running));
            }
        }
        let launch = self.service.launch()?;
        let (started, running) = oneshot::channel();
        let (halt, halted) = halt_channel();
        let run = self.host.run(key, launch, started, halted);
        Ok((Hosted { run, halt }, running))
    }

    /// Once the process is running, ask whether it is ready until it is, or
    /// until its time to become ready is up (`None`).
    async fn ready_after(&self, running: oneshot::Receiver<()>) -> Option<T> {
        // A host that never starts it ends `hosted` instead.
        if running.await.is_err() {
            return std::future::pending().await;
        }
        let deadline = self.timer.now() + self.service.ready_within();
        within(&*self.timer, deadline, async {
            loop {
                if let Some(ready) = self.service.ready().await {
                    return ready;
                }
                self.timer.sleep(READY_POLL).await;
            }
        })
        .await
        .ok()
    }

    /// Wait `wait`, then double it; `false` when stop is asked first.
    async fn pause(&self, stop: &mut watch::Receiver<bool>, wait: &mut Duration) -> bool {
        let waited = tokio::select! {
            biased;
            () = stopped(stop) => false,
            () = self.timer.sleep(*wait) => true,
        };
        *wait = (*wait * 2).min(self.backoff.most);
        waited
    }
}

/// Resolves once stop is asked. A dropped sender is a stop too: nothing can
/// ask for one later.
async fn stopped(stop: &mut watch::Receiver<bool>) {
    let _ = stop.wait_for(|stopped| *stopped).await;
}

#[cfg(test)]
#[path = "../../tests/services/supervisor.rs"]
mod tests;
