use super::config::{self, default, key};
use super::error::EnvironmentError;
use super::source::EnvSource;
use super::stage::Stage;
use super::stage_port::stage_port;

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
    endpoint_storage: Option<(std::path::PathBuf, std::path::PathBuf)>,
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

        // An explicit NESSA_PORT always wins; otherwise the stage decides, so a
        // dev server and an installed product gateway do not want one port.
        let port = match read_optional(source, key::PORT)? {
            Some(value) => parse_port(key::PORT, &value)?,
            None => stage_port(stage),
        };

        let uptime_backend = super::UptimeBackend::parse(
            stage,
            read_optional(source, key::UPTIME_BACKEND)?.as_deref(),
            read_optional(source, key::UPTIME_FIXED_MS)?.as_deref(),
        )?;
        let auth_directory = load_auth_directory(source, stage)?;
        let endpoint_storage = load_endpoint_storage(source, stage)?;

        Ok(Self {
            stage,
            bind_host,
            port,
            version: config::VERSION,
            uptime_backend,
            auth_directory,
            endpoint_storage,
        })
    }

    /// Resolve offline administration paths for the selected stage and instance.
    pub fn auth_directory_from_system() -> Result<std::path::PathBuf, EnvironmentError> {
        let source = super::source::SystemEnv;
        load_auth_directory(&source, load_stage(&source)?)?.ok_or(EnvironmentError::Backend(
            "set NESSA_DATA_DIR or the OS home directory (USERPROFILE on Windows, HOME on Unix) for local credentials",
        ))
    }

    /// This stage's log directory, resolved from the process environment alone.
    ///
    /// Read before the rest of the configuration is parsed, because what lives
    /// in there — the log this process is writing into, and what the last run
    /// wrote down about giving up — has to be dealt with even when the reason
    /// this run is ending is that its configuration would not parse. `None` is
    /// a process with no data root at all, which has no such directory.
    pub fn log_directory_from_system() -> Result<Option<std::path::PathBuf>, EnvironmentError> {
        let source = super::source::SystemEnv;
        let stage = load_stage(&source)?;
        super::paths::log_directory(
            read_optional(&source, key::DATA_DIR)?.as_deref(),
            read_optional(&source, home_variable())?.as_deref(),
            stage.as_str(),
            read_optional(&source, key::INSTANCE)?.as_deref(),
        )
    }

    /// The launchd service generation this process was registered under.
    ///
    /// `None` for anything the desktop host did not start: a developer's
    /// `nessa server`, a CLI subcommand, a test. Only the plist sets it, which
    /// is what makes it the identity of a managed launch rather than a guess.
    pub fn service_generation_from_system() -> Option<String> {
        read_optional(&super::source::SystemEnv, key::SERVICE_GENERATION)
            .ok()
            .flatten()
    }

    /// Plain browser HTTP is confined to numeric loopback in development and CI.
    pub fn browser_http_allowed(&self) -> bool {
        matches!(self.stage, Stage::Dev | Stage::Ci) && is_loopback(&self.bind_host)
    }

    pub fn listen_addr(&self) -> String {
        format_socket_addr(&self.bind_host, self.port)
    }

    /// The log directory in this already-resolved stage and instance namespace.
    ///
    /// Authentication composition requires `auth_directory` before serving, so
    /// its parent is the one namespace authority. Endpoint publication uses the
    /// sibling log directory without re-reading or re-interpreting environment
    /// variables.
    pub(crate) fn gateway_endpoint_storage(
        &self,
    ) -> Option<(std::path::PathBuf, std::path::PathBuf)> {
        self.endpoint_storage.clone()
    }
}

fn load_endpoint_storage(
    source: &impl EnvSource,
    stage: Stage,
) -> Result<Option<(std::path::PathBuf, std::path::PathBuf)>, EnvironmentError> {
    super::paths::endpoint_storage(
        read_optional(source, key::DATA_DIR)?.as_deref(),
        read_optional(source, home_variable())?.as_deref(),
        stage.as_str(),
        read_optional(source, key::INSTANCE)?.as_deref(),
    )
}

fn load_auth_directory(
    source: &impl EnvSource,
    stage: Stage,
) -> Result<Option<std::path::PathBuf>, EnvironmentError> {
    super::paths::auth_directory(
        read_optional(source, key::DATA_DIR)?.as_deref(),
        read_optional(source, home_variable())?.as_deref(),
        stage.as_str(),
        read_optional(source, key::INSTANCE)?.as_deref(),
    )
}

/// The OS variable holding the user's home directory on this target.
fn home_variable() -> &'static str {
    if cfg!(windows) {
        "USERPROFILE"
    } else {
        key::HOME
    }
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
    fn browser_http_requires_development_stage_and_loopback_bind() {
        for stage in ["dev", "ci", "alpha", "prod"] {
            let mut config = Environment::load(&MockEnv::new().set("NESSA_STAGE", stage)).unwrap();
            assert_eq!(config.browser_http_allowed(), matches!(stage, "dev" | "ci"));
            config.bind_host = "0.0.0.0".into();
            assert!(!config.browser_http_allowed());
        }
    }

    #[test]
    fn dev_stage_defaults_when_env_unset() {
        let config = Environment::load(&MockEnv::new()).expect("defaults");
        assert_eq!(config.stage, Stage::Dev);
        assert_eq!(config.bind_host, "127.0.0.1");
        assert_eq!(config.port, 7421);
        assert_eq!(config.version, VERSION);
    }

    #[test]
    fn stage_chooses_the_port_and_prod_keeps_the_product_one() {
        for (stage, expected) in [("dev", 7421), ("ci", 7420), ("alpha", 7420), ("prod", 7420)] {
            let config = Environment::load(&MockEnv::new().set(STAGE, stage)).expect("stage");
            assert_eq!(config.port, expected, "stage {stage}");
        }
    }

    #[test]
    fn explicit_port_overrides_the_stage_default() {
        for stage in ["dev", "prod"] {
            let config = Environment::load(&MockEnv::new().set(STAGE, stage).set(PORT, "9999"))
                .expect("explicit port");
            assert_eq!(config.port, 9999, "stage {stage}");
        }
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
        assert_eq!(config.listen_addr(), "[::1]:7421");
    }

    #[test]
    fn endpoint_publication_uses_the_resolved_auth_namespace() {
        let config = Environment::load(
            &MockEnv::new()
                .set("NESSA_DATA_DIR", "/private/nessa")
                .set(STAGE, "ci")
                .set("NESSA_INSTANCE", "worker-7"),
        )
        .unwrap();
        assert_eq!(
            config.gateway_endpoint_storage().unwrap(),
            (
                std::path::PathBuf::from("/private/nessa"),
                std::path::PathBuf::from("ci/instances/worker-7/logs")
            )
        );
    }
}
