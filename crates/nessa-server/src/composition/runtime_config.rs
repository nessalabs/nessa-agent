//! File loading belongs to composition; consumers receive typed settings.
use super::agent::AgentsConfig;
use crate::{core::RunError, product::SessionSettings};
use nessa_auth::adapters::local::LocalStoreConfig;
use serde::Deserialize;
use std::{io::Read, path::Path, time::Duration};

#[derive(Default, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RuntimeConfig {
    pub registry: LocalStoreConfig,
    pub session: SessionConfig,
    pub agents: Option<AgentsConfig>,
}

#[derive(Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct SessionConfig {
    handshake_timeout_ms: u64,
    write_timeout_ms: u64,
    current_state_interval_ms: u64,
}
impl Default for SessionConfig {
    fn default() -> Self {
        Self {
            handshake_timeout_ms: 10_000,
            write_timeout_ms: 5_000,
            current_state_interval_ms: 1_000,
        }
    }
}
impl RuntimeConfig {
    /// Optional config.json beside auth/, scoped to the same stage and instance.
    /// Invalid existing files fail startup; only a missing file selects defaults.
    pub fn load(auth_directory: &Path) -> Result<Self, RunError> {
        let path = auth_directory
            .parent()
            .ok_or_else(|| invalid("invalid data directory"))?
            .join("config.json");
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(invalid("config.json must be a regular file")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(invalid(error)),
        }
        let file = nessa_local_storage::open(&path, nessa_local_storage::OpenMode::Read)
            .map_err(invalid)?;
        let mut bytes = Vec::new();
        file.take(65_537).read_to_end(&mut bytes).map_err(invalid)?;
        if bytes.len() > 65_536 {
            return Err(invalid("config.json exceeds 64 KiB"));
        }
        Self::parse(&bytes)
    }

    fn parse(bytes: &[u8]) -> Result<Self, RunError> {
        let config: Self = serde_json::from_slice(bytes).map_err(invalid)?;
        config.registry.validate().map_err(invalid)?;
        config.session()?;
        Ok(config)
    }

    pub fn session(&self) -> Result<SessionSettings, RunError> {
        // The settings type owns what a usable deadline is, so a configuration
        // file and an embedding caller are rejected by the same rule.
        SessionSettings::new(
            Duration::from_millis(self.session.handshake_timeout_ms),
            Duration::from_millis(self.session.write_timeout_ms),
            Duration::from_millis(self.session.current_state_interval_ms),
        )
        .map_err(invalid)
    }
}
fn invalid(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(format!("invalid runtime config: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::product::InvalidSessionSettings;
    #[test]
    fn config_loading_accepts_only_private_regular_files() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("namespace");
        nessa_local_storage::create_directory(&directory).unwrap();
        let auth = directory.join("auth");
        assert!(RuntimeConfig::load(&auth).is_ok());
        let path = directory.join("config.json");
        nessa_local_storage::open(&path, nessa_local_storage::OpenMode::CreateNew)
            .unwrap()
            .write_all(br#"{"session":{"handshakeTimeoutMs":1000}}"#)
            .unwrap();
        assert_eq!(
            RuntimeConfig::load(&auth)
                .unwrap()
                .session()
                .unwrap()
                .handshake_timeout(),
            Duration::from_secs(1)
        );
        std::fs::hard_link(&path, directory.join("alias.json")).unwrap();
        assert!(RuntimeConfig::load(&auth).is_err());
        std::fs::remove_file(directory.join("alias.json")).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::{symlink, PermissionsExt};
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
            assert!(RuntimeConfig::load(&auth).is_err());
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
            let target = directory.join("target.json");
            std::fs::rename(&path, &target).unwrap();
            symlink(&target, &path).unwrap();
            assert!(RuntimeConfig::load(&auth).is_err());
            std::fs::remove_file(target).unwrap();
            assert!(RuntimeConfig::load(&auth).is_err());
        }
    }

    #[test]
    fn partial_settings_override_only_their_scope() {
        let a = RuntimeConfig::parse(br#"{"registry":{"maxCredentials":2000,"maxRegistryBytes":8388608},"session":{"writeTimeoutMs":75}}"#).unwrap();
        let b = RuntimeConfig::parse(b"{}").unwrap();
        assert_eq!(a.registry.max_credentials, 2000);
        assert_eq!(a.registry.max_registry_bytes, 8388608);
        assert_eq!(a.registry.max_receipts, 2000);
        assert_eq!(
            a.session().unwrap().write_timeout(),
            Duration::from_millis(75)
        );
        assert_eq!(b.registry.max_credentials, 1000);
        assert_eq!(b.session().unwrap().write_timeout(), Duration::from_secs(5));
    }
    #[test]
    fn invalid_settings_are_never_silently_defaulted() {
        for bytes in [
            br#"{"registry":{"maxCredentials":0}}"#.as_slice(),
            br#"{"registry":{"maxRegistryBytes":18446744073709551615}}"#,
            br#"{"session":{"writeTimeoutMs":0}}"#,
            br#"{"session":{"writeTimeoutMs":-1}}"#,
            br#"{"session":{"writeTimoutMs":3}}"#,
            br#"{"unknown":true}"#,
            b"not json",
        ] {
            assert!(RuntimeConfig::parse(bytes).is_err());
        }
    }
    #[test]
    fn a_zero_polling_interval_is_rejected_by_both_entry_paths() {
        // The file path and a caller constructing settings directly now fail on
        // the same rule, rather than one of them reaching `tokio::time::interval`.
        assert!(RuntimeConfig::parse(br#"{"session":{"currentStateIntervalMs":0}}"#).is_err());
        assert_eq!(
            SessionSettings::new(
                Duration::from_secs(10),
                Duration::from_secs(5),
                Duration::ZERO,
            )
            .unwrap_err(),
            InvalidSessionSettings::CurrentStateInterval
        );
        assert_eq!(
            RuntimeConfig::parse(br#"{"session":{"currentStateIntervalMs":250}}"#)
                .unwrap()
                .session()
                .unwrap()
                .current_state_interval(),
            Duration::from_millis(250)
        );
    }
}
