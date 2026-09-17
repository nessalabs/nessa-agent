use std::{error::Error, fmt, path::Path};

/// Exact native runtime incarnation established by successful reconciliation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReconciledGateway {
    service: String,
    runtime_fingerprint: String,
    runtime_instance: String,
    service_generation: String,
    process_id: u32,
}
// The identity itself is portable evidence carried by the `GatewayHost`
// contract on every target. Reading its parts is what one native adapter does,
// and macOS is the only host that manages a background service today.
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
impl ReconciledGateway {
    pub fn new(
        service: String,
        runtime_fingerprint: String,
        runtime_instance: String,
        service_generation: String,
        process_id: u32,
    ) -> Self {
        Self {
            service,
            runtime_fingerprint,
            runtime_instance,
            service_generation,
            process_id,
        }
    }
    pub fn service(&self) -> &str {
        &self.service
    }
    pub fn runtime_fingerprint(&self) -> &str {
        &self.runtime_fingerprint
    }
    pub fn runtime_instance(&self) -> &str {
        &self.runtime_instance
    }
    pub fn service_generation(&self) -> &str {
        &self.service_generation
    }
    pub fn process_id(&self) -> u32 {
        self.process_id
    }
}

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
/// Reconciliation returns the exact native runtime incarnation only after matching readiness. A stop
/// acknowledges request delivery, not the eventual physical cleanup of each agent.
pub trait GatewayHost: Send + Sync {
    fn register(&self, runtime: &Path, stage: &str) -> Result<ReconciledGateway, GatewayError>;
    fn stop_agents(&self, gateway: &ReconciledGateway) -> Result<(), GatewayError>;
}
