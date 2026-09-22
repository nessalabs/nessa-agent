use nessa_agent_credentials::CredentialNamespace;
use std::{
    error::Error,
    fmt,
    path::{Component, Path, PathBuf},
};

/// Durable inputs that identify one packaged gateway service.
///
/// The value is assembled from the packaged stage and persisted host settings.
/// No field is inherited from the shell that happened to launch the desktop.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceConfiguration {
    credential_namespace: CredentialNamespace,
    data_root: PathBuf,
    port: u16,
    claude_config_directory: Option<PathBuf>,
}

impl ServiceConfiguration {
    /// Validate all durable service inputs before any registration effect.
    pub fn new(
        stage: String,
        data_root: PathBuf,
        instance: Option<String>,
        port: u16,
        claude_config_directory: Option<PathBuf>,
    ) -> Result<Self, ServiceConfigurationError> {
        CredentialNamespace::new(stage.clone(), None)
            .map_err(|_| ServiceConfigurationError::Stage)?;
        let credential_namespace = CredentialNamespace::new(stage, instance)
            .map_err(|_| ServiceConfigurationError::Instance)?;
        if !absolute_without_parent(&data_root) {
            return Err(ServiceConfigurationError::DataRoot);
        }
        if port == 0 {
            return Err(ServiceConfigurationError::Port);
        }
        if claude_config_directory
            .as_deref()
            .is_some_and(|directory| !absolute_without_parent(directory))
        {
            return Err(ServiceConfigurationError::ClaudeConfigDirectory);
        }
        Ok(Self {
            credential_namespace,
            data_root,
            port,
            claude_config_directory,
        })
    }

    /// Stage compiled into this packaged host.
    pub fn stage(&self) -> &str {
        self.credential_namespace.stage()
    }

    /// Trusted base beneath which stage and instance namespaces are created.
    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    /// Optional explicit service instance.
    pub fn instance(&self) -> Option<&str> {
        self.credential_namespace.instance()
    }

    /// The exact namespace shared by host keychain writes and gateway reads.
    pub fn credential_namespace(&self) -> &CredentialNamespace {
        &self.credential_namespace
    }

    /// Loopback port registered and probed for this service.
    pub fn port(&self) -> u16 {
        self.port
    }

    /// Claude's explicit provider configuration directory.
    pub fn claude_config_directory(&self) -> Option<&Path> {
        self.claude_config_directory.as_deref()
    }

    /// Return a replacement value for an explicit provider-setting change.
    pub fn with_claude_config_directory(
        &self,
        directory: Option<PathBuf>,
    ) -> Result<Self, ServiceConfigurationError> {
        Self::new(
            self.stage().to_owned(),
            self.data_root.clone(),
            self.instance().map(str::to_owned),
            self.port,
            directory,
        )
    }
}

fn absolute_without_parent(value: &Path) -> bool {
    value.is_absolute()
        && value
            .components()
            .all(|component| !matches!(component, Component::ParentDir))
}

/// Why persisted service settings cannot identify a safe service.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ServiceConfigurationError {
    /// The stage cannot be one namespace component.
    Stage,
    /// The instance cannot be one namespace component.
    Instance,
    /// The data root is not absolute.
    DataRoot,
    /// Port zero cannot accept a gateway connection.
    Port,
    /// Claude's configuration directory is not absolute.
    ClaudeConfigDirectory,
}

impl fmt::Display for ServiceConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Stage => "gateway stage must be one nonempty namespace segment",
            Self::Instance => "gateway instance must be one nonempty namespace segment",
            Self::DataRoot => "gateway data root must be absolute without parent traversal",
            Self::Port => "gateway port must be nonzero",
            Self::ClaudeConfigDirectory => {
                "Claude config directory must be absolute without parent traversal"
            }
        })
    }
}

impl Error for ServiceConfigurationError {}

/// Why the host requested a service reconciliation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ReconciliationCause {
    /// Ordinary launch or liveness recovery.
    Startup,
    /// The user changed Claude's explicit configuration directory.
    ClaudeConfigurationChanged,
}

#[cfg(test)]
mod tests {
    use super::*;

    fn configuration() -> ServiceConfiguration {
        ServiceConfiguration::new("prod".into(), absolute("nessa"), None, 7420, None).unwrap()
    }

    fn absolute(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\{name}"))
        } else {
            PathBuf::from(format!("/{name}"))
        }
    }

    #[test]
    fn every_path_and_namespace_is_validated_before_registration() {
        assert_eq!(
            ServiceConfiguration::new("../prod".into(), absolute("root"), None, 7420, None,),
            Err(ServiceConfigurationError::Stage)
        );
        assert_eq!(
            ServiceConfiguration::new("prod".into(), "relative".into(), None, 7420, None,),
            Err(ServiceConfigurationError::DataRoot)
        );
        assert_eq!(
            ServiceConfiguration::new(
                "prod".into(),
                absolute("root"),
                Some("../other".into()),
                7420,
                None,
            ),
            Err(ServiceConfigurationError::Instance)
        );
        assert_eq!(
            ServiceConfiguration::new("prod".into(), absolute("root"), None, 0, None),
            Err(ServiceConfigurationError::Port)
        );
        assert_eq!(
            ServiceConfiguration::new(
                "prod".into(),
                absolute("root").join("..").join("other"),
                None,
                7420,
                None,
            ),
            Err(ServiceConfigurationError::DataRoot)
        );
        assert_eq!(
            configuration().with_claude_config_directory(Some("relative".into())),
            Err(ServiceConfigurationError::ClaudeConfigDirectory)
        );
    }

    #[test]
    fn a_provider_change_produces_a_replacement_value() {
        let original = configuration();
        let changed = original
            .with_claude_config_directory(Some(absolute("claude-work")))
            .unwrap();
        assert_eq!(original.claude_config_directory(), None);
        assert_eq!(
            changed.claude_config_directory(),
            Some(absolute("claude-work").as_path())
        );
    }
}
