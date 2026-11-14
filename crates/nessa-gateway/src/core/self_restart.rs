//! A gateway that starts itself again, to apply configuration it just saved
//! (`pairing.setLinking`).
//!
//! The run stops the way a signal stops it — the same drain, the same report —
//! and then this process replaces itself with the same executable, arguments
//! and environment (`execve`). The process id stays, so whatever supervises
//! it (launchd, systemd, the developer loop, the stress harness) sees one
//! process that never exited. By then the drain has closed the stores and
//! listeners, and what is left does not cross into the new image: Rust's
//! standard library and tokio open every descriptor close-on-exec, so no
//! socket or lock is inherited.
//!
//! ```text
//! pairing.setLinking ──schedule──▶ SelfRestart ◀──stop── SIGTERM / Ctrl-C (any time, drain included)
//!          (after the answer) ──request──▶ │ ──requested──▶ stop signal ──▶ drain
//!                                          └─ restarts ──▶ exec_again, unless a stop came first
//! ```
//!
//! The order is one state, owned here: a stop from outside wins over a
//! restart at every point, so a gateway asked to stop never comes back by
//! itself. An exec that fails ends the run as a serve failure: non-zero, so
//! a managed gateway is started again by launchd and a standalone one says
//! why it ended.
use std::{
    future::Future,
    sync::{Arc, Mutex},
};
use tokio::sync::watch;

/// Where the run is, as far as starting again goes.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Phase {
    Serving,
    /// A change was saved; the restart is asked for once its answer had
    /// time to reach the caller.
    Scheduled,
    /// The run is stopping to start again.
    Restarting,
    /// Something outside asked the run to stop: it ends and stays ended.
    Stopping,
}

/// How a held restart ended, told to whoever scheduled it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RestartEnd {
    /// The drain finished; the process is replacing itself now.
    Exec,
    /// The exec failed: the run ends as a serve failure.
    ExecFailed,
    /// The run ended without starting again: a stop won, or the drain failed.
    Cancelled,
}

/// Told how the held restart ended.
pub type RestartReport = Box<dyn Fn(RestartEnd) + Send + Sync>;

/// The run's one answer to "stop, and start again?".
pub struct SelfRestart {
    phase: watch::Sender<Phase>,
    /// The held restart's report, from [`Self::schedule`].
    report: Mutex<Option<RestartReport>>,
}

impl Default for SelfRestart {
    fn default() -> Self {
        Self {
            phase: watch::channel(Phase::Serving).0,
            report: Mutex::new(None),
        }
    }
}

impl SelfRestart {
    /// Hold a restart for a change about to be answered, and tell `report`
    /// how it ends. `false` once one is held, or the run is stopping: the
    /// change waits for that to end, and `report` is dropped.
    pub fn schedule(&self, report: RestartReport) -> bool {
        let held = self.phase.send_if_modified(|phase| {
            let serving = *phase == Phase::Serving;
            if serving {
                *phase = Phase::Scheduled;
            }
            serving
        });
        if held {
            *self
                .report
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()) = Some(report);
        }
        held
    }

    /// Tell the held restart's report how it ended; nothing when none is held.
    pub fn finish(&self, end: RestartEnd) {
        if let Some(report) = self
            .report
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .as_ref()
        {
            report(end);
        }
    }

    /// Whether a change may be made now: nothing scheduled, not stopping.
    pub fn serving(&self) -> bool {
        *self.phase.borrow() == Phase::Serving
    }

    /// Stop the run to start again, if the restart is still held.
    pub fn request(&self) {
        self.phase.send_if_modified(|phase| {
            let held = *phase == Phase::Scheduled;
            if held {
                *phase = Phase::Restarting;
            }
            held
        });
    }

    /// The run was asked to stop from outside: whatever was held or asked,
    /// it does not start again.
    pub fn stop(&self) {
        self.phase.send_replace(Phase::Stopping);
    }

    /// The run is stopping for a reason of its own (the native listener
    /// ended): a restart held but not yet asked for does not happen. One
    /// already under way keeps going.
    pub fn stop_unless_restarting(&self) {
        self.phase.send_if_modified(|phase| {
            let ends = *phase != Phase::Restarting;
            if ends {
                *phase = Phase::Stopping;
            }
            ends
        });
    }

    /// Whether the run ends by starting again.
    pub fn restarts(&self) -> bool {
        *self.phase.borrow() == Phase::Restarting
    }

    /// Resolves once a restart is asked for; never, otherwise.
    pub fn requested(&self) -> impl std::future::Future<Output = ()> + Send + 'static {
        let mut phase = self.phase.subscribe();
        async move {
            if phase
                .wait_for(|phase| *phase == Phase::Restarting)
                .await
                .is_err()
            {
                std::future::pending::<()>().await;
            }
        }
    }
}

