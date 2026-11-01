//! Paired devices' tunnel secrets, on disk.
//!
//! `<namespace>/tunnel/device-secrets.json` holds each active secret.
//! `<conversations>/audit/tunnel-device-secrets/` holds one private record
//! per issue and per revocation: the credential, a fingerprint of the
//! secret, the cause, and who did it. The secret itself is not in the audit
//! and not in a log.
//!
//! A secret admits a tunnel connection, and a relay lookup, only while its
//! credential is still an active device credential. Revoking the credential
//! or unpairing the device removes the secret.
use super::super::application::{
    any_mac, any_match, DeviceTunnelSecrets, SecretError, SecretRevocation,
};
use base64::Engine;
use nessa_auth::application::ports::Clock;
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use nessa_protocol::tunnel::TunnelSecret;
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::{
    fs,
    io::Write,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
};

const SECRETS_FILE: &str = "device-secrets.json";

/// Which credentials still hold an active device credential.
pub(crate) trait ActiveDeviceCredentials: Send + Sync {
    /// Their ids, or an error when the registry cannot be read.
    fn ids(&self) -> Result<Vec<String>, SecretError>;
}

struct Row {
    credential_id: String,
    device_key: [u8; 32],
    secret: [u8; 32],
}

/// The file store composed into the gateway.
pub(crate) struct FileDeviceSecrets {
    secrets: PathBuf,
    audit: PathBuf,
    clock: Arc<dyn Clock>,
    active: Arc<dyn ActiveDeviceCredentials>,
    gate: Mutex<()>,
}

impl FileDeviceSecrets {
    /// The store for `namespace`: secrets under `tunnel/`, audit under
    /// `conversations/audit/tunnel-device-secrets`.
    pub(crate) fn open(
        namespace: &Path,
        registry: Arc<nessa_auth::adapters::local::LocalCredentialStore>,
        audience: nessa_auth::domain::AudienceId,
        clock: Arc<dyn Clock>,
    ) -> Arc<Self> {
        Arc::new(Self::new(
            namespace.join("tunnel"),
            namespace
                .join("conversations")
                .join("audit")
                .join("tunnel-device-secrets"),
            clock,
            Arc::new(RegistryIds { registry, audience }),
        ))
    }

    /// Secrets in `directory` (`<namespace>/tunnel`), audit records in `audit`.
    pub(crate) fn new(
        directory: PathBuf,
        audit: PathBuf,
        clock: Arc<dyn Clock>,
        active: Arc<dyn ActiveDeviceCredentials>,
    ) -> Self {
        Self {
            secrets: directory.join(SECRETS_FILE),
            audit,
            clock,
            active,
            gate: Mutex::new(()),
        }
    }

