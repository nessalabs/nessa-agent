//! Paired devices' tunnel secrets, on disk.
//!
//! `<namespace>/tunnel/device-secrets.json` holds each active secret.
//! `<conversations>/audit/tunnel-device-secrets/` holds two private records
//! for each issue and each revocation, tied by one operation id: an intent,
//! written before the secret file changes, and a publication, written only
//! after that change is stored. A retry of a failed publication keeps the
//! same operation id and does not write a second intent. The unpublished
//! secret sits in `<namespace>/tunnel/pending/`, not in the audit. The
//! secret itself is not in the audit and not in a log.
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
    /// Test hook: the next `count` secret-file writes fail before the file changes.
    #[cfg(test)]
    fail_publications: std::sync::atomic::AtomicU32,
    /// Test hook: the next `count` publication audits fail after the file write.
    #[cfg(test)]
    fail_confirmations: std::sync::atomic::AtomicU32,
    /// Test hook: the next `count` audit records fail after the file is renamed into place.
    #[cfg(test)]
    fail_audit_syncs: std::sync::atomic::AtomicU32,
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
            #[cfg(test)]
            fail_publications: std::sync::atomic::AtomicU32::new(0),
            #[cfg(test)]
            fail_confirmations: std::sync::atomic::AtomicU32::new(0),
            #[cfg(test)]
            fail_audit_syncs: std::sync::atomic::AtomicU32::new(0),
        }
    }

    /// Fail the next `count` writes of `device-secrets.json` without changing it.
    #[cfg(test)]
    fn fail_next_publications(&self, count: u32) {
        self.fail_publications
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    /// Fail the next `count` publication audits, after the secret file is written.
    #[cfg(test)]
    fn fail_next_confirmations(&self, count: u32) {
        self.fail_confirmations
            .store(count, std::sync::atomic::Ordering::SeqCst);
    }

    /// Fail the next `count` audit writes after the record has been renamed into place.
    #[cfg(test)]
    fn fail_next_audit_syncs(&self, count: u32) {
        self.fail_audit_syncs
            .store(count, std::sync::atomic::Ordering::SeqCst);
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
        #[cfg(test)]
        if self
            .fail_publications
            .try_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |count| count.checked_sub(1),
            )
            .is_ok()
        {
            return Err(SecretError::Storage);
        }
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

    /// An intent with no matching publication. Its operation id is stable
    /// across retries. The secret, when one is waiting to be published, is
    /// in the pending file and not here.
    fn open_operation(
        &self,
        credential_id: &str,
        action: &str,
    ) -> Result<Option<Open>, SecretError> {
        let entries = match fs::read_dir(&self.audit) {
            Ok(entries) => entries,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(_) => return Err(SecretError::Audit),
        };
        let mut found = None;
        for entry in entries {
            let path = entry.map_err(|_| SecretError::Audit)?.path();
            let name = path
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or("");
            if !name.starts_with("intent-") {
                continue;
            }
            let value: Value =
                serde_json::from_slice(&fs::read(&path).map_err(|_| SecretError::Audit)?)
                    .map_err(|_| SecretError::Audit)?;
            if value["credentialId"] != credential_id || value["action"] != action {
                continue;
            }
            let operation_id = value["operationId"].as_str().ok_or(SecretError::Audit)?;
            if self
                .audit
                .join(format!("published-{operation_id}.json"))
                .exists()
            {
                continue;
            }
            found = Some(Open::parse(&value)?);
        }
        Ok(found)
    }

    fn write_record(
        &self,
        open: &Open,
        phase: &str,
        after: Option<&str>,
    ) -> Result<(), SecretError> {
        create_directory(&self.audit).map_err(|_| SecretError::Audit)?;
        let name = format!("{phase}-{}.json", open.operation_id);
        let path = self.audit.join(&name);
        if path.exists() {
            return Ok(());
        }
        let value = json!({
            "kind": "tunnel_device_secret",
            "recordId": format!("{phase}-{}", open.operation_id),
            "operationId": open.operation_id,
            "action": open.action,
            "credentialId": open.credential_id,
            "deviceKey": encode_key(&open.device_key),
            "fingerprint": open.fingerprint,
            "phase": phase,
            "cause": open.cause,
            "before": open.before,
            "after": after,
            "initiator": { "kind": "principal", "principalId": open.initiator },
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        let mut file = PrivateTempFile::new_in(&self.audit).map_err(|_| SecretError::Audit)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(|_| SecretError::Audit)?;
        file.as_file_mut()
            .write_all(b"\n")
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| SecretError::Audit)?;
        file.publish(&path).map_err(|_| SecretError::Audit)?;
        #[cfg(test)]
        if self
            .fail_audit_syncs
            .try_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |count| count.checked_sub(1),
            )
            .is_ok()
        {
            // The rename has landed. A directory sync that fails must not
            // pretend the record is absent.
            return Err(SecretError::Audit);
        }
        sync_directory(&self.audit).map_err(|_| SecretError::Audit)
    }

    fn intend(&self, open: &Open) -> Result<(), SecretError> {
        self.write_record(open, "intent", None)
    }

    fn confirm(&self, open: &Open, after: &str) -> Result<(), SecretError> {
        #[cfg(test)]
        if self
            .fail_confirmations
            .try_update(
                std::sync::atomic::Ordering::SeqCst,
                std::sync::atomic::Ordering::SeqCst,
                |count| count.checked_sub(1),
            )
            .is_ok()
        {
            return Err(SecretError::Audit);
        }
        self.write_record(open, "published", Some(after))
    }

    fn pending_path(&self, operation_id: &str) -> Result<PathBuf, SecretError> {
        Ok(self
            .secrets
            .parent()
            .ok_or(SecretError::Storage)?
            .join("pending")
            .join(operation_id))
    }

    fn write_pending(&self, operation_id: &str, secret: &[u8; 32]) -> Result<(), SecretError> {
        let path = self.pending_path(operation_id)?;
        let directory = path.parent().ok_or(SecretError::Storage)?;
        create_directory(directory).map_err(|_| SecretError::Storage)?;
        let mut file = PrivateTempFile::new_in(directory).map_err(|_| SecretError::Storage)?;
        file.as_file_mut()
            .write_all(secret)
            .and_then(|_| file.as_file().sync_all())
            .map_err(|_| SecretError::Storage)?;
        file.persist(&path).map_err(|_| SecretError::Storage)?;
        sync_directory(directory).map_err(|_| SecretError::Storage)
    }

    fn read_pending(&self, open: &Open) -> Result<[u8; 32], SecretError> {
        let bytes =
            fs::read(self.pending_path(&open.operation_id)?).map_err(|_| SecretError::Storage)?;
        let secret: [u8; 32] = bytes.try_into().map_err(|_| SecretError::Storage)?;
        if fingerprint(&secret) != open.fingerprint {
            return Err(SecretError::Storage);
        }
        Ok(secret)
    }

    fn clear_pending(&self, operation_id: &str) -> Result<(), SecretError> {
        match fs::remove_file(self.pending_path(operation_id)?) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(_) => Err(SecretError::Storage),
        }
    }

    /// Record the intent to issue, and keep the secret aside until the file
    /// write lands. The intent names the operation id a retry will reuse.
    fn intend_issue(
        &self,
        credential_id: &str,
        device_key: &[u8; 32],
        initiator: &str,
    ) -> Result<Open, SecretError> {
        let mut secret = [0; 32];
        getrandom::fill(&mut secret).map_err(|_| SecretError::Storage)?;
        let operation_id = operation_id()?;
        self.write_pending(&operation_id, &secret)?;
        let open = Open {
            operation_id,
            action: ACTION_ISSUE.to_owned(),
            credential_id: credential_id.to_owned(),
            device_key: *device_key,
            fingerprint: fingerprint(&secret),
            cause: "device_paired".to_owned(),
            initiator: initiator.to_owned(),
            before: None,
        };
        // A failed audit write may still have renamed the intent into place.
        // The pending secret stays, so the retry of this same intent can read it.
        self.intend(&open)?;
        Ok(open)
    }

    fn intend_revoke(&self, row: &Row, cause: &str, initiator: &str) -> Result<Open, SecretError> {
        let operation_id = operation_id()?;
        let open = Open {
            operation_id,
            action: ACTION_REVOKE.to_owned(),
            credential_id: row.credential_id.clone(),
            device_key: row.device_key,
            fingerprint: fingerprint(&row.secret),
            cause: cause.to_owned(),
            initiator: initiator.to_owned(),
            before: Some("active".to_owned()),
        };
        self.intend(&open)?;
        Ok(open)
    }

    /// Confirm operations the secret file already shows. Every issue and
    /// revoke runs this first, while the row is still the evidence of what
    /// was published. An issuance whose row is stored becomes active. A
    /// revocation whose row is already gone becomes revoked.
    fn reconcile(&self, credential_id: &str, rows: &[Row]) -> Result<(), SecretError> {
        if let Some(row) = rows.iter().find(|row| row.credential_id == credential_id) {
            if let Some(open) = self.open_operation(credential_id, ACTION_ISSUE)? {
                if fingerprint(&row.secret) != open.fingerprint {
                    return Err(SecretError::Storage);
                }
                self.confirm(&open, "active")?;
                self.clear_pending(&open.operation_id)?;
            }
        } else if let Some(open) = self.open_operation(credential_id, ACTION_REVOKE)? {
            self.confirm(&open, "revoked")?;
        }
        Ok(())
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
        self.reconcile(credential_id, &rows)?;
        if let Some(row) = rows.iter().find(|row| row.credential_id == credential_id) {
            return Ok(row.secret);
        }
        let open = match self.open_operation(credential_id, ACTION_ISSUE)? {
            Some(open) => open,
            None => self.intend_issue(credential_id, device_key, initiator)?,
        };
        let secret = self.read_pending(&open)?;
        rows.push(Row {
            credential_id: credential_id.to_owned(),
            device_key: open.device_key,
            secret,
        });
        self.write(&rows)?;
        self.confirm(&open, "active")?;
        self.clear_pending(&open.operation_id)?;
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
        self.reconcile(credential_id, &rows)?;
        let Some(index) = rows
            .iter()
            .position(|row| row.credential_id == credential_id)
        else {
            // The file write already removed the row, and the publication
            // audit did not land. Confirm that same revocation.
            if let Some(open) = self.open_operation(credential_id, ACTION_REVOKE)? {
                self.confirm(&open, "revoked")?;
            }
            // The secret was never stored. Close an open issue so a retry
            // cannot publish it after this revocation.
            if let Some(open) = self.open_operation(credential_id, ACTION_ISSUE)? {
                self.clear_pending(&open.operation_id)?;
                self.write_record(&open, "published", None)?;
            }
            return Ok(());
        };
        let cause = match cause {
            SecretRevocation::Unpaired => "unpaired",
            SecretRevocation::CredentialRevoked => "credential_revoked",
        };
        let open = match self.open_operation(credential_id, ACTION_REVOKE)? {
            Some(open) => open,
            None => self.intend_revoke(&rows[index], cause, initiator)?,
        };
        rows.remove(index);
        self.write(&rows)?;
        self.confirm(&open, "revoked")
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

const ACTION_ISSUE: &str = "issue";
const ACTION_REVOKE: &str = "revoke";

/// One operation's intent. The publication record uses the same id.
struct Open {
    operation_id: String,
    action: String,
    credential_id: String,
    device_key: [u8; 32],
    fingerprint: String,
    cause: String,
    initiator: String,
    before: Option<String>,
}

impl Open {
    fn parse(value: &Value) -> Result<Self, SecretError> {
        let text = |field: &str| {
            value
                .get(field)
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(SecretError::Audit)
        };
        let before = match value.get("before") {
            None | Some(Value::Null) => None,
            Some(Value::String(text)) => Some(text.clone()),
            Some(_) => return Err(SecretError::Audit),
        };
        Ok(Self {
            operation_id: text("operationId")?,
            action: text("action")?,
            credential_id: text("credentialId")?,
            device_key: decode_key(value["deviceKey"].as_str().ok_or(SecretError::Audit)?)
                .map_err(|_| SecretError::Audit)?,
            fingerprint: text("fingerprint")?,
            cause: text("cause")?,
            initiator: value
                .get("initiator")
                .and_then(|initiator| initiator.get("principalId"))
                .and_then(Value::as_str)
                .map(str::to_owned)
                .ok_or(SecretError::Audit)?,
            before,
        })
    }
}

fn operation_id() -> Result<String, SecretError> {
    let mut bytes = [0; 16];
    getrandom::fill(&mut bytes).map_err(|_| SecretError::Storage)?;
    Ok(bytes.iter().map(|byte| format!("{byte:02x}")).collect())
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
        let records = audit_records(_directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["phase"] == "intent" && record["action"] == "issue")
            .collect();
        let published: Vec<_> = records
            .iter()
            .filter(|record| record["phase"] == "published" && record["action"] == "issue")
            .collect();
        assert_eq!(intents.len(), 1, "one issue intent, not one per status");
        assert_eq!(published.len(), 1, "one publication confirms it");
        assert_eq!(intents[0]["operationId"], published[0]["operationId"]);
        assert!(intents[0]["after"].is_null());
        let record = published[0];
        assert_eq!(record["credentialId"], "device-1");
        assert_eq!(record["cause"], "device_paired");
        assert_eq!(record["after"], "active");
        assert_eq!(record["initiator"]["principalId"], "owner");
        assert!(record["fingerprint"].as_str().unwrap().len() == 16);
        assert!(record.get("secret").is_none());
        let encoded = TunnelSecret::new(secret).encode();
        for record in records.iter().filter(|record| record["action"] == "issue") {
            let text = record.to_string();
            assert!(!text.contains(&encoded));
        }
        assert!(
            !pending_dir(_directory.path()).exists()
                || std::fs::read_dir(pending_dir(_directory.path()))
                    .unwrap()
                    .next()
                    .is_none(),
            "the pending secret is removed once the publication is stored"
        );

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
        let records = audit_records(_directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["phase"] == "intent" && record["action"] == "revoke")
            .collect();
        let published: Vec<_> = records
            .iter()
            .filter(|record| record["phase"] == "published" && record["action"] == "revoke")
            .collect();
        assert_eq!(intents.len(), 1);
        assert_eq!(published.len(), 1);
        assert_eq!(intents[0]["operationId"], published[0]["operationId"]);
        assert!(intents[0]["after"].is_null());
        assert_eq!(published[0]["cause"], "credential_revoked");
        assert_eq!(published[0]["before"], "active");
        assert_eq!(published[0]["after"], "revoked");
        assert_eq!(published[0]["initiator"]["principalId"], "owner");
    }

    #[test]
    fn a_failed_publication_is_retried_as_the_same_operation() {
        let (directory, secrets, _) = store(vec!["device-1".into()]);
        secrets.fail_next_publications(1);
        assert_eq!(
            secrets.issue("device-1", &[7; 32], "owner"),
            Err(SecretError::Storage)
        );
        assert!(!secrets.accepts(&[7; 32]));
        let records = audit_records(directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "issue" && record["phase"] == "intent")
            .collect();
        assert_eq!(intents.len(), 1);
        assert!(records.iter().all(|record| record["phase"] != "published"));
        assert!(intents[0]["after"].is_null());
        let operation = intents[0]["operationId"].as_str().unwrap().to_owned();
        let fingerprint = intents[0]["fingerprint"].as_str().unwrap().to_owned();
        let pending = std::fs::read_dir(pending_dir(directory.path())).unwrap();
        let pending: Vec<_> = pending.map(|entry| entry.unwrap().path()).collect();
        assert_eq!(pending.len(), 1);
        assert_eq!(std::fs::read(&pending[0]).unwrap().len(), 32);
        assert!(
            !directory.path().join("tunnel").join(SECRETS_FILE).exists(),
            "the secret file was not written"
        );

        let secret = secrets.issue("device-1", &[7; 32], "owner").unwrap();
        assert!(secrets.accepts(&secret));
        let records = audit_records(directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "issue" && record["phase"] == "intent")
            .collect();
        let published: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "issue" && record["phase"] == "published")
            .collect();
        assert_eq!(intents.len(), 1, "the retry does not write a second intent");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0]["operationId"], operation);
        assert_eq!(published[0]["after"], "active");
        assert_eq!(published[0]["fingerprint"], fingerprint);
        assert!(published[0].get("secret").is_none());
        let encoded = TunnelSecret::new(secret).encode();
        assert!(!published[0].to_string().contains(&encoded));
        assert!(!intents[0].to_string().contains(&encoded));

        secrets.fail_next_publications(1);
        assert_eq!(
            secrets.revoke("device-1", SecretRevocation::Unpaired, "owner"),
            Err(SecretError::Storage)
        );
        assert!(
            secrets.accepts(&secret),
            "a failed revocation leaves the secret in place"
        );
        let records = audit_records(directory.path());
        let revoke_intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "revoke" && record["phase"] == "intent")
            .collect();
        assert_eq!(revoke_intents.len(), 1);
        assert!(revoke_intents[0]["after"].is_null());
        let revoke_operation = revoke_intents[0]["operationId"]
            .as_str()
            .unwrap()
            .to_owned();
        secrets
            .revoke("device-1", SecretRevocation::Unpaired, "owner")
            .unwrap();
        assert!(!secrets.accepts(&secret));
        let records = audit_records(directory.path());
        let revoke_intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "revoke" && record["phase"] == "intent")
            .collect();
        let revoke_published: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "revoke" && record["phase"] == "published")
            .collect();
        assert_eq!(revoke_intents.len(), 1);
        assert_eq!(revoke_published.len(), 1);
        assert_eq!(revoke_published[0]["operationId"], revoke_operation);
        assert_eq!(revoke_published[0]["before"], "active");
        assert_eq!(revoke_published[0]["after"], "revoked");
        assert_ne!(revoke_operation, operation);
    }

    #[test]
    fn a_revoke_whose_confirmation_failed_is_confirmed_on_retry() {
        let (directory, secrets, _) = store(vec!["device-1".into()]);
        let secret = secrets.issue("device-1", &[7; 32], "owner").unwrap();
        secrets.fail_next_confirmations(1);
        assert_eq!(
            secrets.revoke("device-1", SecretRevocation::Unpaired, "owner"),
            Err(SecretError::Audit)
        );
        assert!(
            !secrets.accepts(&secret),
            "the secret file already lost the row"
        );
        let records = audit_records(directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "revoke" && record["phase"] == "intent")
            .collect();
        assert_eq!(intents.len(), 1);
        assert!(records
            .iter()
            .all(|record| record["action"] != "revoke" || record["phase"] != "published"));
        let operation = intents[0]["operationId"].as_str().unwrap().to_owned();
        secrets
            .revoke("device-1", SecretRevocation::Unpaired, "owner")
            .unwrap();
        assert!(!secrets.accepts(&secret));
        let records = audit_records(directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "revoke" && record["phase"] == "intent")
            .collect();
        let published: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "revoke" && record["phase"] == "published")
            .collect();
        assert_eq!(intents.len(), 1, "the retry does not write a second intent");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0]["operationId"], operation);
        assert_eq!(published[0]["before"], "active");
        assert_eq!(published[0]["after"], "revoked");
    }

    #[test]
    fn a_revoke_confirms_the_issuance_the_file_already_holds() {
        let (directory, secrets, _) = store(vec!["device-1".into()]);
        secrets.fail_next_confirmations(1);
        assert_eq!(
            secrets.issue("device-1", &[7; 32], "owner"),
            Err(SecretError::Audit)
        );
        let pending = std::fs::read_dir(pending_dir(directory.path())).unwrap();
        let pending: Vec<_> = pending.map(|entry| entry.unwrap().path()).collect();
        assert_eq!(pending.len(), 1, "the unpublished copy is still aside");
        let secret: [u8; 32] = std::fs::read(&pending[0]).unwrap().try_into().unwrap();
        assert!(secrets.accepts(&secret), "the secret file holds the row");
        let records = audit_records(directory.path());
        let issue_intent = records
            .iter()
            .find(|record| record["action"] == "issue" && record["phase"] == "intent")
            .unwrap();
        let issue_operation = issue_intent["operationId"].as_str().unwrap().to_owned();
        assert!(records
            .iter()
            .all(|record| record["action"] != "issue" || record["phase"] != "published"));

        secrets
            .revoke("device-1", SecretRevocation::CredentialRevoked, "owner")
            .unwrap();
        assert!(!secrets.accepts(&secret));
        assert!(
            std::fs::read_dir(pending_dir(directory.path()))
                .map(|entries| entries.count())
                .unwrap_or(0)
                == 0,
            "the pending secret is deleted once the issuance is confirmed"
        );
        let records = audit_records(directory.path());
        let issued: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "issue" && record["phase"] == "published")
            .collect();
        let revoked: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "revoke" && record["phase"] == "published")
            .collect();
        assert_eq!(issued.len(), 1);
        assert_eq!(issued[0]["operationId"], issue_operation);
        assert_eq!(issued[0]["after"], "active");
        assert_eq!(revoked.len(), 1);
        assert_eq!(revoked[0]["before"], "active");
        assert_eq!(revoked[0]["after"], "revoked");
        assert_ne!(revoked[0]["operationId"], issue_operation);
    }

    #[test]
    fn an_intent_that_lands_keeps_its_pending_secret_when_the_directory_sync_fails() {
        let (directory, secrets, _) = store(vec!["device-1".into()]);
        secrets.fail_next_audit_syncs(1);
        assert_eq!(
            secrets.issue("device-1", &[7; 32], "owner"),
            Err(SecretError::Audit)
        );
        let pending = std::fs::read_dir(pending_dir(directory.path())).unwrap();
        let pending: Vec<_> = pending.map(|entry| entry.unwrap().path()).collect();
        assert_eq!(pending.len(), 1, "the pending secret is kept");
        let secret: [u8; 32] = std::fs::read(&pending[0]).unwrap().try_into().unwrap();
        let records = audit_records(directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "issue" && record["phase"] == "intent")
            .collect();
        assert_eq!(intents.len(), 1, "the renamed intent is visible");
        assert!(records.iter().all(|record| record["phase"] != "published"));
        let operation = intents[0]["operationId"].as_str().unwrap().to_owned();
        assert!(!secrets.accepts(&secret), "the secret file was not written");

        let issued = secrets.issue("device-1", &[7; 32], "owner").unwrap();
        assert_eq!(issued, secret);
        assert!(secrets.accepts(&secret));
        let records = audit_records(directory.path());
        let intents: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "issue" && record["phase"] == "intent")
            .collect();
        let published: Vec<_> = records
            .iter()
            .filter(|record| record["action"] == "issue" && record["phase"] == "published")
            .collect();
        assert_eq!(intents.len(), 1, "the retry reuses the intent");
        assert_eq!(published.len(), 1);
        assert_eq!(published[0]["operationId"], operation);
        assert_eq!(published[0]["after"], "active");
        assert_eq!(
            std::fs::read_dir(pending_dir(directory.path()))
                .map(|entries| entries.count())
                .unwrap_or(0),
            0
        );
    }

    fn audit_records(root: &std::path::Path) -> Vec<Value> {
        std::fs::read_dir(root.join("audit"))
            .unwrap()
            .map(|entry| {
                serde_json::from_slice(&std::fs::read(entry.unwrap().path()).unwrap()).unwrap()
            })
            .collect()
    }

    fn pending_dir(root: &std::path::Path) -> std::path::PathBuf {
        root.join("tunnel").join("pending")
    }
}
