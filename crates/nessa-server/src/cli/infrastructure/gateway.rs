//! CLI command interpretation over the shared bounded local product transport.
use super::surface_call_budget;
use crate::cli::application::{BrowserToken, CliError, Gateway, GatewayIdentity, TokenRequest};
use nessa_auth::application::dto::CredentialMetadataDto;
use nessa_client_core::local_product::{
    ExchangeError, LocalProduct, LocalProductError, ProductCaller,
};
use nessa_protocol::product::generated::VaultErrorCode;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    time::Duration,
};
pub struct LocalGateway {
    client: Option<LocalProduct>,
    identity: GatewayIdentity,
    address: SocketAddr,
    credential: PathBuf,
    caller: ProductCaller,
    wait: Duration,
}
impl LocalGateway {
    pub fn connect(address: SocketAddr, credential_file: &Path) -> Result<Self, CliError> {
        Self::connect_as(address, credential_file, ProductCaller::CLI)
    }
    /// [`connect`] as `caller`. The credential file is whose grants Cedar sees.
    pub fn connect_as(
        address: SocketAddr,
        credential_file: &Path,
        caller: ProductCaller,
    ) -> Result<Self, CliError> {
        Self::open(
            address,
            credential_file.to_path_buf(),
            caller,
            surface_call_budget(),
        )
    }
    /// `wait` is how long one surface call may run before its outcome is unknown.
    pub fn open(
        address: SocketAddr,
        credential: PathBuf,
        caller: ProductCaller,
        wait: Duration,
    ) -> Result<Self, CliError> {
        let mut gateway = Self {
            client: None,
            identity: GatewayIdentity {
                gateway_id: String::new(),
                organization_id: String::new(),
                principal_id: String::new(),
                expires_at: None,
            },
            address,
            credential,
            caller,
            wait,
        };
        gateway.live()?;
        Ok(gateway)
    }
    fn live(&mut self) -> Result<&mut LocalProduct, CliError> {
        if self.client.is_none() {
            let client = LocalProduct::connect_as(self.address, &self.credential, self.caller)
                .map_err(problem)?;
            let found = client.identity();
            self.identity = GatewayIdentity {
                gateway_id: found.gateway_id.clone(),
                organization_id: found.organization_id.clone(),
                principal_id: found.principal_id.clone(),
                expires_at: found.expires_at,
            };
            self.client = Some(client);
        }
        self.client.as_mut().ok_or(CliError::Transport)
    }
    fn retire_if_unknown(&mut self) {
        if self.client.as_ref().is_some_and(LocalProduct::retired) {
            self.client = None;
        }
    }
    fn request<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T, CliError> {
        let result = self.live()?.request(method, params).map_err(problem);
        self.retire_if_unknown();
        result
    }
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value).map_err(|_| CliError::Protocol)
}
fn problem(error: LocalProductError) -> CliError {
    match error {
        LocalProductError::NonLocal => CliError::NonLocal,
        LocalProductError::CredentialUnavailable => CliError::CredentialUnavailable,
        LocalProductError::InvalidCredential => CliError::InvalidCredential,
        LocalProductError::Unreachable => CliError::Unreachable,
        LocalProductError::Transport => CliError::Transport,
        LocalProductError::TimedOut => CliError::TimedOut,
        LocalProductError::Protocol => CliError::Protocol,
        LocalProductError::Correlation => CliError::Correlation,
        LocalProductError::Unauthorized => CliError::Unauthorized,
        LocalProductError::Denied => CliError::Denied,
        LocalProductError::Capacity => CliError::Capacity,
        LocalProductError::Vault(VaultErrorCode::VaultChangeNotConfirmed) => {
            CliError::VaultChangeNotConfirmed
        }
        LocalProductError::Rejected | LocalProductError::Vault(_) => CliError::Rejected,
    }
}
impl Gateway for LocalGateway {
    fn identity(&self) -> &GatewayIdentity {
        &self.identity
    }
    fn health(&mut self) -> Result<(), CliError> {
        let health: Value = self.request("server.health", json!({}))?;
        if health["ok"] != true {
            return Err(CliError::Unhealthy);
        }
        Ok(())
    }
    fn call(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<Value, crate::cli::application::GatewayCallError> {
        let wait = self.wait;
        let outcome = {
            let client = self
                .live()
                .map_err(crate::cli::application::GatewayCallError::Transport)?;
            client.exchange(method, params, wait)
        };
        self.retire_if_unknown();
        outcome.map_err(|error| match error {
            ExchangeError::Refused { code } => {
                crate::cli::application::GatewayCallError::Refused { code }
            }
            ExchangeError::Session(error) => {
                crate::cli::application::GatewayCallError::Transport(problem(error))
            }
        })
    }
    fn issue(&mut self, request: TokenRequest) -> Result<BrowserToken, CliError> {
        let result: Value = self.request("credential.issue", json!({
            "requestId":request.request_id,
            "principal":{"id":request.principal_id,"kind":"integration"},
            "membership":{"id":request.membership_id,"principalId":request.principal_id,"organizationId":request.organization_id,"role":"member","state":"active"},
            "expiresAt":request.expires_at,
            "grants":[
                {"action":"server.read","resource":{"organizationId":request.organization_id,"id":request.gateway_id}},
                {"action":"conversation.write","resource":{"organizationId":request.organization_id,"id":request.gateway_id}}
            ]
        }))?;
        let secret = result["secret"]
            .as_str()
            .filter(|s| !s.is_empty())
            .ok_or(CliError::SecretUnavailable)?;
        let metadata: CredentialMetadataDto = decode(result["credential"].clone())?;
        if metadata.id.is_empty()
            || metadata.principal_id != request.principal_id
            || metadata.organization_id != request.organization_id
            || metadata.audience_id != request.gateway_id
            || metadata.expires_at != request.expires_at
            || metadata.revoked_at.is_some()
            || metadata.grants.len() != 2
            || ["server.read", "conversation.write"].iter().any(|action| {
                !metadata.grants.iter().any(|grant| {
                    grant.action == *action
                        && grant.resource.id == request.gateway_id
                        && grant.resource.organization_id == request.organization_id
                })
            })
        {
            return Err(CliError::ScopeMismatch);
        }
        let id = metadata.id;
        Ok(BrowserToken {
            credential_id: id,
            secret: secret.into(),
        })
    }
}