    fn lock(&self) -> MutexGuard<'_, ()> {
        self.gate.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn read(&self) -> Result<Vec<Row>, SecretError> {
        let bytes = match fs::read(&self.secrets) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
            Err(_) => return Err(SecretError::Storage),
        };
        let value: Value = serde_json::from_slice(&bytes).map_err(|_| SecretError::Storage)?;
        let Some(devices) = value.get("devices").and_then(Value::as_array) else {
            return Err(SecretError::Storage);
        };
        let mut rows = Vec::new();
        for device in devices {
            let credential_id = device
                .get("credentialId")
                .and_then(Value::as_str)
                .ok_or(SecretError::Storage)?
                .to_owned();
            let device_key = decode_key(
                device
                    .get("deviceKey")
                    .and_then(Value::as_str)
                    .ok_or(SecretError::Storage)?,
            )?;
            let secret = TunnelSecret::decode(
                device
                    .get("secret")
                    .and_then(Value::as_str)
                    .ok_or(SecretError::Storage)?,
            )
            .ok_or(SecretError::Storage)?;
            rows.push(Row {
                credential_id,
                device_key,
                secret: *secret.expose(),
            });
        }
        Ok(rows)
    }

    fn write(&self, rows: &[Row]) -> Result<(), SecretError> {
        let directory = self.secrets.parent().ok_or(SecretError::Storage)?;
        create_directory(directory).map_err(|_| SecretError::Storage)?;
        let devices: Vec<Value> = rows
            .iter()
            .map(|row| {
                json!({
                    "credentialId": row.credential_id,
                    "deviceKey": encode_key(&row.device_key),
                    "secret": TunnelSecret::new(row.secret).encode(),
                })
            })
            .collect();
        let mut file = PrivateTempFile::new_in(directory).map_err(|_| SecretError::Storage)?;
        serde_json::to_writer(file.as_file_mut(), &json!({ "devices": devices }))
            .map_err(|_| SecretError::Storage)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| SecretError::Storage)?;
        file.persist(&self.secrets)
            .map_err(|_| SecretError::Storage)?;
        sync_directory(directory).map_err(|_| SecretError::Storage)
    }

    fn audit(
        &self,
        phase: &str,
        row: &Row,
        cause: &str,
        initiator: &str,
        before: Option<&str>,
        after: &str,
    ) -> Result<(), SecretError> {
        create_directory(&self.audit).map_err(|_| SecretError::Audit)?;
        let mut suffix = [0; 4];
        getrandom::fill(&mut suffix).map_err(|_| SecretError::Audit)?;
        let name = format!(
            "{phase}-{}-{}-{}.json",
            &fingerprint(row.credential_id.as_bytes())[..8],
            self.clock.unix_milliseconds(),
            fingerprint(&suffix)
        );
        let value = json!({
            "kind": "tunnel_device_secret",
            "recordId": name.trim_end_matches(".json"),
            "credentialId": row.credential_id,
            "deviceKey": encode_key(&row.device_key),
            "fingerprint": fingerprint(&row.secret),
            "phase": phase,
            "cause": cause,
            "before": before,
            "after": after,
            "initiator": { "kind": "principal", "principalId": initiator },
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        let mut file = PrivateTempFile::new_in(&self.audit).map_err(|_| SecretError::Audit)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(|_| SecretError::Audit)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| SecretError::Audit)?;
        file.publish(&self.audit.join(name))
            .map_err(|_| SecretError::Audit)?;
        sync_directory(&self.audit).map_err(|_| SecretError::Audit)
    }

    fn live(&self) -> Vec<[u8; 32]> {
        let Ok(ids) = self.active.ids() else {
            return Vec::new();
        };
        let Ok(rows) = self.read() else {
            return Vec::new();
        };
        rows.into_iter()
            .filter(|row| ids.iter().any(|id| id == &row.credential_id))
            .map(|row| row.secret)
            .collect()
    }
}

impl DeviceTunnelSecrets for FileDeviceSecrets {
    fn issue(
        &self,
        credential_id: &str,
        device_key: &[u8; 32],
        initiator: &str,
    ) -> Result<[u8; 32], SecretError> {
        let _held = self.lock();
        let mut rows = self.read()?;
        if let Some(row) = rows.iter().find(|row| row.credential_id == credential_id) {
            return Ok(row.secret);
        }
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).map_err(|_| SecretError::Storage)?;
        let row = Row {
            credential_id: credential_id.to_owned(),
            device_key: *device_key,
            secret,
        };
        self.audit("issued", &row, "device_paired", initiator, None, "active")?;
        rows.push(row);
        self.write(&rows)?;
        Ok(secret)
    }

    fn revoke(
        &self,
        credential_id: &str,
        cause: SecretRevocation,
        initiator: &str,
    ) -> Result<(), SecretError> {
        let _held = self.lock();
        let mut rows = self.read()?;
        let Some(index) = rows
            .iter()
            .position(|row| row.credential_id == credential_id)
        else {
            return Ok(());
        };
        let cause = match cause {
            SecretRevocation::Unpaired => "unpaired",
            SecretRevocation::CredentialRevoked => "credential_revoked",
        };
        // The audit is written before the file loses the secret. A failure
        // here leaves the secret in place; the credential is already ended,
        // so `accepts` refuses it, and the retry writes the record.
        self.audit(
            "revoked",
            &rows[index],
            cause,
            initiator,
            Some("active"),
            "revoked",
        )?;
        rows.remove(index);
        self.write(&rows)
    }

    fn accepts(&self, presented: &[u8]) -> bool {
        let _held = self.lock();
        any_match(presented, &self.live())
    }

    fn authorize_lookup(
        &self,
        gateway: &nessa_protocol::relay::GatewayKey,
        nonce: &[u8; 32],
        mac: &[u8; 32],
    ) -> bool {
        let _held = self.lock();
        any_mac(&self.live(), gateway, nonce, mac)
    }
}

