//! Durable evidence for a credential registry refused at open.

use crate::application::{
    credential_registry::{
        CredentialRegistryAuditError, CredentialRegistryFault, CredentialRegistryRefusal,
        CredentialRegistryRefusalAudit,
    },
    ports::Clock,
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use nessa_local_storage::{create_directory, sync_directory, PrivateTempFile};
use serde_json::{json, Value};
use std::{
    io::Write,
    path::{Path, PathBuf},
    sync::Arc,
};

/// One immutable, synced file per refused registry read.
pub struct DurableCredentialRegistryRefusalAudit {
    directory: PathBuf,
    clock: Arc<dyn Clock>,
}

impl DurableCredentialRegistryRefusalAudit {
    /// Configure the audit directory. It is created only if a refusal is
    /// actually recorded, so a healthy open performs no audit filesystem work.
    pub fn new(directory: PathBuf, clock: Arc<dyn Clock>) -> Self {
        Self { directory, clock }
    }
}

impl CredentialRegistryRefusalAudit for DurableCredentialRegistryRefusalAudit {
    fn record(
        &self,
        refusal: &CredentialRegistryRefusal,
    ) -> Result<(), CredentialRegistryAuditError> {
        create_directory(&self.directory).map_err(unavailable)?;
        let id = record_id()?;
        let value = json!({
            "recordId": id,
            "kind": "credential_registry_refused",
            "target": path_value(refusal.target()),
            "transition": {
                "before": "registry_present_untrusted",
                "after": "registry_open_refused_file_preserved",
            },
            "cause": refusal.cause().as_str(),
            "initiator": {"kind": refusal.initiator().as_str()},
            "fault": fault_value(refusal.fault()),
            "observedAtMs": self.clock.unix_milliseconds(),
        });
        let mut file = PrivateTempFile::new_in(&self.directory).map_err(unavailable)?;
        serde_json::to_writer(file.as_file_mut(), &value).map_err(unavailable)?;
        file.as_file_mut().write_all(b"\n").map_err(unavailable)?;
        file.as_file().sync_all().map_err(unavailable)?;
        file.persist(&self.directory.join(format!("{id}.json")))
            .map_err(unavailable)?;
        sync_directory(&self.directory).map_err(unavailable)
    }
}

fn record_id() -> Result<String, CredentialRegistryAuditError> {
    let mut bytes = [0_u8; 16];
    getrandom::fill(&mut bytes).map_err(unavailable)?;
    Ok(URL_SAFE_NO_PAD.encode(bytes))
}

fn unavailable(error: impl std::fmt::Display) -> CredentialRegistryAuditError {
    CredentialRegistryAuditError::unavailable(error.to_string())
}

fn fault_value(fault: &CredentialRegistryFault) -> Value {
    match fault {
        CredentialRegistryFault::MalformedJson {
            line,
            column,
            category,
        } => json!({
            "kind": "malformed_json",
            "category": category.as_str(),
            "line": line,
            "column": column,
        }),
        CredentialRegistryFault::UnsupportedSchema { found, expected } => json!({
            "kind": "unsupported_schema",
            "found": found,
            "expected": expected,
        }),
        CredentialRegistryFault::InvalidState(rule) => json!({
            "kind": "invalid_state",
            "invariant": rule.as_str(),
        }),
        CredentialRegistryFault::TooLarge {
            observed_bytes,
            maximum_bytes,
        } => json!({
            "kind": "too_large",
            "observedBytes": observed_bytes,
            "maximumBytes": maximum_bytes,
        }),
    }
}

fn path_value(path: &Path) -> Value {
    if let Some(value) = path.to_str() {
        return json!({"encoding": "utf8", "value": value});
    }
    #[cfg(unix)]
    {
        use std::os::unix::ffi::OsStrExt;
        return json!({
            "encoding": "unix_bytes_base64url",
            "value": URL_SAFE_NO_PAD.encode(path.as_os_str().as_bytes()),
        });
    }
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        let bytes: Vec<u8> = path
            .as_os_str()
            .encode_wide()
            .flat_map(u16::to_le_bytes)
            .collect();
        return json!({
            "encoding": "windows_utf16le_base64url",
            "value": URL_SAFE_NO_PAD.encode(bytes),
        });
    }
    #[allow(unreachable_code)]
    json!({"encoding": "platform_debug", "value": format!("{path:?}")})
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::application::credential_registry::{
        CredentialRegistryRefusalCause, CredentialRegistryRefusalInitiator,
    };
    use std::fs;

    struct FixedClock;
    impl Clock for FixedClock {
        fn unix_milliseconds(&self) -> u64 {
            42
        }
    }

    #[test]
    fn refusal_record_keeps_target_transition_cause_and_initiator() {
        let root = tempfile::tempdir().unwrap();
        let audit = DurableCredentialRegistryRefusalAudit::new(
            root.path().join("audit"),
            Arc::new(FixedClock),
        );
        let target = root.path().join("credentials.v1.json");
        let refusal = CredentialRegistryRefusal::new(
            target.clone(),
            CredentialRegistryFault::UnsupportedSchema {
                found: 1,
                expected: 2,
            },
            CredentialRegistryRefusalCause::GatewayStartup,
            CredentialRegistryRefusalInitiator::Automatic,
        );

        audit.record(&refusal).unwrap();

        let entry = fs::read_dir(root.path().join("audit"))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        let record: Value = serde_json::from_slice(&fs::read(entry.path()).unwrap()).unwrap();
        assert_eq!(record["target"]["value"], target.to_str().unwrap());
        assert_eq!(record["transition"]["before"], "registry_present_untrusted");
        assert_eq!(
            record["transition"]["after"],
            "registry_open_refused_file_preserved"
        );
        assert_eq!(record["cause"], "gateway_startup");
        assert_eq!(record["initiator"]["kind"], "automatic");
        assert_eq!(record["fault"]["kind"], "unsupported_schema");
        assert_eq!(record["observedAtMs"], 42);
    }

    #[cfg(unix)]
    #[test]
    fn non_utf8_target_is_encoded_without_replacement() {
        use std::{ffi::OsStr, os::unix::ffi::OsStrExt};

        let bytes = b"/private/credentials-\xff.json";
        let value = path_value(Path::new(OsStr::from_bytes(bytes)));

        assert_eq!(value["encoding"], "unix_bytes_base64url");
        assert_eq!(
            URL_SAFE_NO_PAD
                .decode(value["value"].as_str().unwrap())
                .unwrap(),
            bytes
        );
    }

    #[cfg(windows)]
    #[test]
    fn non_unicode_target_is_encoded_without_replacement() {
        use std::{ffi::OsString, os::windows::ffi::OsStringExt};

        let units = [b'C' as u16, b':' as u16, b'\\' as u16, 0xD800];
        let path = PathBuf::from(OsString::from_wide(&units));
        let value = path_value(&path);
        let decoded = URL_SAFE_NO_PAD
            .decode(value["value"].as_str().unwrap())
            .unwrap();
        let expected: Vec<u8> = units.into_iter().flat_map(u16::to_le_bytes).collect();

        assert_eq!(value["encoding"], "windows_utf16le_base64url");
        assert_eq!(decoded, expected);
    }
}
