//! Retryable desktop gateway reconciliation; native effects enter through the owned port.
mod ports;
mod service;
pub use crate::gateway::domain::value_objects::ReconciliationHistoryFact;
#[cfg(test)]
pub(crate) use ports::testing;
#[cfg(any(target_os = "macos", target_os = "linux"))]
pub use ports::GatewayLifecycleRecovery;
#[cfg(target_os = "linux")]
pub use ports::GatewayStopProofToken;
pub use ports::{
    GatewayError, GatewayHost, GatewayPhysicalResult, GatewayReconciliationAttempt,
    GatewayReconciliationAudit, GatewayReconciliationEffect, GatewayReconciliationEffectTiming,
    GatewayReconciliationIds, GatewayReconciliationIntent, GatewayReconciliationIntentDelivery,
    GatewayReconciliationJournalSession, GatewayReconciliationOutcome,
    GatewayReconciliationOutcomeError, GatewayReconciliationProgress, GatewayReconciliationRequest,
    GatewayStartup, GatewayStartupEvents, GatewayStartupPhase, GatewayStopRequest,
    GatewayStopSession, LoginShellError, LoginShellPath, MonotonicClock, ReconciledGateway,
    StartupStep, SystemMonotonicClock,
};
pub use service::{Gateway, GatewayRuntimeDependencies};
