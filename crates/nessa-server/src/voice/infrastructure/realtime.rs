//! Minting realtime calls over HTTPS: OpenAI's client secrets, or the same
//! request to a scripted provider on this machine named by
//! `NESSA_VOICE_REALTIME` (the stress check's fake).
//!
//! What an answer means is [`grant`]'s, a pure function tested on OpenAI's
//! documented shape; the client only carries bytes. A redirect is a status,
//! never followed. The key goes only in the request's Authorization header.
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};

use crate::agent_install::infrastructure::https_archives::install_tls_backend;
use crate::voice::application::{MintFailure, RealtimeProvider};
use crate::voice::domain::{
    CallGrant, CallTransport, VoiceKey, INSTRUCTIONS, MODEL, SECRET_SECONDS, TOOLS,
    TRANSCRIPTION_MODEL, TRANSCRIPTION_PROMPT, VOICE,
};

const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);
const CALL_TIMEOUT: Duration = Duration::from_secs(20);
/// A client secret answer echoes the session: a few kilobytes.
const BODY_LIMIT: usize = 262_144;
/// The longest secret the wire carries (`VoiceCall.clientSecret`).
const MAX_SECRET_BYTES: usize = 4096;
const MAX_URL_BYTES: usize = 2048;

/// Where calls are minted and connected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RealtimeEndpoint {
    /// Scheme and host the client-secret request goes to, with no trailing `/`.
    api_base: String,
    transport: CallTransport,
    call_url: String,
}

/// Why a provider file was not used.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum EndpointProblem {
    Unreadable,
    /// An address that is not this machine's loopback.
    NotAllowed(String),
}

impl RealtimeEndpoint {
    /// OpenAI's realtime calls over WebRTC.
    pub fn openai() -> Self {
        Self {
            api_base: "https://api.openai.com".into(),
            transport: CallTransport::WebRtc,
            call_url: "https://api.openai.com/v1/realtime/calls".into(),
        }
    }

    /// A provider file: `{"apiBase", "transport", "callUrl"}`, both addresses
    /// on this machine's loopback, so the file can point a gateway at a fake
    /// and nowhere else.
    pub fn parse(bytes: &[u8]) -> Result<Self, EndpointProblem> {
        let value: Value =
            serde_json::from_slice(bytes).map_err(|_| EndpointProblem::Unreadable)?;
        let text = |name: &str| {
            value
                .get(name)
                .and_then(Value::as_str)
                .filter(|text| !text.is_empty() && text.len() <= MAX_URL_BYTES)
                .map(str::to_owned)
                .ok_or(EndpointProblem::Unreadable)
        };
        let api_base = text("apiBase")?.trim_end_matches('/').to_owned();
        let call_url = text("callUrl")?;
        let transport = match value.get("transport").and_then(Value::as_str) {
            Some("webrtc") => CallTransport::WebRtc,
            Some("scripted") => CallTransport::Scripted,
            _ => return Err(EndpointProblem::Unreadable),
        };
        for address in [&api_base, &call_url] {
            if !allowed(address) {
                return Err(EndpointProblem::NotAllowed(address.clone()));
            }
        }
        Ok(Self {
            api_base,
            transport,
            call_url,
        })
    }
}

/// A provider file points only at this machine, by its loopback address:
/// the saved key goes to `apiBase` as a bearer token, so whatever can set
/// the gateway's environment must not be able to send it anywhere else.
/// OpenAI itself is [`RealtimeEndpoint::openai`], never a file.
fn allowed(address: &str) -> bool {
    let Ok(url) = url::Url::parse(address) else {
        return false;
    };
    matches!(url.scheme(), "http" | "https" | "ws" | "wss")
        && match url.host() {
            Some(url::Host::Ipv4(ip)) => ip.is_loopback(),
            Some(url::Host::Ipv6(ip)) => ip.is_loopback(),
            _ => false,
        }
}

pub struct RealtimeHttps {
    endpoint: RealtimeEndpoint,
    client: reqwest::Client,
}

impl RealtimeHttps {
    /// `None` when no HTTP client can be built on this machine.
    pub fn new(endpoint: RealtimeEndpoint) -> Option<Self> {
        install_tls_backend();
        reqwest::Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .timeout(CALL_TIMEOUT)
            .redirect(reqwest::redirect::Policy::none())
            .user_agent("Nessa")
            .build()
            .ok()
            .map(|client| Self { endpoint, client })
    }
}

#[async_trait]
impl RealtimeProvider for RealtimeHttps {
    async fn mint(&self, key: &VoiceKey) -> Result<CallGrant, MintFailure> {
        let mut response = self
            .client
            .post(format!(
                "{}/v1/realtime/client_secrets",
                self.endpoint.api_base
            ))
            .bearer_auth(key.expose())
            .json(&client_secret_request())
            .send()
            .await
            .map_err(|_| MintFailure::Unreachable)?;
        let status = response.status().as_u16();
        let mut body = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| MintFailure::Unreachable)?
        {
            if body.len().saturating_add(chunk.len()) > BODY_LIMIT {
                return Err(MintFailure::Unreachable);
            }
            body.extend_from_slice(&chunk);
        }
        grant(&self.endpoint, status, &body)
    }
}

