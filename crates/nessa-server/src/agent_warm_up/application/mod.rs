//! Coordinates the one-time runtime preparation and owns its ports.
//!
//! `service -> domain` names the runtime; `service -> ports` commits evidence
//! and the completion record. The conversation context consumes this through
//! its own `RuntimeReadiness` port, so nothing here depends on conversations.
mod ports;
mod service;
pub use ports::{
    ProviderFailure, WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpFuture, WarmUpRecords,
};
pub use service::AgentWarmUp;
