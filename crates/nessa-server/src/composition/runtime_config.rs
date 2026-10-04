//! File loading belongs to composition; consumers receive typed settings.
use super::agent::AgentsConfig;
use crate::{core::RunError, product::SessionSettings};
use nessa_auth::adapters::local::LocalStoreConfig;
use serde::Deserialize;
use std::{io::Read, net::SocketAddr, path::Path, time::Duration};

/// The most bytes `config.json` may have: what startup reads and what a
/// change to the stored MCP servers may write (`mcp_servers::settings`).
pub(super) const MAX_CONFIG_BYTES: usize = 65_536;

#[derive(Default, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RuntimeConfig {
    pub registry: LocalStoreConfig,
    pub session: SessionConfig,
    pub agents: Option<AgentsConfig>,
    /// Native device pairing; absent or `null` keeps it off (design rows S1, S2).
    pub native: Option<NativeConfig>,
}

/// Where the native enrollment listener binds. The address is numeric: serde's
/// standard `SocketAddr` parser, so no hostname is ever looked up.
#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct NativeConfig {
    pub listen_address: SocketAddr,
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
        let path = config_path(
            auth_directory
                .parent()
                .ok_or_else(|| invalid("invalid data directory"))?,
        );
        match std::fs::symlink_metadata(&path) {
            Ok(metadata) if metadata.is_file() => {}
            Ok(_) => return Err(refused("config.json must be a regular file")),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(invalid(error)),
        }
        let file = nessa_local_storage::open(&path, nessa_local_storage::OpenMode::Read)
            .map_err(invalid)?;
        let mut bytes = Vec::new();
        file.take(MAX_CONFIG_BYTES as u64 + 1)
            .read_to_end(&mut bytes)
            .map_err(invalid)?;
        if bytes.len() > MAX_CONFIG_BYTES {
            return Err(refused("config.json exceeds 64 KiB"));
        }
        Self::parse(&bytes)
    }

    pub(super) fn parse(bytes: &[u8]) -> Result<Self, RunError> {
        let config: Self = serde_json::from_slice(bytes).map_err(refused)?;
        config.registry.validate().map_err(refused)?;
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
        .map_err(refused)
    }
}
/// Where the namespace at `namespace` keeps `config.json`.
pub(super) fn config_path(namespace: &Path) -> std::path::PathBuf {
    namespace.join("config.json")
}

/// `config.json` could not be read: the file system can change before the next
/// start, so this stays a retried setup failure.
fn invalid(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(format!("invalid runtime config: {error}"))
}
/// `config.json` was read and its contents refused. Reading the same file
/// again refuses it the same way (design row S2).
fn refused(error: impl std::fmt::Display) -> RunError {
    RunError::RuntimeConfig(error.to_string())
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
    /// Design row S2: refused contents are a configuration failure that a
    /// restart cannot fix, whichever section they are in.
    #[test]
    fn refused_runtime_configuration_is_not_worth_restarting_for() {
        for bytes in [
            br#"{"native":{"listenAddress":"localhost:47650"}}"#.as_slice(),
            br#"{"native":{"listenAddress":"127.0.0.1"}}"#,
            br#"{"native":{"listenAddress":"127.0.0.1:1","tls":true}}"#,
            br#"{"session":{"writeTimeoutMs":0}}"#,
            b"not json",
        ] {
            assert!(matches!(
                RuntimeConfig::parse(bytes),
                Err(RunError::RuntimeConfig(_))
            ));
        }
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
    /// Design rows S1 and S2: native pairing is off unless named, and a
    /// malformed native section refuses startup before anything is opened.
    #[test]
    fn native_config_refuses_before_effect() {
        assert!(RuntimeConfig::parse(b"{}").unwrap().native.is_none());
        assert!(RuntimeConfig::parse(br#"{"native":null}"#)
            .unwrap()
            .native
            .is_none());
        for (bytes, address) in [
            (
                br#"{"native":{"listenAddress":"127.0.0.1:47650"}}"#.as_slice(),
                "127.0.0.1:47650",
            ),
            (br#"{"native":{"listenAddress":"[::1]:0"}}"#, "[::1]:0"),
            (
                br#"{"native":{"listenAddress":"0.0.0.0:47650"}}"#,
                "0.0.0.0:47650",
            ),
        ] {
            assert_eq!(
                RuntimeConfig::parse(bytes)
                    .unwrap()
                    .native
                    .unwrap()
                    .listen_address,
                address.parse::<SocketAddr>().unwrap()
            );
        }
        for bytes in [
            br#"{"native":{"listenAddress":"localhost:47650"}}"#.as_slice(),
            br#"{"native":{"listenAddress":"127.0.0.1"}}"#,
            br#"{"native":{"listenAddress":""}}"#,
            br#"{"native":{"listenAddress":47650}}"#,
            br#"{"native":{}}"#,
            br#"{"native":{"listenAddress":"127.0.0.1:1","tls":true}}"#,
            br#"{"native":"127.0.0.1:47650"}"#,
        ] {
            assert!(
                RuntimeConfig::parse(bytes).is_err(),
                "{}",
                String::from_utf8_lossy(bytes)
            );
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