/// The client-secret request every call is minted with: the coordinator's
/// instructions, its two tools, tidied transcription, and the voice.
pub fn client_secret_request() -> Value {
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|(name, description)| {
            json!({
                "type": "function",
                "name": name,
                "description": description,
                "parameters": { "type": "object", "properties": {}, "additionalProperties": false },
            })
        })
        .collect();
    json!({
        "expires_after": { "anchor": "created_at", "seconds": SECRET_SECONDS },
        "session": {
            "type": "realtime",
            "model": MODEL,
            "instructions": INSTRUCTIONS,
            "tools": tools,
            "tool_choice": "auto",
            "audio": {
                "input": {
                    "transcription": { "model": TRANSCRIPTION_MODEL, "prompt": TRANSCRIPTION_PROMPT },
                    "turn_detection": { "type": "semantic_vad", "interrupt_response": true },
                },
                "output": { "voice": VOICE },
            },
        },
    })
}

/// What one client-secret answer means: a secret with when it expires, the
/// key refused (401, 403), or nothing usable.
pub fn grant(
    endpoint: &RealtimeEndpoint,
    status: u16,
    body: &[u8],
) -> Result<CallGrant, MintFailure> {
    match status {
        200..=299 => {}
        401 | 403 => return Err(MintFailure::Rejected),
        _ => return Err(MintFailure::Unreachable),
    }
    let value: Value = serde_json::from_slice(body).map_err(|_| MintFailure::Unreachable)?;
    let secret = value
        .get("value")
        .and_then(Value::as_str)
        .filter(|secret| !secret.is_empty() && secret.len() <= MAX_SECRET_BYTES)
        .ok_or(MintFailure::Unreachable)?;
    let expires_at_seconds = value
        .get("expires_at")
        .and_then(Value::as_u64)
        .ok_or(MintFailure::Unreachable)?;
    Ok(CallGrant {
        transport: endpoint.transport,
        call_url: endpoint.call_url.clone(),
        client_secret: secret.to_owned(),
        expires_at_ms: expires_at_seconds.saturating_mul(1000),
        model: MODEL.to_owned(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_call_is_minted_with_both_tools_and_tidied_transcription() {
        let body = client_secret_request();
        let names: Vec<&str> = body["session"]["tools"]
            .as_array()
            .unwrap()
            .iter()
            .map(|tool| tool["name"].as_str().unwrap())
            .collect();
        assert_eq!(names, ["send_to_agent", "stop_agent"]);
        assert_eq!(
            body["session"]["audio"]["input"]["transcription"]["model"],
            TRANSCRIPTION_MODEL
        );
        assert_eq!(body["expires_after"]["seconds"], SECRET_SECONDS);
    }

    #[test]
    fn an_openai_answer_is_a_secret_and_when_it_expires() {
        let body = br#"{"value":"ek_68af","expires_at":1756310470,"session":{"type":"realtime"}}"#;
        let grant = grant(&RealtimeEndpoint::openai(), 200, body).unwrap();
        assert_eq!(grant.client_secret, "ek_68af");
        assert_eq!(grant.expires_at_ms, 1_756_310_470_000);
        assert_eq!(grant.transport, CallTransport::WebRtc);
        assert_eq!(grant.call_url, "https://api.openai.com/v1/realtime/calls");
    }

    #[test]
    fn a_refused_key_and_everything_else_are_told_apart() {
        let openai = RealtimeEndpoint::openai();
        assert_eq!(grant(&openai, 401, b"{}"), Err(MintFailure::Rejected));
        assert_eq!(grant(&openai, 403, b"{}"), Err(MintFailure::Rejected));
        assert_eq!(grant(&openai, 500, b"{}"), Err(MintFailure::Unreachable));
        assert_eq!(grant(&openai, 302, b""), Err(MintFailure::Unreachable));
        assert_eq!(
            grant(&openai, 200, b"not json"),
            Err(MintFailure::Unreachable)
        );
        assert_eq!(
            grant(&openai, 200, br#"{"value":"","expires_at":1}"#),
            Err(MintFailure::Unreachable)
        );
        assert_eq!(
            grant(&openai, 200, br#"{"value":"ek"}"#),
            Err(MintFailure::Unreachable)
        );
    }

    #[test]
    fn a_provider_file_may_point_only_at_this_machine_unencrypted() {
        let local = br#"{"apiBase":"http://127.0.0.1:4100/","transport":"scripted","callUrl":"ws://127.0.0.1:4100/realtime"}"#;
        let endpoint = RealtimeEndpoint::parse(local).unwrap();
        assert_eq!(endpoint.api_base, "http://127.0.0.1:4100");
        assert_eq!(endpoint.transport, CallTransport::Scripted);
        let remote = br#"{"apiBase":"http://example.com","transport":"scripted","callUrl":"ws://127.0.0.1:1/r"}"#;
        assert_eq!(
            RealtimeEndpoint::parse(remote),
            Err(EndpointProblem::NotAllowed("http://example.com".into()))
        );
        let elsewhere = br#"{"apiBase":"https://api.example.com","transport":"webrtc","callUrl":"https://127.0.0.1:1/c"}"#;
        assert_eq!(
            RealtimeEndpoint::parse(elsewhere),
            Err(EndpointProblem::NotAllowed(
                "https://api.example.com".into()
            ))
        );
        let named = br#"{"apiBase":"http://localhost:1","transport":"scripted","callUrl":"ws://127.0.0.1:1/r"}"#;
        assert!(matches!(
            RealtimeEndpoint::parse(named),
            Err(EndpointProblem::NotAllowed(_))
        ));
        let unknown = br#"{"apiBase":"https://127.0.0.1:1","transport":"carrier-pigeon","callUrl":"wss://127.0.0.1:1"}"#;
        assert_eq!(
            RealtimeEndpoint::parse(unknown),
            Err(EndpointProblem::Unreadable)
        );
        assert_eq!(
            RealtimeEndpoint::parse(b"[]"),
            Err(EndpointProblem::Unreadable)
        );
    }
}
