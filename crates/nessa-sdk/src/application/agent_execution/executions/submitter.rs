use crate::application::agent_execution::agents::AgentError;
use crate::application::agent_execution::permissions::ActionContext;
use crate::application::agent_execution::providers::ApprovalMode;
use crate::domain::agent_execution::executions::Initiation;
use std::fmt;
use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;

/// Who submits a turn: the caller the host verified, and how the turn was
/// started. The SDK saves both with the turn and compares both on retry; a
/// retry with a different context or initiation is a different submission.
/// Construction checks nothing: the host has already verified both.
///
/// A check attached with [`Self::with_after_save`] is not part of that
/// identity. [`Agent::steer`](crate::application::agent_execution::agents::Agent::steer)
/// calls it after the input is durable and before the turn is injected into
/// the provider or queued for a later boundary. Dropping the submitter
/// without that call drops the check. [`Agent::enqueue`](crate::application::agent_execution::agents::Agent::enqueue)
/// does not call it.
///
/// A notice attached with [`Self::when_admitted`] is not part of that
/// identity either. A fresh queue admission calls it once, while the
/// scheduler lock is still held, before the runner is started. A recovered
/// retry does not call it. [`Agent::steer`](crate::application::agent_execution::agents::Agent::steer)
/// does not call it.
#[derive(Clone)]
pub struct Submitter {
    /// Caller attribution verified by the host. Audit, scheduling and queue
    /// evidence keep this alone.
    pub context: ActionContext,
    /// Whether the owner or a share grant started the turn, and whether its
    /// tools were checked. Saved only on the turn's own record.
    pub initiation: Initiation,
    /// Host check run by [`Agent::steer`](crate::application::agent_execution::agents::Agent::steer)
    /// after the save and before injection. Absent for a turn the host does
    /// not re-check. Not compared on retry.
    pub(crate) after_save: Option<Arc<dyn AfterSaveCheck>>,
    /// The approval mode the turn must run at, checked at admission against
    /// the mode the agent would run it at. Not compared on retry.
    pub(crate) approval_mode: Option<ApprovalMode>,
    /// Called once on a fresh queue admission, before the runner starts.
    /// Not compared on retry. Absent when the host has nothing to arm.
    pub(crate) admission: Option<Arc<dyn AdmissionNotice>>,
}

/// Told once that a queued turn was admitted, before its runner can publish.
///
/// The scheduler lock is held. The notice returns without waiting and without
/// calling back into the agent; a call that takes the scheduler lock again
/// deadlocks the admission. A panic fails that admission task. A recovered
/// retry of the same submission does not call it, and neither does
/// [`Agent::steer`](crate::application::agent_execution::agents::Agent::steer).
pub trait AdmissionNotice: Send + Sync {
    /// The turn is queued and its runner has not been started.
    fn admitted(&self);
}

/// Host decision after a steered turn's input is saved and before the
/// provider is asked to inject it, or before it is queued when it cannot
/// join the running turn.
///
/// The SDK calls [`AfterSaveCheck::after_save`] once, still holding its
/// scheduler lock. `Err` settles the saved attempt and does not inject or
/// queue it. `Ok(())` may keep a permit until
/// [`AfterSaveCheck::provider_operation_started`], which runs once the
/// provider operation has started and before the SDK waits for that
/// operation's response, and also when the SDK will not inject. A panic
/// from the check is the scheduling task's panic. Dropping the submitter
/// before `steer` runs drops the check without calling it; the check owns
/// whatever it must release on drop.
pub trait AfterSaveCheck: Send + Sync {
    /// Whether the saved turn may still be injected or queued.
    ///
    /// `Ok(())` continues. [`AfterSaveRefusal::Refused`] and
    /// [`AfterSaveRefusal::Unavailable`] both leave the provider uncalled;
    /// the first is the host's refusal, the second is the host being unable
    /// to decide. A permit kept for the provider call stays with the check
    /// until [`Self::provider_operation_started`].
    fn after_save<'a>(
        &'a self,
    ) -> Pin<Box<dyn Future<Output = Result<(), AfterSaveRefusal>> + Send + 'a>>;

    /// The provider operation has started, or the SDK will not inject.
    ///
    /// Release whatever [`Self::after_save`] kept so a later change can
    /// commit. Called before the SDK waits for the provider's response, and
    /// on the paths that queue or refuse instead of injecting. A provider
    /// operation that panics before it calls this is still released, before
    /// the SDK waits for that session's cleanup. A close that lands after
    /// the save check and before that provider future is polled releases it
    /// too, before the SDK settles the refusal. A second call releases
    /// nothing. A check that kept nothing does nothing.
    fn provider_operation_started(&self) {}
}

