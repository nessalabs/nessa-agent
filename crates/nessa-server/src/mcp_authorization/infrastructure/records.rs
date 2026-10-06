//! Non-secret authorization records on disk, and secret material in the
//! macOS keychain. Another platform has no secret writer: it does not write
//! a token to a file.
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use nessa_agent_credentials::CredentialNamespace;
use serde_json::{json, Value};
use uuid::Uuid;

use crate::mcp_authorization::application::{AuthorizationRecords, RecordFailure, TokenMaterial};
use crate::mcp_authorization::domain::{
    Attempt, Obligation, Phase, RefreshActivity, RevokeCause, ServerAuth, Settlement,
    TokenAvailability,
};

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
const SERVICE: &str = "nessa-mcp-oauth";

pub struct FileRecords {
    directory: PathBuf,
    namespace: CredentialNamespace,
}

impl FileRecords {
    pub fn new(directory: impl Into<PathBuf>, namespace: CredentialNamespace) -> Self {
        Self {
            directory: directory.into(),
            namespace,
        }
    }

    /// macOS can write a keychain item. Every other platform reports the
    /// writer missing, and authorize refuses before discovery.
    pub fn writer_available(&self) -> bool {
        cfg!(target_os = "macos")
    }

    fn path(&self, server: Uuid) -> PathBuf {
        self.directory.join(format!("{server}.json"))
    }
}

#[async_trait]
impl AuthorizationRecords for FileRecords {
    async fn load(&self, server: Uuid) -> Result<Option<ServerAuth>, RecordFailure> {
        let path = self.path(server);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(RecordFailure::Unavailable),
        };
        let value: Value =
            serde_json::from_slice(&bytes).map_err(|_| RecordFailure::Unavailable)?;
        from_json(&value)
            .ok_or(RecordFailure::Unavailable)
            .map(Some)
    }

    async fn store(&self, auth: &ServerAuth) -> Result<(), RecordFailure> {
        std::fs::create_dir_all(&self.directory).map_err(|_| RecordFailure::Unavailable)?;
        let path = self.path(auth.server);
        let body =
            serde_json::to_vec_pretty(&to_json(auth)).map_err(|_| RecordFailure::Unavailable)?;
        write_new(&path, &body)
    }

    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        read_secret(&self.namespace, server)
    }

    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<crate::mcp_authorization::domain::Publication, RecordFailure> {
        write_secret(&self.namespace, server, secret)
    }

    async fn delete_secret(
        &self,
        server: Uuid,
    ) -> Result<crate::mcp_authorization::domain::Deletion, RecordFailure> {
        delete_secret(&self.namespace, server)
    }
}

fn write_new(path: &Path, body: &[u8]) -> Result<(), RecordFailure> {
    let temporary = path.with_extension("json.tmp");
    std::fs::write(&temporary, body).map_err(|_| RecordFailure::Unavailable)?;
    std::fs::rename(&temporary, path).map_err(|_| RecordFailure::Unavailable)
}

fn to_json(auth: &ServerAuth) -> Value {
    json!({
        "server": auth.server.to_string(),
        "name": auth.name,
        "resource": auth.resource,
        "phase": phase_json(&auth.phase),
        "generation": auth.generation,
        "issuer": auth.issuer,
        "clientId": auth.client_id,
        "scopes": auth.scopes,
        "expiresAtMs": auth.expires_at_ms,
        "refreshDispatched": auth.refresh_dispatched,
        "dispatchFenced": auth.dispatch_fenced,
        "nextAttempt": auth.next_attempt,
        "pkceS256": auth.pkce_s256,
        "registrationOffered": auth.registration_offered,
        "revocationOffered": auth.revocation_offered,
        "tokenEndpoint": auth.token_endpoint,
        "revocationEndpoint": auth.revocation_endpoint,
        "authorizationEndpoint": auth.authorization_endpoint,
        "acknowledgedDomains": auth.acknowledged_domains,
        "attempt": auth.attempt.as_ref().map(|attempt| json!({
            "id": attempt.id,
            "state": attempt.state,
            "deadlineMs": attempt.deadline_ms,
            "consumed": attempt.consumed,
        })),
    })
}

