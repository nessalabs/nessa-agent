use crate::cli::application::{
    BrowserToken, CliError, Gateway, GatewayIdentity, TokenRequest, MAX_SAFE_TIMESTAMP,
};
use crate::product::{SessionChallenge, SessionReady};
use nessa_auth::application::dto::CredentialMetadataDto;
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    path::Path,
    time::{Duration, Instant},
};
use tungstenite::{client::client_with_config, protocol::WebSocketConfig, Message, WebSocket};

/// Local mode accepts numeric loopback only. Cloud transport is not implemented.
pub struct LocalGateway {
    socket: WebSocket<TcpStream>,
    identity: GatewayIdentity,
    deadline: Instant,
    next_id: u64,
}
impl LocalGateway {
    pub fn connect(address: SocketAddr, credential_file: &Path) -> Result<Self, CliError> {
        if !address.ip().is_loopback() {
            return Err(CliError::NonLocal);
        }
        let mut file =
            nessa_local_storage::open(credential_file, nessa_local_storage::OpenMode::Read)
                .map_err(|_| CliError::CredentialUnavailable)?;
        let mut bytes = Vec::new();
        file.by_ref()
            .take(16386)
            .read_to_end(&mut bytes)
            .map_err(|_| CliError::CredentialUnavailable)?;
        let credential = std::str::from_utf8(&bytes)
            .map_err(|_| CliError::InvalidCredential)?
            .trim();
        if credential.is_empty() || credential.len() > 16384 {
            return Err(CliError::InvalidCredential);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
            .map_err(|_| CliError::Unreachable)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| CliError::Transport)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| CliError::Transport)?;
        let (socket, _) = client_with_config(
            format!("ws://{address}/session"),
            stream,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(1_048_576))
                    .max_frame_size(Some(1_048_576)),
            ),
        )
        .map_err(|_| CliError::Transport)?;
        let mut result = Self {
            socket,
            deadline,
            next_id: 0,
            identity: GatewayIdentity {
                gateway_id: String::new(),
                organization_id: String::new(),
                principal_id: String::new(),
                expires_at: None,
            },
        };
        let challenge = result.read()?;
        if challenge["type"] != "event" || challenge["event"] != "session.challenge" {
            return Err(CliError::Protocol);
        }
        let challenge: SessionChallenge = decode(challenge["payload"].clone())?;
        if challenge.min_version > 1 || challenge.max_version < 1 {
            return Err(CliError::Protocol);
        }
        let ready: SessionReady = result.request("session.authenticate", json!({"minVersion":1,"maxVersion":1,"nonce":challenge.nonce,"credential":credential,"client":{"id":"nessa-cli"}}))?;
        if ready.version != 1
            || ready.gateway_id.is_empty()
            || ready.organization_id.is_empty()
            || ready.principal_id.is_empty()
            || ready
                .expires_at
                .is_some_and(|expiry| expiry > MAX_SAFE_TIMESTAMP)
        {
            return Err(CliError::Protocol);
        }
        result.identity = GatewayIdentity {
            gateway_id: ready.gateway_id,
            organization_id: ready.organization_id,
            principal_id: ready.principal_id,
            expires_at: ready.expires_at,
        };
        Ok(result)
    }
    fn read(&mut self) -> Result<Value, CliError> {
        loop {
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .filter(|v| !v.is_zero())
                .ok_or(CliError::TimedOut)?;
            self.socket
                .get_mut()
                .set_read_timeout(Some(remaining))
                .map_err(|_| CliError::Transport)?;
            match self.socket.read().map_err(|_| CliError::Transport)? {
                Message::Text(text) if text.len() <= 1_048_576 => {
                    return serde_json::from_str(&text).map_err(|_| CliError::Protocol)
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                _ => return Err(CliError::Protocol),
            }
        }
    }
    fn request<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T, CliError> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        self.socket
            .send(Message::Text(
                json!({"type":"req","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .map_err(|_| CliError::Transport)?;
        let response = self.read()?;
        if response["type"] != "res" || response["id"] != id {
            return Err(CliError::Correlation);
        }
        if response["ok"] != true {
            // No arbitrary server message is allowed into diagnostics (it may contain secrets).
            return Err(match response["error"]["code"].as_str() {
                Some("unauthorized") => CliError::Unauthorized,
                Some("forbidden") => CliError::Denied,
                Some("credential_capacity") => CliError::Capacity,
                _ => CliError::Rejected,
            });
        }
        decode(response["payload"].clone())
    }
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, CliError> {
    serde_json::from_value(value).map_err(|_| CliError::Protocol)
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
impl Drop for LocalGateway {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}
