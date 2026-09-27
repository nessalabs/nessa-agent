use std::sync::{Arc, Mutex};
use std::time::Duration;

use tokio::sync::watch;

use crate::agents::application::ports::AgentProbe;
use crate::agents::application::readiness::ReadAgentReadiness;
use crate::agents::domain::{AgentId, Readiness};

/// How long a caller waits for the machine's answer before giving up on it.
///
/// Sized from the one part of answering that is genuinely slow: the keychain
/// lookup, which `infrastructure/claude.rs` already caps at three seconds. Five
/// leaves that cap room to be the thing that fires — so a locked keychain is
/// reported as a locked keychain — while still putting a ceiling on the request
/// when something further down takes longer than anything here expects.
pub const READINESS_DEADLINE: Duration = Duration::from_secs(5);

/// Why a reading produced no answer.
///
/// Two failures rather than one, because they are different facts about this
/// server: one is a machine that would not answer in time, the other is the
/// asking itself coming apart. Neither is ever turned into a readiness.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadingFailure {
    /// The machine was asked and had not answered by the deadline.
    Undetermined,
    /// The asking came apart — the host thread panicked, or the runtime is
    /// going away — so there is no answer and no reason to expect one.
    Lost,
}

/// One probe's answers, shared by everyone who was waiting for it.
type Answers = Arc<Vec<(AgentId, Readiness)>>;

/// Asks this machine about its agents, at most once at a time.
///
/// The question this route answers takes no parameters: every caller is asking
/// the identical "what could start here?", so concurrent callers can share one
/// probe's answer instead of each starting their own. That is what bounds the
/// cost. Without it, N requests mean N blocking threads and, on macOS, N
/// `security` subprocesses — and the route is unauthenticated, so N is chosen by
/// whoever is calling.
///
/// ```text
///   first caller  ──starts──▶  one probe on one blocking thread
///   later callers ──join────▶  the same probe's answer
///   next caller, after it finishes ──starts──▶ a fresh probe
/// ```
///
/// Deliberately **not** a cache. Nothing is kept once a probe has finished: a
/// caller that arrives afterwards asks the machine again. Setup's "check again"
/// exists for the person who installs the agent while this server is running,
/// and an answer served from a moment ago is exactly the staleness that button
/// was added to remove. Sharing an answer with callers who were waiting *while*
/// it was produced takes nothing away from them — they asked and are being told
/// what the machine said during their own request.
pub struct SharedAgentReadiness {
    /// The machine being asked.
    probe: Arc<dyn AgentProbe>,
    /// How long a caller waits for an answer.
    deadline: Duration,
    /// The probe currently running, if one is, as a receiver later callers
    /// clone to wait on its answer. Cleared as soon as it has one.
    in_flight: Arc<Mutex<Option<watch::Receiver<Option<Answers>>>>>,
}

impl SharedAgentReadiness {
    /// Share one probe of `probe` between concurrent callers.
    pub fn new(probe: Arc<dyn AgentProbe>) -> Self {
        Self::with_deadline(probe, READINESS_DEADLINE)
    }

    /// The same, with the caller's wait set explicitly. Tests use this to keep
    /// their own runtime short; composition uses [`READINESS_DEADLINE`].
    pub fn with_deadline(probe: Arc<dyn AgentProbe>, deadline: Duration) -> Self {
        Self {
            probe,
            deadline,
            in_flight: Arc::new(Mutex::new(None)),
        }
    }

    /// What this machine says about every agent, or why it did not say.
    pub async fn read(&self) -> Result<Vec<(AgentId, Readiness)>, ReadingFailure> {
        let mut answer = self.join_or_start();
        match tokio::time::timeout(self.deadline, answer.changed()).await {
            // Nothing is abandoned by giving up here: the probe belongs to a
            // task of its own, it is still the only one running, and it will
            // finish and be cleared away whether or not anyone is left waiting.
            Err(_elapsed) => Err(ReadingFailure::Undetermined),
            Ok(Err(_gone)) => Err(ReadingFailure::Lost),
            Ok(Ok(())) => {
                let answers = answer.borrow().clone();
                answers.map_or(Err(ReadingFailure::Lost), |answers| {
                    Ok(answers.as_ref().clone())
                })
            }
        }
    }

    /// Wait on the probe already running, or start the only one that will be.
    ///
    /// The probe runs in a task of its own rather than inside whichever request
    /// happened to start it, so a caller that gives up — or a browser that
    /// disconnects — cannot leave the others without the answer they are waiting
    /// for, and cannot cause a second probe to be started in place of the one
    /// still running.
    fn join_or_start(&self) -> watch::Receiver<Option<Answers>> {
        let mut in_flight = lock(&self.in_flight);
        if let Some(running) = in_flight.as_ref() {
            return running.clone();
        }
        let (sender, receiver) = watch::channel(None);
        *in_flight = Some(receiver.clone());
        drop(in_flight);

        let probe = self.probe.clone();
        let slot = self.in_flight.clone();
        tokio::spawn(async move {
            // Asking means metadata reads and, on macOS, a subprocess. That is
            // blocking work, so it runs where blocking is what the thread is
            // for — one such thread, however many callers are waiting.
            let answers = tokio::task::spawn_blocking(move || {
                ReadAgentReadiness {
                    probe: probe.as_ref(),
                }
                .all()
            })
            .await;
            // Cleared before the answer is published, so that a caller arriving
            // after this point starts a fresh probe instead of being handed a
            // finished one. Cleared even when the probe came apart, so one
            // panicking answer cannot wedge the route for good.
            *lock(&slot) = None;
            if let Ok(answers) = answers {
                let _ = sender.send(Some(Arc::new(answers)));
            }
            // Dropping the sender without sending is how a lost probe reaches
            // everyone waiting on it.
        });
        receiver
    }
}

/// Take the lock, and keep going if a previous holder panicked.
///
/// What this guards is a handle to the probe that is running, not state with an
/// invariant to protect: a panic elsewhere cannot have left it meaning something
/// false, and refusing to answer any further request because of one would be a
/// worse failure than the one being contained.
fn lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
#[path = "../../../tests/agents/shared_readiness.rs"]
mod tests;
