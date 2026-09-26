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
    /// A bounded collection holds more entries than it permits.
    TooManyValues {
        /// Collection whose length was rejected.
        field: &'static str,
        /// Maximum permitted number of entries.
        max: usize,
    },
    /// A path description is empty or contains a NUL byte.
    InvalidPath,
    /// A linked file's path does not start at the root. Only an absolute path
    /// names a file, because the agent's working directory is not the caller's.
    RelativeFilePath,
    /// A linked file's path holds a control character. No path needs one, and
    /// a path with one cannot be shown to whoever approves the read, written
    /// into an audit record, or read back out of a log as the same path.
    ControlCharacterInFilePath,
    /// A linked file's path ends in a separator, so it names a directory and
    /// there is no file name to show for it.
    FilePathWithoutName,
    /// A linked file's path has a `.` or `..` component. Written as a URI that
    /// component is resolved away, which would make the link name a different
    /// path from the one given.
    UnresolvedFilePathComponent,
    /// A linked file's path has a doubled separator, so one of its components
    /// names nothing. Written as a URI that component disappears, which would
    /// make the link name a different path from the one given.
    RepeatedSeparatorInFilePath,
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
    /// Two questions in one ask share a key, so an answer to either could not be
    /// correlated back to the question that wanted it.
    DuplicateQuestionKey,
    /// Two answers to one question record the same value, so a host choosing
    /// between them could not say which it meant.
    DuplicateAnswerOption,
    /// An answer names a question the agent did not ask in this ask.
    UnaskedQuestion,
    /// A question the asker said may not be skipped was answered with no
    /// choice.
    UnansweredQuestion,
    /// An answer chooses something the question did not offer, or supplies
    /// prose where the question invited none.
    UnofferedAnswer,
    /// The selected option was not offered by this request.
    UnknownPermissionOption,
}
impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent execution: {self:?}")
    }
}
impl Error for ExecutionError {}
