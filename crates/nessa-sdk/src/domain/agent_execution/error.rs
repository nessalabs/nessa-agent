#![deny(missing_docs)]

use std::{error::Error, fmt};

/// Rejected domain construction or transition; no external effect is performed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ExecutionError {
    /// A required value contains no non-whitespace text; identifies the field.
    EmptyValue(&'static str),
    /// A constrained value exceeds its byte limit.
    ValueTooLong {
        /// Field whose value was rejected.
        field: &'static str,
        /// Maximum permitted UTF-8 byte length.
        max_bytes: usize,
    },
    /// A path description is empty or contains a NUL byte.
    InvalidPath,
    /// The local storage key is not a portable session identity.
    InvalidSessionId,
    /// This live attachment ended; restoration requires a fresh aggregate.
    SessionClosed,
    /// The live attachment already owns an active execution.
    SessionBusy,
    /// The requested tool has not been observed in this execution.
    UnknownTool,
    /// The review is absent from the pending set, including after resolution.
    UnknownPermission,
    /// This review identity was already admitted in the current execution.
    DuplicatePermission,
    /// An update targets a different tool entity.
    DifferentTool,
    /// An action targets a different or inactive execution.
    DifferentExecution,
    /// This execution identity was already admitted by the same live attachment.
    DuplicateExecution,
    /// The failure cause is invalid here, including a deadline without prior close.
    InvalidExecutionFailureReason,
    /// A permission-only cause cannot describe closure of the live session.
    InvalidSessionClosureReason,
    /// A terminal cancellation requires its owning execution finish or session close transition.
    InvalidPermissionCancellationReason,
    /// An answered or cancelled review cannot transition again.
    PermissionResolved,
    /// No permitted choices remain after offer-policy filtering.
    NoPermissionOptions,
    /// Multiple offered choices share the same option identity.
    DuplicatePermissionOption,
    /// The offer policy contains a repeated effect/scope decision.
    DuplicatePermissionDecision,
    /// The selected option was not offered by this request.
    UnknownPermissionOption,
}
impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent execution: {self:?}")
    }
}
impl Error for ExecutionError {}