/// The run's stop: `signal` (SIGTERM, Ctrl-C) stops it for good, before a
/// restart or during that restart's drain; a restart asked for stops it to
/// start again. One `signal` future is kept throughout, so a signal between
/// the two is not lost.
pub fn stop_or_restart(
    signal: impl Future<Output = ()> + Send + 'static,
    restart: Arc<SelfRestart>,
) -> impl Future<Output = ()> + Send + 'static {
    let requested = restart.requested();
    async move {
        let mut signal = Box::pin(signal);
        tokio::select! {
            () = &mut signal => restart.stop(),
            () = requested => {
                tokio::spawn(async move {
                    signal.await;
                    restart.stop();
                });
            }
        }
    }
}

/// Replace this process with the same executable, arguments and environment.
/// Returns only when that failed.
#[cfg(unix)]
pub fn exec_again() -> std::io::Error {
    use std::os::unix::process::CommandExt;
    match std::env::current_exe() {
        Ok(executable) => std::process::Command::new(executable)
            .args(std::env::args_os().skip(1))
            .exec(),
        Err(error) => error,
    }
}

#[cfg(test)]
mod tests {
    use super::{stop_or_restart, RestartEnd, SelfRestart};
    use std::sync::{Arc, Mutex};
    use std::time::Duration;

    fn quiet() -> super::RestartReport {
        Box::new(|_| {})
    }

    #[tokio::test]
    async fn a_request_resolves_waiters_before_and_after_it() {
        let restart = SelfRestart::default();
        let early = restart.requested();
        assert!(restart.schedule(quiet()));
        assert!(
            tokio::time::timeout(Duration::from_millis(20), restart.requested())
                .await
                .is_err(),
            "held, not asked yet"
        );
        restart.request();
        assert!(restart.restarts());
        tokio::time::timeout(Duration::from_secs(1), early)
            .await
            .expect("a waiter from before the request resolves");
        tokio::time::timeout(Duration::from_secs(1), restart.requested())
            .await
            .expect("a waiter from after the request resolves");
    }

    #[test]
    fn a_request_without_a_held_restart_does_nothing() {
        let restart = SelfRestart::default();
        restart.request();
        assert!(!restart.restarts());
        assert!(restart.serving());
    }

    #[test]
    fn one_restart_is_held_at_a_time() {
        let restart = SelfRestart::default();
        assert!(restart.schedule(quiet()));
        assert!(!restart.serving());
        assert!(!restart.schedule(quiet()), "a second change waits");
    }

    #[test]
    fn a_stop_before_the_request_cancels_the_restart() {
        let restart = SelfRestart::default();
        assert!(restart.schedule(quiet()));
        restart.stop();
        restart.request();
        assert!(!restart.restarts());
        assert!(!restart.schedule(quiet()), "nothing is held once stopping");
    }

    #[test]
    fn a_stop_during_the_restart_s_drain_wins() {
        let restart = SelfRestart::default();
        assert!(restart.schedule(quiet()));
        restart.request();
        assert!(restart.restarts());
        restart.stop();
        assert!(!restart.restarts());
    }

    #[test]
    fn a_run_ending_on_its_own_closes_a_held_restart_only() {
        let held = SelfRestart::default();
        assert!(held.schedule(quiet()));
        held.stop_unless_restarting();
        held.request();
        assert!(!held.restarts());
        let asked = SelfRestart::default();
        assert!(asked.schedule(quiet()));
        asked.request();
        asked.stop_unless_restarting();
        assert!(asked.restarts());
    }

    #[test]
    fn the_held_restart_is_told_how_it_ended() {
        let restart = SelfRestart::default();
        restart.finish(RestartEnd::Exec);
        let told = Arc::new(Mutex::new(Vec::new()));
        let seen = told.clone();
        assert!(restart.schedule(Box::new(move |end| seen.lock().unwrap().push(end))));
        assert!(!restart.schedule(quiet()), "the first report stays");
        restart.finish(RestartEnd::Exec);
        restart.finish(RestartEnd::ExecFailed);
        assert_eq!(
            *told.lock().unwrap(),
            [RestartEnd::Exec, RestartEnd::ExecFailed]
        );
    }

    #[tokio::test]
    async fn a_signal_during_the_restart_s_drain_still_stops_for_good() {
        let restart = Arc::new(SelfRestart::default());
        let (signal, signalled) = tokio::sync::oneshot::channel::<()>();
        let stop = stop_or_restart(
            async move {
                let _ = signalled.await;
            },
            restart.clone(),
        );
        assert!(restart.schedule(quiet()));
        restart.request();
        tokio::time::timeout(Duration::from_secs(1), stop)
            .await
            .expect("the restart stops the run");
        assert!(restart.restarts());
        signal.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(1), async {
            while restart.restarts() {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the signal is still heard during the drain");
    }

    #[tokio::test]
    async fn a_signal_first_stops_the_run_and_cancels_what_is_held() {
        let restart = Arc::new(SelfRestart::default());
        assert!(restart.schedule(quiet()));
        let stop = stop_or_restart(async {}, restart.clone());
        tokio::time::timeout(Duration::from_secs(1), stop)
            .await
            .unwrap();
        restart.request();
        assert!(!restart.restarts());
    }
}
