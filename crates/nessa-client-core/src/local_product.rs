//! Bounded numeric-loopback product transport shared by native surfaces and CLI.
//! The connection lifetime owns server-side session resources; dropping it closes
//! the socket. Consumers own command interpretation and authorization remains at
//! the gateway. No protocol payload or server message enters diagnostics.
use nessa_protocol::product::generated::{ProductSessionReady, SessionChallenge};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    path::Path,
    time::{Duration, Instant},
};
use tungstenite::{client::client_with_config, protocol::WebSocketConfig, Message, WebSocket};
use zeroize::Zeroizing;

/// Local mode accepts numeric loopback only. Cloud transport is not implemented.
pub struct LocalProduct {
    socket: WebSocket<TcpStream>,
    identity: LocalProductIdentity,
    deadline: Instant,
    next_id: u64,
}
impl LocalProduct {
    /// Verified identity from this connection's handshake; permissions are rechecked by the server.
    pub fn identity(&self) -> &LocalProductIdentity {
        &self.identity
    }
    /// Connect and authenticate with a private credential file in at most ten seconds.
    /// Numeric loopback only; dropping the client closes its session.
    pub fn connect(address: SocketAddr, credential_file: &Path) -> Result<Self, LocalProductError> {
        if !address.ip().is_loopback() {
            return Err(LocalProductError::NonLocal);
        }
        let mut file =
            nessa_local_storage::open(credential_file, nessa_local_storage::OpenMode::Read)
                .map_err(|_| LocalProductError::CredentialUnavailable)?;
        let mut bytes = Zeroizing::new(Vec::new());
        file.by_ref()
            .take(16386)
            .read_to_end(&mut bytes)
            .map_err(|_| LocalProductError::CredentialUnavailable)?;
        let credential = std::str::from_utf8(&bytes)
            .map_err(|_| LocalProductError::InvalidCredential)?
            .trim();
        if credential.is_empty() || credential.len() > 16384 {
            return Err(LocalProductError::InvalidCredential);
        }
        let deadline = Instant::now() + Duration::from_secs(10);
        let stream = TcpStream::connect_timeout(&address, Duration::from_secs(3))
            .map_err(|_| LocalProductError::Unreachable)?;
        stream
            .set_read_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| LocalProductError::Transport)?;
        stream
            .set_write_timeout(Some(Duration::from_secs(5)))
            .map_err(|_| LocalProductError::Transport)?;
        let (socket, _) = client_with_config(
            format!("ws://{address}/session"),
            stream,
            Some(
                WebSocketConfig::default()
                    .max_message_size(Some(1_048_576))
                    .max_frame_size(Some(1_048_576)),
            ),
        )
        .map_err(|_| LocalProductError::Transport)?;
        let mut result = Self {
            socket,
            deadline,
            next_id: 0,
            identity: LocalProductIdentity {
                gateway_id: String::new(),
                organization_id: String::new(),
                principal_id: String::new(),
                expires_at: None,
            },
        };
        let challenge = result.read()?;
        if challenge["type"] != "event" || challenge["event"] != "session.challenge" {
            return Err(LocalProductError::Protocol);
        }
        let challenge: SessionChallenge = decode(challenge["payload"].clone())?;
        if challenge.min_version > 1 || challenge.max_version < 1 {
            return Err(LocalProductError::Protocol);
        }
        let ready: ProductSessionReady = result.request("session.authenticate", json!({"minVersion":1,"maxVersion":1,"nonce":challenge.nonce,"credential":credential,"client":{"id":"nessa-cli"},"surface":{"kind":"cli","instance":"nessa-cli"}}))?;
        if ready.version != 1
            || ready.gateway_id.is_empty()
            || ready.organization_id.is_empty()
            || ready.principal_id.is_empty()
            || ready
                .expires_at
                .is_some_and(|expiry| expiry > 9_007_199_254_740_991)
        {
            return Err(LocalProductError::Protocol);
        }
        result.identity = LocalProductIdentity {
            gateway_id: ready.gateway_id,
            organization_id: ready.organization_id,
            principal_id: ready.principal_id,
            expires_at: ready.expires_at,
        };
        Ok(result)
    }
    fn read(&mut self) -> Result<Value, LocalProductError> {
        loop {
            let remaining = self
                .deadline
                .checked_duration_since(Instant::now())
                .filter(|v| !v.is_zero())
                .ok_or(LocalProductError::TimedOut)?;
            self.socket
                .get_mut()
                .set_read_timeout(Some(remaining))
                .map_err(|_| LocalProductError::Transport)?;
            match self
                .socket
                .read()
                .map_err(|_| LocalProductError::Transport)?
            {
                Message::Text(text) if text.len() <= 1_048_576 => {
                    return serde_json::from_str(&text).map_err(|_| LocalProductError::Protocol)
                }
                Message::Ping(_) | Message::Pong(_) => continue,
                _ => return Err(LocalProductError::Protocol),
            }
        }
    }
    /// Send one correlated command and decode its bounded reply. Calls are serialized
    /// by the mutable borrow; no command replay occurs after an uncertain delivery.
    pub fn request<T: DeserializeOwned>(
        &mut self,
        method: &str,
        params: Value,
    ) -> Result<T, LocalProductError> {
        self.next_id += 1;
        let id = self.next_id.to_string();
        self.socket
            .send(Message::Text(
                json!({"type":"req","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .map_err(|_| LocalProductError::Transport)?;
        let response = self.read()?;
        if response["type"] != "res" || response["id"] != id {
            return Err(LocalProductError::Correlation);
        }
        if response["ok"] != true {
            // No arbitrary server message is allowed into diagnostics (it may contain secrets).
            return Err(match response["error"]["code"].as_str() {
                Some("unauthorized") => LocalProductError::Unauthorized,
                Some("forbidden") => LocalProductError::Denied,
                Some("credential_capacity") => LocalProductError::Capacity,
                _ => LocalProductError::Rejected,
            });
        }
        decode(response["payload"].clone())
    }
}
fn decode<T: DeserializeOwned>(value: Value) -> Result<T, LocalProductError> {
    serde_json::from_value(value).map_err(|_| LocalProductError::Protocol)
}

impl Drop for LocalProduct {
    fn drop(&mut self) {
        let _ = self.socket.close(None);
    }
}
/// Redacted transport failures. Server messages never become diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LocalProductError {
    /// The requested address is not numeric loopback.
    NonLocal,
    /// The private credential file cannot be read.
    CredentialUnavailable,
    /// The file does not contain a bounded credential.
    InvalidCredential,
    /// The listener could not be reached.
    Unreachable,
    /// The connection ended or delivery failed.
    Transport,
    /// The operation deadline expired.
    TimedOut,
    /// The reply does not satisfy the product contract.
    Protocol,
    /// The reply names another request.
    Correlation,
    /// Authentication was refused.
    Unauthorized,
    /// Current policy refused the operation.
    Denied,
    /// The credential registry is full.
    Capacity,
    /// The server refused with another redacted code.
    Rejected,
}
/// Verified handshake identity, without any cached permission decision.
#[derive(Clone, Debug)]
pub struct LocalProductIdentity {
    /// Gateway proven by the handshake.
    pub gateway_id: String,
    /// Organization of the authenticated gateway.
    pub organization_id: String,
    /// Authenticated principal.
    pub principal_id: String,
    /// Exclusive credential expiry in Unix seconds, when present.
    pub expires_at: Option<u64>,
}
