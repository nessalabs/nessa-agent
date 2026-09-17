use crate::gateway::application::{GatewayError, GatewayHost, ReconciledGateway};
use std::path::Path;
pub(super) struct Unsupported;
impl GatewayHost for Unsupported {
    fn register(&self, _: &Path, _: &str) -> Result<ReconciledGateway, GatewayError> {
        Err(GatewayError::Registration(
            "Bundled gateway services currently require macOS".into(),
        ))
    }
    fn stop_agents(&self, _: &ReconciledGateway) -> Result<(), GatewayError> {
        Err(GatewayError::Stop(
            "Bundled gateway services currently require macOS".into(),
        ))
    }
}
