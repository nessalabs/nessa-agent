//! SessionManager owns local session identity, saved evidence, and the storage lease.
//!
//! ```text
//! Agent -> SessionManager -> SessionStorageLease -> storage adapter
//! Agent lifecycle -> AttachmentLease -> provider cleanup + protective writer lease
//! SessionManager -> SessionSnapshot (input, scheduling, observations, settlement)
//!                       |-> validation -> domain InvocationHistory / permission rules
//!                                      |-> retention -> live controller limits
//! ```
//! Arrows show coordination and retained evidence. Provider-private execution
//! state stays with the provider; snapshots never replay prompts automatically.
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
//! Streaming text is live output until a tool/review/terminal or settlement save.
//! A process failure may lose unfinished text. Consequential changes require saved
//! evidence; snapshots expose only committed state. Adapters choose its encoding.

pub(crate) mod attachment;
mod manager;
mod retention;
pub mod storage;
pub(crate) mod validation;
pub use manager::SessionManager;
pub use storage::{
    InvocationCancellationEvent, InvocationRecord, InvocationSchedulingEvent, SessionSnapshot,
    SessionStorage, SessionStorageLease, StorageError, StorageFuture,
};