fn phase_json(phase: &Phase) -> Value {
    match phase {
        Phase::Unauthenticated => json!({"kind": "unauthenticated"}),
        Phase::ConsentNeeded => json!({"kind": "consent_needed"}),
        Phase::Discovering { attempt } => json!({"kind": "discovering", "attempt": attempt}),
        Phase::PendingConsent { attempt } => json!({"kind": "pending_consent", "attempt": attempt}),
        Phase::Exchanging { attempt } => json!({"kind": "exchanging", "attempt": attempt}),
        Phase::Ready {
            availability,
            refresh,
            scope_required,
        } => json!({
            "kind": "ready",
            "availability": match availability {
                TokenAvailability::Usable => "usable",
                TokenAvailability::Unavailable => "unavailable",
            },
            "refresh": match refresh {
                RefreshActivity::Idle => "idle",
                RefreshActivity::Refreshing => "refreshing",
            },
            "scopeRequired": scope_required,
        }),
        Phase::AuthorizationIncomplete { obligation } => json!({
            "kind": "authorization_incomplete",
            "obligation": match obligation {
                Obligation::ExchangeUnknown => "exchange_unknown",
                Obligation::StoreUnknown => "store_unknown",
                Obligation::EvidencePending => "evidence_pending",
                Obligation::RefreshUnknown => "refresh_unknown",
            },
        }),
        Phase::Revoking { cause, settlement } => json!({
            "kind": "revoking",
            "cause": cause_name(*cause),
            "settlement": settlement_json(settlement),
        }),
        Phase::RevocationIncomplete { cause, settlement } => json!({
            "kind": "revocation_incomplete",
            "cause": cause_name(*cause),
            "settlement": settlement_json(settlement),
        }),
    }
}

fn settlement_json(settlement: &Settlement) -> Value {
    json!({
        "localDrained": settlement.local_drained,
        "secretDeleted": settlement.secret_deleted,
        "remote": settlement.remote.map(|observation| match observation {
            crate::mcp_authorization::domain::RemoteObservation::Acknowledged => "acknowledged",
            crate::mcp_authorization::domain::RemoteObservation::Unsupported => "unsupported",
            crate::mcp_authorization::domain::RemoteObservation::Unconfirmed => "unconfirmed",
        }),
        "evidenceAcked": settlement.evidence_acked,
    })
}

fn cause_name(cause: RevokeCause) -> &'static str {
    match cause {
        RevokeCause::Revoke => "revoke",
        RevokeCause::Removed => "removed",
        RevokeCause::ResourceChanged => "resource_changed",
        RevokeCause::InvalidGrant => "invalid_grant",
    }
}

