//! File loading belongs to composition; consumers receive typed settings.
use crate::{core::RunError, product::SessionSettings};
use nessa_auth::adapters::local::LocalStoreConfig;
use serde::Deserialize;
use std::{io::Read, path::Path, time::Duration};

#[derive(Default, Debug, Deserialize)]
#[serde(default, deny_unknown_fields, rename_all = "camelCase")]
pub(super) struct RuntimeConfig {
    pub registry: LocalStoreConfig,
    pub session: SessionConfig,
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
        let file = match std::fs::File::open(&path) {
            Ok(file) => file,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                return Ok(Self::default())
            }
            Err(error) => return Err(invalid(error)),
        };
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
        let duration = |value| {
            // Check before constructing Tokio timers; reject zero and overflowing deadlines.
            let value = Duration::from_millis(value);
            if value.is_zero() || std::time::Instant::now().checked_add(value).is_none() {
                Err(invalid(
                    "session deadlines must be positive and representable",
                ))
            } else {
                Ok(value)
            }
        };
        Ok(SessionSettings {
            handshake_timeout: duration(self.session.handshake_timeout_ms)?,
            write_timeout: duration(self.session.write_timeout_ms)?,
            current_state_interval: duration(self.session.current_state_interval_ms)?,
        })
    }
}
fn invalid(error: impl std::fmt::Display) -> RunError {
    RunError::Authentication(format!("invalid runtime config: {error}"))
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn partial_settings_override_only_their_scope() {
        let a = RuntimeConfig::parse(br#"{"registry":{"maxCredentials":2000,"maxRegistryBytes":8388608},"session":{"writeTimeoutMs":75}}"#).unwrap();
        let b = RuntimeConfig::parse(b"{}").unwrap();
        assert_eq!(a.registry.max_credentials, 2000);
        assert_eq!(a.registry.max_registry_bytes, 8388608);
        assert_eq!(a.registry.max_receipts, 2000);
        assert_eq!(
            a.session().unwrap().write_timeout,
            Duration::from_millis(75)
        );
        assert_eq!(b.registry.max_credentials, 1000);
        assert_eq!(b.session().unwrap().write_timeout, Duration::from_secs(5));
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
}