struct RegistryIds {
    registry: Arc<nessa_auth::adapters::local::LocalCredentialStore>,
    audience: nessa_auth::domain::AudienceId,
}

impl ActiveDeviceCredentials for RegistryIds {
    fn ids(&self) -> Result<Vec<String>, SecretError> {
        self.registry
            .active_device_credential_ids(&self.audience)
            .map(|ids| ids.into_iter().map(|id| id.as_str().to_owned()).collect())
            .map_err(|_| SecretError::Storage)
    }
}

fn encode_key(key: &[u8; 32]) -> String {
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(key)
}

fn decode_key(text: &str) -> Result<[u8; 32], SecretError> {
    let bytes = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(text)
        .map_err(|_| SecretError::Storage)?;
    bytes.try_into().map_err(|_| SecretError::Storage)
}

/// First 16 hex characters of SHA-256. Enough to tell two secrets apart in
/// an audit record, and not enough to stand in for one.
fn fingerprint(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .take(8)
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_auth::application::ports::Clock;
    use std::sync::Mutex;

    struct Fixed;
    impl Clock for Fixed {
        fn unix_milliseconds(&self) -> u64 {
            1_700_000_000_000
        }
        fn unix_seconds(&self) -> u64 {
            1_700_000_000
        }
    }

    struct Ids(Mutex<Vec<String>>);
    impl ActiveDeviceCredentials for Ids {
        fn ids(&self) -> Result<Vec<String>, SecretError> {
            Ok(self.0.lock().unwrap().clone())
        }
    }

    fn store(active: Vec<String>) -> (tempfile::TempDir, FileDeviceSecrets, Arc<Ids>) {
        let directory = tempfile::tempdir().unwrap();
        let ids = Arc::new(Ids(Mutex::new(active)));
        let secrets = FileDeviceSecrets::new(
            directory.path().join("tunnel"),
            directory.path().join("audit"),
            Arc::new(Fixed),
            ids.clone(),
        );
        (directory, secrets, ids)
    }

    #[test]
    fn issuing_and_revoking_a_secret_is_audited_and_a_revoked_one_is_refused() {
        let (_directory, secrets, ids) = store(vec!["device-1".into()]);
        let secret = secrets.issue("device-1", &[7; 32], "owner").unwrap();
        assert_eq!(
            secrets.issue("device-1", &[7; 32], "owner").unwrap(),
            secret,
            "a second status returns the same secret"
        );
        assert!(secrets.accepts(&secret));
        let audits = std::fs::read_dir(_directory.path().join("audit")).unwrap();
        let issued: Vec<_> = audits
            .map(|entry| entry.unwrap().path())
            .filter(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("issued-")
            })
            .collect();
        assert_eq!(issued.len(), 1, "one issue record, not one per status");
        let record: Value = serde_json::from_slice(&std::fs::read(&issued[0]).unwrap()).unwrap();
        assert_eq!(record["credentialId"], "device-1");
        assert_eq!(record["cause"], "device_paired");
        assert_eq!(record["after"], "active");
        assert_eq!(record["initiator"]["principalId"], "owner");
        assert!(record["fingerprint"].as_str().unwrap().len() == 16);
        assert!(record.get("secret").is_none());
        let text = String::from_utf8(std::fs::read(&issued[0]).unwrap()).unwrap();
        assert!(!text.contains(&TunnelSecret::new(secret).encode()));

        ids.0.lock().unwrap().clear();
        assert!(
            !secrets.accepts(&secret),
            "a credential that is no longer active is refused before the file is updated"
        );
        ids.0.lock().unwrap().push("device-1".into());
        secrets
            .revoke("device-1", SecretRevocation::CredentialRevoked, "owner")
            .unwrap();
        assert!(!secrets.accepts(&secret));
        let revoked = std::fs::read_dir(_directory.path().join("audit"))
            .unwrap()
            .map(|entry| entry.unwrap().path())
            .find(|path| {
                path.file_name()
                    .unwrap()
                    .to_str()
                    .unwrap()
                    .starts_with("revoked-")
            })
            .unwrap();
        let record: Value = serde_json::from_slice(&std::fs::read(revoked).unwrap()).unwrap();
        assert_eq!(record["cause"], "credential_revoked");
        assert_eq!(record["before"], "active");
        assert_eq!(record["after"], "revoked");
        assert_eq!(record["initiator"]["principalId"], "owner");
    }
}
