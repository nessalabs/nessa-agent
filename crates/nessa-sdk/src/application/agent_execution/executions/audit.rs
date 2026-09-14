//! Mandatory execution lifecycle evidence, independent of live event consumers.
#![deny(missing_docs)]

use crate::application::agent_execution::agents::{AgentError, AgentFuture};
use crate::application::agent_execution::permissions::{
    CancellationOrigin, PermissionAnswerRecord, PermissionCancellation,
};
use crate::domain::agent_execution::sessions::{ExecutionFinish, SessionClosure};

/// The local live aggregate's closure, paired with its known initiator.
/// This evidence does not claim provider deletion or completed process cleanup.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SessionClosureRecord {
    closure: SessionClosure,
    origin: CancellationOrigin,
}
impl SessionClosureRecord {
    /// Pairs the domain's once-only `closure` evidence with the known `origin`.
    /// The host supplies verified caller attribution for explicit closure; adapters
    /// label provider and automatic causes. Contradictory cause/origin pairs return
    /// InvalidInput. Construction performs no I/O.
    pub fn new(closure: SessionClosure, origin: CancellationOrigin) -> Result<Self, AgentError> {
        origin.validate_reason(closure.reason())?;
        Ok(Self { closure, origin })
    }
    /// Session identity, optional active execution, and causal lifecycle reason.
    pub fn closure(&self) -> &SessionClosure {
        &self.closure
    }
    /// Verified client attribution or the honestly labelled automatic origin.
    pub fn origin(&self) -> &CancellationOrigin {
        &self.origin
    }
}

/// Evidence supplied to the mandatory audit sink in causal order.
#[derive(Clone, Debug, PartialEq, Eq)]
#[must_use = "execution evidence must be recorded or its delivery failure reported"]
pub enum ExecutionAuditRecord {
    /// Once-only release of an active execution, including runs with no permissions.
    /// The runtime initiates this release after observing a terminal result; explicit
    /// shutdown attribution remains on the preceding SessionClosed record.
    Finished(ExecutionFinish),
    /// Once-only closure of a live aggregate, including idle contexts.
    SessionClosed(SessionClosureRecord),
    /// Once-only cancellation with its original lifecycle cause and initiator.
    Cancelled(PermissionCancellation),
    /// Selection before effects, followed by a separate wire delivery observation.
    Answered(PermissionAnswerRecord),
}

/// Required audit boundary, independent of bounded UI streams and caller waits.
/// Composition supplies controlled storage and documents its durability contract.
/// Implementations assign record identities and observation/commit times through
/// injected infrastructure. Exact inputs must not be copied into general logs.
pub trait ExecutionAudit: Send + Sync {
    /// Accept `record` under the sink's durability contract before returning success.
    /// Errors or bounded adapter timeouts prevent successful audited acknowledgements
    /// but must not prevent necessary cleanup. Delivery observations never establish
    /// provider acknowledgement or tool execution. Record every call in order.
    fn record(&self, record: ExecutionAuditRecord) -> AgentFuture<'_, ()>;
}
