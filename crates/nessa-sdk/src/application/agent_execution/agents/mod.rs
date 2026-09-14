//! Agent is the public entry point for invocation, hooks, observations, and controls.
//!
//! ```text
//! caller -> supervised initialization -> Agent
//!              |-> AgentInitializationError -> attachment cleanup retry
//! Agent -> SessionManager -> SessionStorageLease
//!       |-> AgentProvider -> provider session + event reader
//!       |-> saved submission identity -> shared retry receipt
//!       |-> supervised direct invocation / bounded queue -> invocation hooks
//!       |-> control admission -> provider acknowledgement -> local validation / saved receipt
//!       |                        |-> pending wait interrupted by shared stop
//!       |-> SessionLifecycle -> work permits / work generations
//!                              |-> attachment generation / active dispatch
//!                              |-> shared stop -> cleanup facts / retained lease
//!       |-> event subscribers
//! ```
//! Arrows show calls. Agent drains provider events and saves evidence before
//! publishing observations. Error preflight bounds adapter diagnostics before
//! retention while preserving lifecycle decisions. Provider and storage adapters
//! remain injected.
//! The lifecycle coordinator alone opens/closes admission. Supervised submission
//! owners retain work permits through evidence and receipt settlement; waiting
//! callers own no dispatch authority. Attachment generation and admission work generation
//! remain distinct: new queued input may be stopped without restoring a provider.

mod agent;
mod coordination;
mod error;
mod error_limits;
mod initialization;
mod lifecycle;
mod scheduling;
mod submissions;

pub use agent::{Agent, AgentEvents};
pub use error::{AgentError, AgentFuture};
pub use initialization::AgentInitializationError;
pub use scheduling::{QueueRemoval, QueuedInvocation, SteeringDelivery};
