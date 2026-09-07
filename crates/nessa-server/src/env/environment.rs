use super::config::{self, default, key};
use super::error::EnvironmentError;
use super::source::EnvSource;
use super::stage::Stage;

/// Fully parsed runtime configuration for this process.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Environment {
    pub stage: Stage,
    pub bind_host: String,
    pub port: u16,
    pub version: &'static str,
    /// Uptime selection affects health reporting, never authentication deadlines.
    pub uptime_backend: super::UptimeBackend,
    /// Private product auth directory, resolved from typed startup configuration.
    /// None is valid for isolated config tests; serving requires a data root.
    pub auth_directory: Option<std::path::PathBuf>,
}

impl Environment {
    /// Load from the real process environment. Production entry point only.
    pub fn from_system() -> Result<Self, EnvironmentError> {
        Self::load(&super::source::SystemEnv)
    }

    /// Load from any `EnvSource`. Use `MockEnv` in tests.
    pub fn load(source: &impl EnvSource) -> Result<Self, EnvironmentError> {
        let stage = load_stage(source)?;
        let bind_host =
            read_optional(source, key::HOST)?.unwrap_or_else(|| default::HOST.to_string());
        if bind_host.is_empty() {
            return Err(EnvironmentError::Empty {
                variable: key::HOST,
            });
        }
        validate_bind_host(stage, &bind_host, source)?;

        let port = match read_optional(source, key::PORT)? {
            Some(value) => parse_port(key::PORT, &value)?,
            None => default::PORT,
        };

        let uptime_backend = super::UptimeBackend::parse(
            stage,
            read_optional(source, key::UPTIME_BACKEND)?.as_deref(),
            read_optional(source, key::UPTIME_FIXED_MS)?.as_deref(),
        )?;
        let auth_directory = load_auth_directory(source, stage)?;

        Ok(Self {
            stage,
            bind_host,
            port,
            version: config::VERSION,
            uptime_backend,
            auth_directory,
        })
    }

    /// Resolve offline administration paths for the selected stage and instance.
    pub fn auth_directory_from_system() -> Result<std::path::PathBuf, EnvironmentError> {
        let source = super::source::SystemEnv;
        load_auth_directory(&source, load_stage(&source)?)?.ok_or(EnvironmentError::Backend(
            "set NESSA_DATA_DIR or the OS home directory (USERPROFILE on Windows, HOME on Unix) for local credentials",
        ))
    }

    pub fn listen_addr(&self) -> String {
        format_socket_addr(&self.bind_host, self.port)
    }
}

fn load_auth_directory(
    source: &impl EnvSource,
    stage: Stage,
) -> Result<Option<std::path::PathBuf>, EnvironmentError> {
    super::paths::auth_directory(
        read_optional(source, key::DATA_DIR)?.as_deref(),
        read_optional(
            source,
            if cfg!(windows) {
                "USERPROFILE"
            } else {
                key::HOME
            },
        )?
        .as_deref(),
        stage.as_str(),
        read_optional(source, key::INSTANCE)?.as_deref(),
    )
}

fn format_socket_addr(host: &str, port: u16) -> String {
    if host.contains(':') && !host.starts_with('[') {
        format!("[{host}]:{port}")
    } else {
        format!("{host}:{port}")
    }
}

fn load_stage(source: &impl EnvSource) -> Result<Stage, EnvironmentError> {
    match read_optional(source, key::STAGE)? {
        Some(value) => Stage::parse(&value).map_err(EnvironmentError::InvalidStage),
        None => Ok(Stage::Dev),
    }
}

fn read_optional(
    source: &impl EnvSource,
    key: &'static str,
) -> Result<Option<String>, EnvironmentError> {
    source.get(key).map_err(|source| EnvironmentError::Read {
        variable: key,
        source,
    })
}

fn parse_port(variable: &'static str, value: &str) -> Result<u16, EnvironmentError> {
    value
        .parse::<u16>()
        .map_err(|_| EnvironmentError::InvalidPort {
            variable,
            value: value.to_string(),
        })
}

fn validate_bind_host(
    stage: Stage,
    bind_host: &str,
    _source: &impl EnvSource,
) -> Result<(), EnvironmentError> {
    if is_loopback(bind_host) {
        return Ok(());
    }

    Err(EnvironmentError::InsecureBind {
        bind_host: bind_host.to_string(),
        stage,
    })
}

fn is_loopback(host: &str) -> bool {
    matches!(host, "127.0.0.1" | "::1")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::env::{MockEnv, HOST, PORT, STAGE, VERSION};

    #[test]
    fn dev_stage_defaults_when_env_unset() {
        let config = Environment::load(&MockEnv::new()).expect("defaults");
        assert_eq!(config.stage, Stage::Dev);
        assert_eq!(config.bind_host, "127.0.0.1");
        assert_eq!(config.port, 7420);
        assert_eq!(config.version, VERSION);
    }

    #[test]
    fn reads_explicit_env() {
        let config = Environment::load(
            &MockEnv::new()
                .set(STAGE, "alpha")
                .set(HOST, "127.0.0.1")
                .set(PORT, "8080"),
        )
        .expect("explicit env");
        assert_eq!(config.stage, Stage::Alpha);
        assert_eq!(config.port, 8080);
    }

    #[test]
    fn alpha_configuration_uses_local_auth() {
        let config = Environment::load(&MockEnv::new().set(STAGE, "alpha")).unwrap();
        assert_eq!(config.stage, Stage::Alpha);
    }

    #[test]
    fn rejects_invalid_port() {
        let error = Environment::load(&MockEnv::new().set(PORT, "not-a-port")).unwrap_err();
        assert!(matches!(error, EnvironmentError::InvalidPort { .. }));
    }

    #[test]
    fn alpha_rejects_remote_bind_without_override() {
        let error = Environment::load(&MockEnv::new().set(STAGE, "alpha").set(HOST, "0.0.0.0"))
            .unwrap_err();
        assert!(matches!(error, EnvironmentError::InsecureBind { .. }));
    }

    #[test]
    fn rejects_localhost_hostname_bind() {
        let error = Environment::load(&MockEnv::new().set(HOST, "localhost")).unwrap_err();
        assert!(matches!(error, EnvironmentError::InsecureBind { .. }));
    }

    #[test]
    fn dev_rejects_non_loopback_bind() {
        let error = Environment::load(&MockEnv::new().set(HOST, "0.0.0.0")).unwrap_err();
        assert!(matches!(error, EnvironmentError::InsecureBind { .. }));
    }

    #[test]
    fn formats_ipv6_loopback_listen_addr() {
        let config = Environment::load(&MockEnv::new().set(HOST, "::1")).expect("ipv6");
        assert_eq!(config.listen_addr(), "[::1]:7420");
    }
}
