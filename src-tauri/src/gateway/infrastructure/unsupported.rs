use crate::gateway::application::{GatewayError, GatewayHost, ReconciledGateway};
use crate::gateway::domain::value_objects::SearchPath;
use std::path::Path;
pub(super) struct Unsupported;
impl GatewayHost for Unsupported {
    fn register(
        &self,
        _: &Path,
        _: &str,
        _: Option<&SearchPath>,
    ) -> Result<ReconciledGateway, GatewayError> {
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
