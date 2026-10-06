//! Non-secret authorization records on disk, and secret material in a sealed
//! file beside them. The same store runs on macOS, Linux, and Windows: a
//! local key file and one sealed blob per server, both created with
//! user-only permissions. A token is not written into the non-secret record.
use std::io::{Read, Write};
use std::path::{Path, PathBuf};

use async_trait::async_trait;
use hmac::{Hmac, Mac};
use nessa_local_storage::{open, OpenMode, PrivateTempFile};
use serde_json::{json, Value};
use sha2::Sha256;
use subtle::ConstantTimeEq;
use uuid::Uuid;

use crate::mcp_authorization::application::{AuthorizationRecords, RecordFailure, TokenMaterial};
use crate::mcp_authorization::domain::{
    Attempt, Obligation, Phase, RefreshActivity, RevokeCause, ServerAuth, Settlement,
    TokenAvailability,
};

type HmacSha256 = Hmac<Sha256>;

const KEY_LABEL_ENC: &[u8] = b"nessa-mcp-oauth-enc";
const KEY_LABEL_MAC: &[u8] = b"nessa-mcp-oauth-mac";

pub struct FileRecords {
    directory: PathBuf,
}

impl FileRecords {
    pub fn new(directory: impl Into<PathBuf>) -> Self {
        Self {
            directory: directory.into(),
        }
    }

    /// The sealed file store is the writer on every OS. A later write that
    /// cannot create the directory still fails as unavailable.
    pub fn writer_available(&self) -> bool {
        true
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
        nessa_local_storage::create_directory(&self.directory)
            .map_err(|_| RecordFailure::Unavailable)?;
        let path = self.path(auth.server);
        let body =
            serde_json::to_vec_pretty(&to_json(auth)).map_err(|_| RecordFailure::Unavailable)?;
        write_new(&path, &body)
    }

    async fn load_secret(&self, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
        read_secret(&self.directory, server)
    }

    async fn store_secret(
        &self,
        server: Uuid,
        secret: &TokenMaterial,
    ) -> Result<crate::mcp_authorization::domain::Publication, RecordFailure> {
        write_secret(&self.directory, server, secret)
    }

    async fn delete_secret(
        &self,
        server: Uuid,
    ) -> Result<crate::mcp_authorization::domain::Deletion, RecordFailure> {
        delete_secret(&self.directory, server)
    }
}

fn write_new(path: &Path, body: &[u8]) -> Result<(), RecordFailure> {
    let directory = path.parent().ok_or(RecordFailure::Unavailable)?;
    write_private(directory, path, body)?;
    nessa_local_storage::sync_directory(directory).map_err(|_| RecordFailure::Unavailable)
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

fn secrets_dir(directory: &Path) -> PathBuf {
    directory.join("secrets")
}

fn seal_path(directory: &Path, server: Uuid) -> PathBuf {
    secrets_dir(directory).join(format!("{server}.seal"))
}

fn ensure_secrets(directory: &Path) -> Result<PathBuf, RecordFailure> {
    let secrets = secrets_dir(directory);
    nessa_local_storage::create_directory(&secrets).map_err(|_| RecordFailure::Unavailable)?;
    Ok(secrets)
}

fn read_secret(directory: &Path, server: Uuid) -> Result<Option<TokenMaterial>, RecordFailure> {
    let path = seal_path(directory, server);
    let sealed = match read_private(&path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(_) => return Err(RecordFailure::Unavailable),
    };
    let key = match read_private(&secrets_dir(directory).join("key")) {
        Ok(bytes) => bytes,
        Err(_) => return Err(RecordFailure::Unavailable),
    };
    let plain = unseal(&key, &sealed).ok_or(RecordFailure::Unavailable)?;
    material_of(&plain)
}

fn write_secret(
    directory: &Path,
    server: Uuid,
    secret: &TokenMaterial,
) -> Result<crate::mcp_authorization::domain::Publication, RecordFailure> {
    let secrets = ensure_secrets(directory)?;
    let key = load_or_create_key(&secrets)?;
    let plain = serde_json::to_vec(&json!({
        "access": secret.access_token,
        "refresh": secret.refresh_token,
        "generation": secret.generation,
    }))
    .map_err(|_| RecordFailure::Unavailable)?;
    let sealed = seal(&key, &plain).map_err(|_| RecordFailure::Unavailable)?;
    write_private(&secrets, &seal_path(directory, server), &sealed)?;
    Ok(crate::mcp_authorization::domain::Publication::Acknowledged)
}

fn delete_secret(
    directory: &Path,
    server: Uuid,
) -> Result<crate::mcp_authorization::domain::Deletion, RecordFailure> {
    match std::fs::remove_file(seal_path(directory, server)) {
        Ok(()) => Ok(crate::mcp_authorization::domain::Deletion::Deleted),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            Ok(crate::mcp_authorization::domain::Deletion::Deleted)
        }
        Err(_) => Ok(crate::mcp_authorization::domain::Deletion::Failed),
    }
}

