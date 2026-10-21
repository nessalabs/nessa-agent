//! Agent is the public entry point for invocation, hooks, observations, and controls.
//!
//! ```text
//! caller -> supervised initialization -> Agent
//!              |-> AgentInitializationError -> attachment cleanup retry
//! Agent -> SessionManager -> SessionStorageLease
//!       |-> AgentProvider -> provider session + event reader
//!       |-> saved submission identity -> shared retry receipt
//!       |-> supervised direct invocation / bounded queue -> invocation hooks
//!       |-> queue settlement -> bounded audit observation -> original work retirement
//!                                                        |-> receipt publication
//!       |-> control admission -> provider acknowledgement -> local validation / saved receipt
//!       |                        |-> pending wait interrupted by shared stop
//!       |-> SessionLifecycle -> work permits / work generations
//!                              |-> attachment generation / active dispatch
//!                              |-> owner-issued attachment audit records + evidence ledger
//!                              |                   |-> shared bounded AuditObserver
//!                              |-> cleanup joins physical result + bounded evidence slots
//!                              |-> shared stop -> cleanup attempts / retained lease
//!       |-> historical decline -> saved original preparation -> terminal edge
//!                                |-> idempotent audit -> saved acknowledgement
//!       |-> AgentCloseReport -> same close transaction cleanup + scheduling evidence
//!                              |-> optional owned-lifetime seal read before admission
//!       |-> event subscribers
//! ```
//! Arrows show calls. Agent drains provider events and saves evidence before
//! publishing observations. Error preflight bounds adapter diagnostics before
//! retention while preserving lifecycle decisions. [`ProviderDiagnostic`]
//! retains bounded provider prose separately from the numeric provider code;
//! neither diagnostic text nor its presence grants lifecycle authority.
//! Provider and storage adapters remain injected.
//! `models` owns acknowledged live model binding changes. The scheduler excludes
//! new admission while a control is verified and persisted; the active native
//! turn and stable trace port retain their identities. Model-change factories
//! normalize all acknowledged settings for restoration under the same fence.
//! The lifecycle coordinator alone opens/closes admission. Supervised submission
//! owners retain work permits through evidence and receipt settlement. Their original
//! queue permit and invocation slot retire before receipt publication. Waiting
//! callers own no dispatch authority. Attachment generation and admission work generation
//! remain distinct: new queued input may be stopped without restoring a provider.

mod agent;
mod attachment;
mod attachment_evidence;
mod audit_observation;
mod closing;
mod coordination;
mod error;
mod error_limits;
pub(crate) use error_limits::DiagnosticTreeLimits;
mod initialization;
pub(crate) mod lifecycle;
mod lifetime;
mod resumption;
mod scheduling;
mod submissions;

pub use agent::{Agent, AgentEvents};
pub use attachment::{
    AttachmentAuthorization, AttachmentCancellation, AttachmentFailure, AttachmentFailureCode,
    AttachmentPhase, AttachmentRequest, AttachmentStatus, AttachmentWait,
};
pub use closing::AgentCloseReport;
pub use error::{
    AgentError, AgentFuture, AgentStartupContext, AgentStartupPhase, AgentStartupStep,
    ProviderDiagnostic,
};
pub use initialization::AgentInitializationError;
pub use lifetime::{lifetime_disposition, LifetimeDisposition, OwnedLifetime};
pub(crate) use resumption::{decline_stop_is_owed, validate_saved_resumption};
pub use resumption::{owed_turn_ids, resumed_turn_id, OwedTurn, Resumed};
pub use scheduling::{
    AdmissionEvidence, AdmissionEvidenceFailure, QueueAdmission, QueueRemoval, QueueReorder,
    SteeringDelivery, SteeringEvidence,
};

mod models;
