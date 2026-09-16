//! Execution requests and projections connect adapters to domain session state.
//! The controller owns review-input pairing and resource limits, while the domain
//! owns admission, observations, and permission transitions. The mandatory audit
//! port retains queue order, session closure, and permission evidence independently
//! of events.
//! Shared identity limits keep live admission and restored observations consistent.
//!
//! ```text
//! adapter --> ExecutionController --> domain session
//!                 |
//!                 +--> ExecutionEvent
//! Agent / adapter --> ExecutionAudit --> host-owned durable sink
//! ```
//!
//! Arrows mean calling the controller or domain, and constructing an event from
//! the accepted domain transition. No transport lives in this module.

mod audit;
mod controller;
mod events;
pub(crate) mod limits;
mod request;
pub use audit::{
    ExecutionAudit, ExecutionAuditRecord, QueueOrderCause, QueueOrderRecord, SessionClosureRecord,
};
pub use controller::ExecutionController;
pub use events::{ExecutionEvent, ExecutionUpdate};
pub use request::ExecutionRequest;

pub use crate::domain::agent_execution::executions::SubmissionMode;
