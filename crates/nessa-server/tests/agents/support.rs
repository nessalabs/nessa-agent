//! A host that answers whatever the test says it does, so that no test asks the
//! developer's own machine about their credentials.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Condvar, Mutex};
use std::time::Duration;

use crate::agents::application::{AgentProbe, ProbeFailure};
use crate::agents::domain::AgentId;

/// A host with both of its answers fixed in advance, including the answer that
/// it could not answer.
pub(crate) struct StubAgentProbe {
    pub(crate) installed: Result<bool, ProbeFailure>,
    pub(crate) authenticated: Result<bool, ProbeFailure>,
}

impl StubAgentProbe {
    /// A host that answers both questions plainly.
    pub(crate) fn answering(installed: bool, authenticated: bool) -> Self {
        Self {
            installed: Ok(installed),
            authenticated: Ok(authenticated),
        }
    }
}

impl AgentProbe for StubAgentProbe {
    /// Configured for every agent: these tests are about what the machine
    /// answers, and an unconfigured agent is never asked.
    fn configured(&self, _agent: AgentId) -> bool {
        true
    }

    fn installed(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        self.installed
    }

    fn authenticated(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        self.authenticated
    }
}

/// How long a blocked probe waits before giving up on ever being released.
///
/// Nothing in these tests should reach it: it is there so that a test that fails
/// to release its probe fails as a test rather than hanging the suite, and so
/// that no blocking thread outlives the runtime it was started under.
const GATE_SAFETY_LIMIT: Duration = Duration::from_secs(5);

/// A host that does not answer until the test lets it.
///
/// Answering on the real machine is slow in ways a test cannot reproduce — a
/// keychain that will not reply, a filesystem that hangs — so the slowness is
/// made explicit and controlled instead. It also counts itself: how many probes
/// ran, and the most that were ever inside at one time, which is the number the
/// bound on this route is about.
#[derive(Default)]
pub(crate) struct WaitingAgentProbe {
    released: Mutex<bool>,
    wakeup: Condvar,
    runs: AtomicUsize,
    inside: AtomicUsize,
    most_at_once: AtomicUsize,
}

impl WaitingAgentProbe {
    /// Let every blocked probe finish, and every later one pass straight through.
    pub(crate) fn release(&self) {
        *self
            .released
            .lock()
            .unwrap_or_else(|held| held.into_inner()) = true;
        self.wakeup.notify_all();
    }

    /// How many probes have begun.
    pub(crate) fn runs(&self) -> usize {
        self.runs.load(Ordering::SeqCst)
    }

    /// The most probes that were ever running at the same moment.
    pub(crate) fn most_at_once(&self) -> usize {
        self.most_at_once.load(Ordering::SeqCst)
    }

    /// Wait until a probe has actually begun, so a test can be sure the first
    /// caller is in flight before the next one arrives. Polled rather than
    /// signalled, because the waiting side is async and the probe side is not.
    pub(crate) async fn started(&self) {
        for _ in 0..500 {
            if self.runs() > 0 {
                return;
            }
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
        panic!("the probe never started");
    }

    /// Enter, record, wait for release, leave.
    fn probe(&self) {
        self.runs.fetch_add(1, Ordering::SeqCst);
        let inside = self.inside.fetch_add(1, Ordering::SeqCst) + 1;
        self.most_at_once.fetch_max(inside, Ordering::SeqCst);
        let mut released = self
            .released
            .lock()
            .unwrap_or_else(|held| held.into_inner());
        while !*released {
            let (next, timed_out) = self
                .wakeup
                .wait_timeout(released, GATE_SAFETY_LIMIT)
                .unwrap_or_else(|held| held.into_inner());
            released = next;
            if timed_out.timed_out() {
                break;
            }
        }
        drop(released);
        self.inside.fetch_sub(1, Ordering::SeqCst);
    }
}

impl AgentProbe for WaitingAgentProbe {
    fn configured(&self, _agent: AgentId) -> bool {
        true
    }

    fn installed(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        self.probe();
        Ok(true)
    }

    fn authenticated(&self, _agent: AgentId) -> Result<bool, ProbeFailure> {
        Ok(true)
    }
}
