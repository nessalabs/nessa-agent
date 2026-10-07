//! Maps an admitted token onto the MCP transport's authorization port.
//! The owner decides; this adapter names that decision in the SDK's error
//! type and builds the bearer one request carries.
use std::sync::Arc;

use async_trait::async_trait;
use nessa_sdk::infrastructure::mcp::{Bearer, McpError, RemoteAuthorization};
use uuid::Uuid;

use crate::mcp_authorization::application::{AdmissionRefusal, AdmittedToken, AuthorizationOwner};

/// The transport's view of [`AuthorizationOwner`].
pub struct TransportAuthorization {
    owner: Arc<AuthorizationOwner>,
}

impl TransportAuthorization {
    pub fn new(owner: Arc<AuthorizationOwner>) -> Self {
        Self { owner }
    }
}

#[async_trait]
impl RemoteAuthorization for TransportAuthorization {
    async fn bearer(&self, server: Uuid) -> Result<Option<Bearer>, McpError> {
        issued(self.owner.bearer(server).await)
    }

    async fn rejected(
        &self,
        server: Uuid,
        www_authenticate: &str,
    ) -> Result<Option<Bearer>, McpError> {
        issued(self.owner.rejected(server, www_authenticate).await)
    }

    async fn insufficient_scope(&self, server: Uuid, www_authenticate: &str) {
        self.owner
            .insufficient_scope(server, www_authenticate)
            .await;
    }
}

fn issued(
    result: Result<Option<AdmittedToken>, AdmissionRefusal>,
) -> Result<Option<Bearer>, McpError> {
    match result {
        Ok(None) => Ok(None),
        Ok(Some(secret)) => Ok(Some(Bearer::new(secret.access_token, secret.generation))),
        Err(AdmissionRefusal::Unauthorized) => Err(McpError::Unauthorized),
        Err(AdmissionRefusal::InsufficientScope) => Err(McpError::InsufficientScope),
        Err(AdmissionRefusal::Unreachable) => Err(McpError::Unreachable),
    }
}
