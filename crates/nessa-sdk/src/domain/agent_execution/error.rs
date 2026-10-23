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
    /// An MCP server or tool name holds whitespace or a control character, so
    /// it cannot be shown on one line or compared as the name it was given as.
    /// Identifies which of the two.
    InvalidMcpToolName(&'static str),
    /// A structured tool result is not one JSON value.
    InvalidStructuredResult,
    /// An app's structured context is not the JSON text of one object.
    InvalidStructuredContent,
    /// Arguments of an MCP tool call are not the JSON text of one object.
    InvalidMcpCallArguments,
    /// The input an agent reported for a tool call is not the JSON text of
    /// one object.
    InvalidToolInput,
    /// An update names a different MCP server or tool than the one already
    /// observed for this tool call. The first identity stands.
    DifferentMcpTool,
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
    /// A lease identity is not a portable key.
    InvalidLeaseId,
    /// A lease revision is zero, which names no issuance.
    InvalidLeaseRevision,
    /// An SSH destination outside the portable alphabet or its bound.
    InvalidSshDestination,
    /// A `peer:` environment name that holds no device key in lowercase hex.
    InvalidPeerEnvironment,
    /// A command's timeout is zero or past the longest a command may run.
    InvalidCommandTimeout,
    /// A command argument holds a control character other than newline or
    /// tab: no program needs one, and a record of it could not keep its bound.
    InvalidCommandArgument,
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
    /// A local declined-review identity is not a checked positive decimal sequence.
    InvalidReviewDeclineId,
    /// A restored declined-review tool label violates the retained label contract.
    InvalidReviewDeclineToolName,
    /// Declined-review evidence did not advance once from selection to a final write fact.
    InvalidReviewDeclineTransition,
    /// A declined-review record pairs its outcome with a rule incorrectly: a
    /// stopping outcome has no rule, or a continue outcome has one.
    InvalidReviewDecline,
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
    /// Open asks exceed their admitted count or carrying-cost limits, or refusal
    /// evidence disagrees with its reason: a room refusal requires a candidate
    /// that does not fit, and other refusal reasons carry no candidate.
    InvalidQuestionRefusal,
    /// An agent's question holds a control character it cannot be shown with:
    /// any in an identity, or any but a line break or tab in text. Identifies
    /// the field.
    ControlCharacterInQuestion(&'static str),
    /// A question the asker said may not be skipped was answered with no
    /// choice.
    UnansweredQuestion,
    /// An answer chooses something the question did not offer, or supplies
    /// prose where the question invited none.
    UnofferedAnswer,
    /// The selected option was not offered by this request.
    UnknownPermissionOption,
    /// A permission deadline waits no time, so the request would be decided in
    /// the instant it appeared.
    ZeroPermissionDeadline,
    /// A permission deadline waits longer than a day, so a misconfiguration
    /// could push the moment past what the clock can reach and leave the
    /// review waiting.
    PermissionDeadlineTooLong,
    /// A permission deadline's wait is not a whole number of milliseconds.
    /// The wire and the saved record carry `waitMs`, so a part of a
    /// millisecond would be lost on the way: a wait under one would read as
    /// zero, which a client refuses.
    FractionalPermissionDeadline,
    /// A permission deadline reason holds a control character, so it cannot be
    /// shown on one line.
    ControlCharacterInPermissionReason,
    /// A class deadline was installed in the other class's slot.
    MismatchedPermissionDeadline,
    /// An allow deadline records that it stops the turn. Only a refusal can
    /// stop the turn, so this combination is not a deadline.
    AllowDeadlineStopsTurn,
    /// A deadline installed as a class policy carries state only a settled or
    /// live review has: it stops the turn, has a due time, or is armed. A
    /// policy is a template, so a settlement would inherit that state.
    SettledPermissionDeadlineTemplate,
    /// A deadline's due time is zero or later than a client can read exactly,
    /// so the card would show a moment other than the one the worker keeps.
    UnrepresentablePermissionDeadlineDue,
    /// A cancellation code is in the namespace only the runner writes, and was
    /// not issued for the review it would withdraw.
    ReservedCancellationCode,
}
impl fmt::Display for ExecutionError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "agent execution: {self:?}")
    }
}
impl Error for ExecutionError {}
