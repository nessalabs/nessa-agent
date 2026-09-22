//! Retryable desktop gateway reconciliation; native effects enter through the owned port.
mod ports;
mod service;
pub use crate::gateway::domain::value_objects::{
    ReconciliationNativeDecision, ReconciliationNativeEffect,
};
#[cfg(test)]
pub(crate) use ports::testing;
pub use ports::{
    GatewayError, GatewayHost, GatewayReconciliationAttempt, GatewayReconciliationAudit,
    GatewayReconciliationEffect, GatewayReconciliationIds, GatewayReconciliationIntent,
    GatewayReconciliationOutcome, GatewayReconciliationProgress, GatewayReconciliationRequest,
    GatewayStartup, GatewayStartupEvents, GatewayStartupPhase, LoginShellError, LoginShellPath,
    ReconciledGateway,
};
pub use service::Gateway;
