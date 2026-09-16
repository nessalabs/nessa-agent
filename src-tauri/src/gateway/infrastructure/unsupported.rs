use crate::gateway::application::{GatewayError, GatewayHost};
use std::path::Path;
pub(super) struct Unsupported;
impl GatewayHost for Unsupported {
    fn register(&self, _: &Path, _: &str) -> Result<String, GatewayError> {
        Err(GatewayError::Registration(
            "Bundled gateway services currently require macOS".into(),
        ))
    }
    fn stop_agents(&self, _: &str) -> Result<(), GatewayError> {
        Err(GatewayError::Registration(
            "Bundled gateway services currently require macOS".into(),
        ))
    }
}
