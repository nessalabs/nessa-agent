use std::{error::Error, fmt, path::Path};
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GatewayError {
    Registration(String),
    NotReconciled,
    Stop(String),
}
impl fmt::Display for GatewayError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Registration(message) | Self::Stop(message) => f.write_str(message),
            Self::NotReconciled => f.write_str("gateway service has not reconciled"),
        }
    }
}
impl Error for GatewayError {}
/// Reconciliation returns the native service identity only after matching readiness. A stop acknowledges the
/// request delivery, not the eventual physical cleanup of each agent.
pub trait GatewayHost: Send + Sync {
    fn register(&self, runtime: &Path, stage: &str) -> Result<String, GatewayError>;
    fn stop_agents(&self, service: &str) -> Result<(), GatewayError>;
}