fn from_json(value: &Value) -> Option<ServerAuth> {
    let server = Uuid::parse_str(value.get("server")?.as_str()?).ok()?;
    let mut auth = ServerAuth::consent_needed(
        server,
        value.get("name")?.as_str()?,
        value.get("resource")?.as_str()?,
    );
    auth.generation = value.get("generation")?.as_u64()?;
    auth.issuer = optional_string(value, "issuer");
    auth.client_id = optional_string(value, "clientId");
    auth.scopes = value
        .get("scopes")?
        .as_array()?
        .iter()
        .filter_map(|item| item.as_str().map(str::to_owned))
        .collect();
    auth.expires_at_ms = value.get("expiresAtMs").and_then(|item| item.as_u64());
    auth.refresh_dispatched = value.get("refreshDispatched")?.as_bool()?;
    auth.dispatch_fenced = value.get("dispatchFenced")?.as_bool()?;
    auth.next_attempt = value.get("nextAttempt")?.as_u64()?;
    auth.pkce_s256 = value.get("pkceS256")?.as_bool()?;
    auth.registration_offered = value.get("registrationOffered")?.as_bool()?;
    auth.revocation_offered = value.get("revocationOffered")?.as_bool()?;
    auth.token_endpoint = optional_string(value, "tokenEndpoint");
    auth.revocation_endpoint = optional_string(value, "revocationEndpoint");
    auth.authorization_endpoint = optional_string(value, "authorizationEndpoint");
    auth.acknowledged_domains = optional_string(value, "acknowledgedDomains");
    auth.attempt = value.get("attempt").and_then(|attempt| {
        Some(Attempt {
            id: attempt.get("id")?.as_u64()?,
            state: attempt.get("state")?.as_str()?.to_owned(),
            deadline_ms: attempt.get("deadlineMs")?.as_u64()?,
            consumed: attempt.get("consumed")?.as_bool()?,
        })
    });
    auth.phase = phase_of(value.get("phase")?)?;
    Some(auth)
}

fn optional_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(|item| item.as_str())
        .map(str::to_owned)
}

fn phase_of(value: &Value) -> Option<Phase> {
    match value.get("kind")?.as_str()? {
        "unauthenticated" => Some(Phase::Unauthenticated),
        "consent_needed" => Some(Phase::ConsentNeeded),
        "discovering" => Some(Phase::Discovering {
            attempt: value.get("attempt")?.as_u64()?,
        }),
        "pending_consent" => Some(Phase::PendingConsent {
            attempt: value.get("attempt")?.as_u64()?,
        }),
        "exchanging" => Some(Phase::Exchanging {
            attempt: value.get("attempt")?.as_u64()?,
        }),
        "ready" => Some(Phase::Ready {
            availability: match value.get("availability")?.as_str()? {
                "usable" => TokenAvailability::Usable,
                "unavailable" => TokenAvailability::Unavailable,
                _ => return None,
            },
            refresh: match value.get("refresh")?.as_str()? {
                "idle" => RefreshActivity::Idle,
                "refreshing" => RefreshActivity::Refreshing,
                _ => return None,
            },
            scope_required: value.get("scopeRequired")?.as_bool()?,
        }),
        "authorization_incomplete" => Some(Phase::AuthorizationIncomplete {
            obligation: match value.get("obligation")?.as_str()? {
                "exchange_unknown" => Obligation::ExchangeUnknown,
                "store_unknown" => Obligation::StoreUnknown,
                "evidence_pending" => Obligation::EvidencePending,
                "refresh_unknown" => Obligation::RefreshUnknown,
                _ => return None,
            },
        }),
        "revoking" => Some(Phase::Revoking {
            cause: cause_of(value.get("cause")?.as_str()?)?,
            settlement: settlement_of(value.get("settlement")?)?,
        }),
        "revocation_incomplete" => Some(Phase::RevocationIncomplete {
            cause: cause_of(value.get("cause")?.as_str()?)?,
            settlement: settlement_of(value.get("settlement")?)?,
        }),
        _ => None,
    }
}

fn cause_of(name: &str) -> Option<RevokeCause> {
    match name {
        "revoke" => Some(RevokeCause::Revoke),
        "removed" => Some(RevokeCause::Removed),
        "resource_changed" => Some(RevokeCause::ResourceChanged),
        "invalid_grant" => Some(RevokeCause::InvalidGrant),
        _ => None,
    }
}