/// Why [`AfterSaveCheck`] declined to start a turn that was already saved.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AfterSaveRefusal {
    /// The host refuses to start this saved turn. Nothing is injected or queued.
    Refused,
    /// The host could not decide. Nothing is injected or queued.
    Unavailable,
}

impl Submitter {
    /// A turn started by `initiation` through `context`.
    ///
    /// No [`AfterSaveCheck`] is attached. `context` is the caller the host
    /// verified. `initiation` is the owner or the share grant, and whether
    /// its tools were checked.
    pub fn new(context: ActionContext, initiation: Initiation) -> Self {
        Self {
            context,
            initiation,
            after_save: None,
            approval_mode: None,
            admission: None,
        }
    }

    /// A turn the conversation's owner submits through `context`; its tools
    /// are not checked.
    pub fn owner(context: ActionContext) -> Self {
        Self::new(context, Initiation::owner())
    }

    /// Re-check `check` after this steer is saved and before it is injected.
    ///
    /// Replaces a check already attached. The SDK keeps the `Arc` until
    /// `steer` finishes or the submitter is dropped, whichever is later for
    /// the clones it makes. One call of [`AfterSaveCheck::after_save`] is
    /// the decision; later clones do not ask again unless the host's check
    /// does.
    pub fn with_after_save(mut self, check: Arc<dyn AfterSaveCheck>) -> Self {
        self.after_save = Some(check);
        self
    }

    /// Admit this turn only where it runs at `mode`.
    ///
    /// Require the preset of the configuration that accepts this input.
    /// Native steering uses the active invocation's configured provider; after
    /// a NextTurn model change this can differ from
    /// [`Agent::approval_mode`](crate::application::agent_execution::agents::Agent::approval_mode).
    /// A new queued invocation uses the current configured provider instead.
    /// A mismatch returns
    /// [`AgentError::ApprovalModeChanged`](crate::application::agent_execution::agents::AgentError::ApprovalModeChanged).
    /// Fresh admission refuses before saving. If native steering explicitly
    /// requires an independent prompt, the attempted steering record can already
    /// be saved, but the current preset is checked before queue ownership or its
    /// admission audit. Exact saved retries recover their original receipt first.
    pub fn with_approval_mode(mut self, mode: ApprovalMode) -> Self {
        self.approval_mode = Some(mode);
        self
    }

    /// Run `notice` once when this submission is freshly queued.
    ///
    /// Replaces a notice already attached. The call happens on the admission
    /// task, still holding the scheduler lock, before
    /// [`Agent::enqueue`](crate::application::agent_execution::agents::Agent::enqueue)
    /// starts the runner, so a reader cannot observe the turn's reply before
    /// the notice has returned. A recovered retry does not call it. The notice
    /// is not part of retry identity.
    pub fn when_admitted(mut self, notice: Arc<dyn AdmissionNotice>) -> Self {
        self.admission = Some(notice);
        self
    }

    /// Whether a turn the agent would run at `mode` is one this submitter
    /// may be admitted for.
    pub(crate) fn admits(&self, mode: Option<ApprovalMode>) -> Result<(), AgentError> {
        match self.approval_mode {
            Some(required) if mode != Some(required) => Err(AgentError::ApprovalModeChanged),
            _ => Ok(()),
        }
    }
}

impl PartialEq for Submitter {
    fn eq(&self, other: &Self) -> bool {
        self.context == other.context && self.initiation == other.initiation
    }
}

impl Eq for Submitter {}

impl fmt::Debug for Submitter {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("Submitter")
            .field("context", &self.context)
            .field("initiation", &self.initiation)
            .field("after_save", &self.after_save.is_some())
            .field("approval_mode", &self.approval_mode)
            .field("admission", &self.admission.is_some())
            .finish()
    }
}