fn load_or_create_key(secrets: &Path) -> Result<Vec<u8>, RecordFailure> {
    let path = secrets.join("key");
    if let Ok(bytes) = read_private(&path) {
        if bytes.len() == 32 {
            return Ok(bytes);
        }
        return Err(RecordFailure::Unavailable);
    }
    let mut key = vec![0; 32];
    getrandom::fill(&mut key).map_err(|_| RecordFailure::Unavailable)?;
    match publish_exclusive(secrets, &path, &key) {
        Ok(()) => Ok(key),
        Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => read_private(&path)
            .ok()
            .filter(|bytes| bytes.len() == 32)
            .ok_or(RecordFailure::Unavailable),
        Err(_) => Err(RecordFailure::Unavailable),
    }
}

fn publish_exclusive(directory: &Path, path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let mut file = PrivateTempFile::new_in(directory)?;
    file.as_file_mut().write_all(bytes)?;
    file.as_file().sync_all()?;
    file.publish(path)
}

fn read_private(path: &Path) -> std::io::Result<Vec<u8>> {
    let mut file = open(path, OpenMode::ReadNonblocking)?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)?;
    Ok(bytes)
}

fn write_private(directory: &Path, path: &Path, bytes: &[u8]) -> Result<(), RecordFailure> {
    let mut file = PrivateTempFile::new_in(directory).map_err(|_| RecordFailure::Unavailable)?;
    file.as_file_mut()
        .write_all(bytes)
        .map_err(|_| RecordFailure::Unavailable)?;
    file.as_file()
        .sync_all()
        .map_err(|_| RecordFailure::Unavailable)?;
    file.persist(path).map_err(|_| RecordFailure::Unavailable)?;
    Ok(())
}

fn material_of(bytes: &[u8]) -> Result<Option<TokenMaterial>, RecordFailure> {
    let value: Value = serde_json::from_slice(bytes).map_err(|_| RecordFailure::Unavailable)?;
    let access = value
        .get("access")
        .and_then(Value::as_str)
        .ok_or(RecordFailure::Unavailable)?;
    let generation = value
        .get("generation")
        .and_then(Value::as_u64)
        .ok_or(RecordFailure::Unavailable)?;
    Ok(Some(TokenMaterial {
        access_token: access.to_owned(),
        refresh_token: value
            .get("refresh")
            .and_then(Value::as_str)
            .map(str::to_owned),
        generation,
    }))
}

/// `nonce || ciphertext || mac`. The mac covers the nonce and ciphertext.
fn seal(key: &[u8], plain: &[u8]) -> Result<Vec<u8>, RecordFailure> {
    let mut nonce = [0_u8; 16];
    getrandom::fill(&mut nonce).map_err(|_| RecordFailure::Unavailable)?;
    let mut body = plain.to_vec();
    apply_keystream(&derive(key, KEY_LABEL_ENC), &nonce, &mut body);
    let mut sealed = Vec::with_capacity(16 + body.len() + 32);
    sealed.extend_from_slice(&nonce);
    sealed.extend_from_slice(&body);
    sealed.extend_from_slice(&tag(&derive(key, KEY_LABEL_MAC), &nonce, &body));
    Ok(sealed)
}

fn unseal(key: &[u8], sealed: &[u8]) -> Option<Vec<u8>> {
    if sealed.len() < 16 + 32 {
        return None;
    }
    let (nonce, rest) = sealed.split_at(16);
    let (body, mac) = rest.split_at(rest.len() - 32);
    let expected = tag(&derive(key, KEY_LABEL_MAC), nonce, body);
    if !bool::from(mac.ct_eq(expected.as_slice())) {
        return None;
    }
    let mut plain = body.to_vec();
    apply_keystream(&derive(key, KEY_LABEL_ENC), nonce, &mut plain);
    Some(plain)
}

fn derive(key: &[u8], label: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(key).expect("hmac accepts this key length");
    mac.update(label);
    mac.finalize().into_bytes().into()
}

fn tag(mac_key: &[u8], nonce: &[u8], body: &[u8]) -> [u8; 32] {
    let mut mac = HmacSha256::new_from_slice(mac_key).expect("hmac accepts this key length");
    mac.update(nonce);
    mac.update(body);
    mac.finalize().into_bytes().into()
}

