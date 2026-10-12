use crate::application::agent_execution::permissions::ActionContext;
use crate::domain::agent_execution::executions::Initiation;

/// Who submits a turn: the caller the host verified, and how the turn was
/// started. The SDK saves both with the turn and compares both on retry; a
/// retry with a different context or initiation is a different submission.
/// Construction checks nothing: the host has already verified both.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Submitter {
    /// Caller attribution verified by the host. Audit, scheduling and queue
    /// evidence keep this alone.
    pub context: ActionContext,
    /// Whether the owner or a share grant started the turn, and whether its
    /// tools were checked. Saved only on the turn's own record.
    pub initiation: Initiation,
}
impl Submitter {
    /// A turn the conversation's owner submits through `context`; its tools
    /// are not checked.
    pub fn owner(context: ActionContext) -> Self {
        Self {
            context,
            initiation: Initiation::owner(),
        }
    }
}
