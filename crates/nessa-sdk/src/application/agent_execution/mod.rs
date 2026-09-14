//! Agent coordinates invocation through provider, session storage, and hook ports.
//! Domain aggregates retain live execution invariants; application snapshots retain
//! submitted input and observations without replaying provider work.
//!
//! ```text
//! caller -> Agent -> SessionManager -> SessionStorage
//!              |-> AgentProvider -> provider controls and observations
//!              |-> InvocationHooks
//! adapter -> ExecutionController -> domain session
//! adapter -> ExecutionAudit
//! ```
//! Arrows show calls. Authentication stays with the host; mandatory permission
//! audit remains separate from session snapshots and optional UI subscribers.

pub mod agents;
pub mod executions;
pub mod hooks;
pub mod permissions;
pub mod providers;
pub mod sessions;
pub mod tools;
