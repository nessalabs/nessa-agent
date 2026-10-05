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
    pub fn resolve(&self, stage: &str) -> Result<Option<String>, EndpointError> {
        if stage != self.stage {
            return Err(EndpointError::WrongStage {
                bundle: self.stage.clone(),
                requested: stage.to_owned(),
            });
        }
        self.discovery
            .discover()
            .map(|endpoint| endpoint.map(|value| value.web_socket_url()))
            .map_err(|error| EndpointError::Unavailable(error.to_string()))
    }

    /// Require a credential request to name the endpoint selected for this attempt.
    pub fn permits_credential_for(
        &self,
        stage: &str,
        requested_url: &str,
    ) -> Result<(), EndpointError> {
        if let Some(endpoint) = self.resolve(stage)? {
            if requested_url != endpoint {
                return Err(EndpointError::DestinationMismatch);
            }
        }
        Ok(())
    }
}

/// Why an endpoint could not be resolved for the stage a window asked about.
///
/// `WrongStage` carries both stages. The page branches on that pair; this
/// type does not invent the sentence.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointError {
    WrongStage { bundle: String, requested: String },
    Unavailable(String),
    DestinationMismatch,
}

impl std::fmt::Display for EndpointError {
    fn fmt(&self, out: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::WrongStage { bundle, requested } => {
                write!(out, "window stage {bundle}, requested stage {requested}")
            }
            Self::Unavailable(message) => out.write_str(message),
            Self::DestinationMismatch => {
                out.write_str("The credential request does not match the verified gateway endpoint")
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/gateway_endpoint/application.rs"]
mod tests;