fn settlement_of(value: &Value) -> Option<Settlement> {
    Some(Settlement {
        local_drained: value.get("localDrained")?.as_bool()?,
        secret_deleted: value.get("secretDeleted")?.as_bool()?,
        remote: match value.get("remote").and_then(|item| item.as_str()) {
            Some("acknowledged") => {
                Some(crate::mcp_authorization::domain::RemoteObservation::Acknowledged)
            }
            Some("unsupported") => {
                Some(crate::mcp_authorization::domain::RemoteObservation::Unsupported)
            }
            Some("unconfirmed") => {
                Some(crate::mcp_authorization::domain::RemoteObservation::Unconfirmed)
            }
            None => None,
            _ => return None,
        },
        evidence_acked: value.get("evidenceAcked")?.as_bool()?,
    })
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
fn account(namespace: &CredentialNamespace, server: Uuid) -> Result<String, RecordFailure> {
    namespace
        .account(&format!("mcp-{server}"))
        .map_err(|_| RecordFailure::Unavailable)
}

#[cfg(target_os = "macos")]
fn read_secret(
    namespace: &CredentialNamespace,
    server: Uuid,
) -> Result<Option<TokenMaterial>, RecordFailure> {
    let account = account(namespace, server)?;
    match security_framework::passwords::generic_password(SERVICE, &account) {
        Ok(bytes) => {
            let value: Value =
                serde_json::from_slice(&bytes).map_err(|_| RecordFailure::Unavailable)?;
            Some(TokenMaterial {
                access_token: value.get("access")?.as_str()?.to_owned(),
                refresh_token: value
                    .get("refresh")
                    .and_then(|item| item.as_str())
                    .map(str::to_owned),
                generation: value.get("generation")?.as_u64()?,
            })
            .map(Ok)
            .unwrap_or(Err(RecordFailure::Unavailable))
        }
        Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => Ok(None),
        Err(_) => Err(RecordFailure::Unavailable),
    }
}

#[cfg(target_os = "macos")]
fn write_secret(
    namespace: &CredentialNamespace,
    server: Uuid,
    secret: &TokenMaterial,
) -> Result<crate::mcp_authorization::domain::Publication, RecordFailure> {
    let account = account(namespace, server)?;
    let body = serde_json::to_vec(&json!({
        "access": secret.access_token,
        "refresh": secret.refresh_token,
        "generation": secret.generation,
    }))
    .map_err(|_| RecordFailure::Unavailable)?;
    let _ = security_framework::passwords::delete_generic_password(SERVICE, &account);
    security_framework::passwords::set_generic_password(SERVICE, &account, &body)
        .map(|_| crate::mcp_authorization::domain::Publication::Acknowledged)
        .map_err(|_| RecordFailure::Unavailable)
}

#[cfg(target_os = "macos")]
fn delete_secret(
    namespace: &CredentialNamespace,
    server: Uuid,
) -> Result<crate::mcp_authorization::domain::Deletion, RecordFailure> {
    let account = account(namespace, server)?;
    match security_framework::passwords::delete_generic_password(SERVICE, &account) {
        Ok(()) => Ok(crate::mcp_authorization::domain::Deletion::Deleted),
        Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => {
            Ok(crate::mcp_authorization::domain::Deletion::Deleted)
        }
        Err(_) => Ok(crate::mcp_authorization::domain::Deletion::Failed),
    }
}

#[cfg(not(target_os = "macos"))]
fn read_secret(
    _namespace: &CredentialNamespace,
    _server: Uuid,
) -> Result<Option<TokenMaterial>, RecordFailure> {
    Err(RecordFailure::Unavailable)
}

#[cfg(not(target_os = "macos"))]
fn write_secret(
    _namespace: &CredentialNamespace,
    _server: Uuid,
    _secret: &TokenMaterial,
) -> Result<crate::mcp_authorization::domain::Publication, RecordFailure> {
    Err(RecordFailure::Unavailable)
}

#[cfg(not(target_os = "macos"))]
fn delete_secret(
    _namespace: &CredentialNamespace,
    _server: Uuid,
) -> Result<crate::mcp_authorization::domain::Deletion, RecordFailure> {
    Ok(crate::mcp_authorization::domain::Deletion::Unknown)
}

/// The secret half of [`FileRecords`], named for composition.
pub type SecretStore = FileRecords;
