//! Coordinates the one-time runtime preparation and owns its ports.
//!
//! `service -> domain` names the runtime; `service -> ports` commits evidence
//! and the completion record; `service -> terminal` publishes the four facts
//! composition needs without flattening physical ownership into success. The
//! conversation context consumes settlement through its own `RuntimeReadiness`
//! port, so nothing here depends on conversations.
mod ports;
mod service;
mod terminal;
pub use ports::{
    ProviderFailure, WarmUpAudit, WarmUpAuditRecord, WarmUpError, WarmUpFuture, WarmUpRecords,
};
pub use service::AgentWarmUp;
#[cfg(any(unix, test))]
pub(crate) use terminal::WarmUpLaunchOwnership;
#[cfg(test)]
pub(crate) use terminal::{WarmUpAuditDelivery, WarmUpCompletionRecordDelivery, WarmUpEffect};
