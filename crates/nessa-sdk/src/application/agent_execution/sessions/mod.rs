//! `committed_changes` owns payloadless interest independent of save bindings,
//! writer leases and receiver progress. RecordStorage closes interest before
//! shutdown joins.
//! Queue evidence follows the same consistency boundary as pending dispatch:
//! ```text
//! Agent scheduler -> InvocationQueue -> actual membership/order changes
//!                 -> SessionManager -> SessionSnapshot.queue_history
//! snapshot queue history -> replayed InvocationQueue -> complete-order validation
//! invocation scheduling -> correlated checkpoint -> lifecycle/cause validation
//! ```
//! Arrows show ownership and validation flow. Local queue selection is saved
//! before another command can reorder the remainder; it is not provider dispatch.
//!
//! SessionManager owns local session identity, saved evidence, and the storage lease.
//!
//! ```text
//! Agent -> SessionManager -> SessionStorageLease -> storage adapter
//! Agent lifecycle -> AttachmentLease -> provider cleanup + protective writer lease
//! SessionManager -> SessionSnapshot + ordered SessionChange decisions
//!                + lease-scoped SessionSaveGeneration
//!                + injected MessageCommitClock -> deadline wake
//!                -> SessionStorageLease
//!                       |-> validation -> domain InvocationHistory / permission rules
//!                                      |-> retention -> live controller limits
//! ```
//! Arrows show coordination and retained evidence. Provider-private execution
//! state stays with the provider; snapshots never replay prompts automatically.
//! CommittedTranscript -> records::continuation -> reversible records transitions
//!                     -> validation::observations + retention::intervals
//!                     -> existing domain history/queue and live controller limits
//! The continuation retains derived indices/accounting; read publication creates
//! one immutable full snapshot. Receiver guard Drop reverses only staged changes.
//! Validation checks cross-record evidence at the application boundary, including
//! custom storage adapters, before opening a provider or accepting observations.
//! Per-invocation output accounting bounds saved observations independently from
//! transient adapter queues; the same borrowed check precedes live event copies.
//! Retention validation reuses live admission on the least-retained history that
//! observations allow: later cancellations prove pending lifetimes, while absent
//! answer observations cannot prove concurrency. Mandatory answer audit stays separate.
//! The manager records live dispatch ownership immediately before provider polling;
//! queued admission and imported history cannot authorize new observations.
//! Ordinary output authority ends when its observation loop ends; retained IDs
//! allow only correlated trailing permission cancellation. Confirmed panic cleanup
//! drains already-ready evidence before retiring that authority.
//! Attachment ownership survives initialization failure and Agent drop until
//! cleanup is confirmed; supervised retry requires the runtime to remain alive.
//! Initialization transfers the attachment to Agent's lifecycle coordinator.
//! Provider settlement facts are retained separately from local receipt failures.
//! Streaming text is live output until the fixed deadline, size/count cadence,
//! or a tool/review/terminal or settlement save.
//! A process failure may lose unfinished text. Consequential changes require saved
//! evidence; snapshots expose only committed state. Adapters choose its encoding.
//! `app_sources` owns which apps a message may name: MCP tool calls the
//! session recorded before it, asked at admission and of restored history.
//! `steering_position` owns where a steered message stands in its target turn:
//! taken at admission from the target's saved events, and read once from saved
//! history for restoration, replay, `app_sources` and provider correlation
//! evidence.

mod app_sources;
pub use app_sources::UnknownApp;
pub(crate) mod attachment;
pub(crate) mod committed_changes;
mod manager;
mod message_commit_clock;
pub use committed_changes::{ChangeWatchError, ChangeWatchState, CommittedChangeWatch};
pub(crate) mod records;
mod retained;
mod retention;
mod steering_position;
#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/sessions/support.rs"]
mod test_support;
// Queue membership is replayed separately from provider/lifecycle scheduling.
mod queue_validation;
pub mod storage;
mod transcript;
pub use transcript::{
    CommittedCompleteness, CommittedFreshness, CommittedStatus, CommittedTranscript,
    CommittedViewState,
};
pub(crate) mod validation;
pub use manager::SessionManager;
pub(crate) use manager::{AttachedProvider, AttachmentOpenFailureSource};
pub use message_commit_clock::{MessageCommitClock, MessageCommitSleep};
pub use storage::{
    CommittedSession, InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent,
    ProviderContext, QueueHistoryRecord, SessionChange, SessionLoad, SessionLoadState,
    SessionSaveBackend, SessionSaveGeneration, SessionSaveReceipt, SessionSaveUnit,
    SessionSnapshot, SessionStorage, SessionStorageLease, StorageError, StorageFuture,
    StorageShutdownFailure, SubmissionAcknowledgement,
};

pub(crate) use transcript::CommittedTransactionState;

#[cfg(test)]
pub(crate) use retained::SNAPSHOT_ACCOUNTING_CALLS;
#[cfg(test)]
pub(crate) use validation::VALIDATION_CALLS;
