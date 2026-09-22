use nessa_gateway_endpoint::application::EndpointDiscovery;
use std::sync::Arc;

/// One resolved service namespace and the discovery port that reads it.
pub struct GatewayEndpointAccess {
    stage: String,
    discovery: Arc<dyn EndpointDiscovery>,
}

impl GatewayEndpointAccess {
    pub fn new(stage: String, discovery: Arc<dyn EndpointDiscovery>) -> Self {
        Self { stage, discovery }
    }

    /// Return a health-correlated endpoint for the configured stage.
    pub fn resolve(&self, stage: &str) -> Result<Option<String>, String> {
        if stage != self.stage {
            return Err("Desktop and gateway stages must match".into());
        }
        self.discovery
            .discover()
            .map(|endpoint| endpoint.map(|value| value.web_socket_url()))
            .map_err(|error| error.to_string())
    }

    /// Require a credential request to name the endpoint selected for this attempt.
    pub fn permits_credential_for(&self, stage: &str, requested_url: &str) -> Result<(), String> {
        if let Some(endpoint) = self.resolve(stage)? {
            let expected = format!("{endpoint}/session");
            if requested_url != expected {
                return Err(
                    "The credential request does not match the verified gateway endpoint".into(),
                );
            }
        }
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/gateway_endpoint/application.rs"]
mod tests;
