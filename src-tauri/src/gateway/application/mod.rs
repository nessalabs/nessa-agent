//! Retryable desktop gateway reconciliation; native effects enter through the owned port.
mod ports;
mod service;
#[cfg(test)]
pub(crate) use ports::testing;
pub use ports::{
    GatewayError, GatewayHost, GatewayNativeEffect, GatewayReconciliationAttempt,
    GatewayReconciliationAudit, GatewayReconciliationEffect, GatewayReconciliationIds,
    GatewayReconciliationIntent, GatewayReconciliationOutcome, GatewayReconciliationProgress,
    GatewayReconciliationRequest, GatewayStartup, GatewayStartupEvents, GatewayStartupPhase,
    LoginShellError, LoginShellPath, ReconciledGateway,
};
pub use service::Gateway;