fn apply_keystream(enc_key: &[u8], nonce: &[u8], data: &mut [u8]) {
    let mut offset = 0;
    let mut counter = 0_u32;
    while offset < data.len() {
        let mut mac = HmacSha256::new_from_slice(enc_key).expect("hmac accepts this key length");
        mac.update(nonce);
        mac.update(&counter.to_be_bytes());
        let block = mac.finalize().into_bytes();
        let take = (data.len() - offset).min(block.len());
        for (slot, byte) in data[offset..offset + take].iter_mut().zip(block) {
            *slot ^= byte;
        }
        offset += take;
        counter = counter.saturating_add(1);
    }
}

/// The secret half of [`FileRecords`], named for composition.
pub type SecretStore = FileRecords;

#[cfg(test)]
mod tests {
    use std::io::{Read, Seek, SeekFrom, Write};

    use uuid::Uuid;

    use super::FileRecords;
    use crate::mcp_authorization::application::{
        AuthorizationRecords, RecordFailure, TokenMaterial,
    };
    use crate::mcp_authorization::domain::{Deletion, Publication, ServerAuth};

    fn secret() -> TokenMaterial {
        TokenMaterial {
            access_token: "sekret-token".into(),
            refresh_token: Some("refresh-sekret".into()),
            generation: 3,
        }
    }

    fn records() -> (FileRecords, std::path::PathBuf) {
        let directory = std::env::temp_dir().join(format!("nessa-mcp-auth-{}", Uuid::new_v4()));
        (FileRecords::new(&directory), directory)
    }

    fn contains_token(directory: &std::path::Path, token: &str) -> bool {
        let mut pending = vec![directory.to_path_buf()];
        while let Some(path) = pending.pop() {
            let Ok(entries) = std::fs::read_dir(&path) else {
                continue;
            };
            for entry in entries.flatten() {
                let path = entry.path();
                if path.is_dir() {
                    pending.push(path);
                    continue;
                }
                let Ok(bytes) = std::fs::read(&path) else {
                    continue;
                };
                if bytes
                    .windows(token.len())
                    .any(|window| window == token.as_bytes())
                {
                    return true;
                }
            }
        }
        false
    }

    #[tokio::test]
    async fn a_sealed_file_round_trips_and_never_stores_the_token_in_the_clear() {
        let (records, directory) = records();
        assert!(records.writer_available());
        let server = Uuid::new_v4();
        let secret = secret();
        assert_eq!(
            records.store_secret(server, &secret).await,
            Ok(Publication::Acknowledged)
        );
        let loaded = records.load_secret(server).await.unwrap().unwrap();
        assert_eq!(loaded.access_token, secret.access_token);
        assert_eq!(loaded.refresh_token, secret.refresh_token);
        assert_eq!(loaded.generation, secret.generation);
        assert!(!contains_token(&directory, "sekret-token"));
        assert!(!contains_token(&directory, "refresh-sekret"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            for name in ["key", &format!("{server}.seal")] {
                let mode = std::fs::metadata(directory.join("secrets").join(name))
                    .unwrap()
                    .permissions()
                    .mode();
                assert_eq!(mode & 0o077, 0, "{name} is not user-only: {mode:o}");
            }
        }
        let facts = ServerAuth::consent_needed(server, "docs", "https://mcp.example/mcp");
        records.store(&facts).await.unwrap();
        let record_path = directory.join(format!("{server}.json"));
        let record = std::fs::read(&record_path).unwrap();
        assert!(!record
            .windows(b"sekret-token".len())
            .any(|window| window == b"sekret-token"));
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&record_path)
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "the record is not user-only: {mode:o}");
        }
        assert_eq!(records.delete_secret(server).await, Ok(Deletion::Deleted));
        assert_eq!(records.load_secret(server).await, Ok(None));
        let _ = std::fs::remove_dir_all(&directory);
    }

    #[tokio::test]
    async fn a_tampered_seal_is_unavailable_and_not_a_token() {
        let (records, directory) = records();
        let server = Uuid::new_v4();
        records.store_secret(server, &secret()).await.unwrap();
        let path = directory.join("secrets").join(format!("{server}.seal"));
        let mut file =
            nessa_local_storage::open(&path, nessa_local_storage::OpenMode::ReadWrite).unwrap();
        let mut byte = [0_u8; 1];
        file.read_exact(&mut byte).unwrap();
        byte[0] ^= 0xff;
        file.seek(SeekFrom::Start(0)).unwrap();
        file.write_all(&byte).unwrap();
        assert_eq!(
            records.load_secret(server).await,
            Err(RecordFailure::Unavailable)
        );
        let _ = std::fs::remove_dir_all(&directory);
    }
}
