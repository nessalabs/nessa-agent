//! Retryable desktop gateway reconciliation; native effects enter through the owned port.
mod ports;
mod service;
pub use crate::gateway::domain::value_objects::ReconciliationHistoryFact;
#[cfg(test)]
pub(crate) use ports::testing;
pub use ports::{
    GatewayError, GatewayHost, GatewayPhysicalResult, GatewayReconciliationAttempt,
    GatewayReconciliationAudit, GatewayReconciliationEffect, GatewayReconciliationEffectTiming,
    GatewayReconciliationIds, GatewayReconciliationIntent, GatewayReconciliationIntentDelivery,
    GatewayReconciliationJournalSession, GatewayReconciliationOutcome,
    GatewayReconciliationProgress, GatewayReconciliationRequest, GatewayStartup,
    GatewayStartupEvents, GatewayStartupPhase, GatewayStopRequest, GatewayStopSession,
    LoginShellError, LoginShellPath, MonotonicClock, ReconciledGateway, SystemMonotonicClock,
};
pub use service::{Gateway, GatewayRuntimeDependencies};
