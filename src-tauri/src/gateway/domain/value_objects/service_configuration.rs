use std::{
    error::Error,
    fmt,
    path::{Component, Path, PathBuf},
};

use nessa_agent_credentials::{CredentialNamespace, CredentialNamespaceError};

/// Validated durable inputs for one packaged gateway service definition.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServiceConfiguration {
    namespace: CredentialNamespace,
    data_root: PathBuf,
    port: u16,
    claude_config_directory: Option<PathBuf>,
}

impl ServiceConfiguration {
    pub fn new(
        stage: String,
        data_root: PathBuf,
        instance: Option<String>,
        port: u16,
        claude_config_directory: Option<PathBuf>,
    ) -> Result<Self, ServiceConfigurationError> {
        let namespace = CredentialNamespace::new(stage, instance)
            .map_err(ServiceConfigurationError::Namespace)?;
        if !normalized_absolute(&data_root) {
            return Err(ServiceConfigurationError::DataRoot);
        }
        if port == 0 {
            return Err(ServiceConfigurationError::Port);
        }
        if claude_config_directory
            .as_deref()
            .is_some_and(|directory| !normalized_absolute(directory))
        {
            return Err(ServiceConfigurationError::ClaudeConfigDirectory);
        }
        Ok(Self {
            namespace,
            data_root,
            port,
            claude_config_directory,
        })
    }

    pub fn stage(&self) -> &str {
        self.namespace.stage()
    }

    pub fn instance(&self) -> Option<&str> {
        self.namespace.instance()
    }

    pub fn credential_namespace(&self) -> &CredentialNamespace {
        &self.namespace
    }

    pub fn data_root(&self) -> &Path {
        &self.data_root
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn claude_config_directory(&self) -> Option<&Path> {
        self.claude_config_directory.as_deref()
    }
}

fn normalized_absolute(path: &Path) -> bool {
    path.is_absolute()
        && path
            .components()
            .all(|component| !matches!(component, Component::CurDir | Component::ParentDir))
}

#[derive(Debug)]
pub enum ServiceConfigurationError {
    Namespace(CredentialNamespaceError),
    DataRoot,
    Port,
    ClaudeConfigDirectory,
}

impl fmt::Display for ServiceConfigurationError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Namespace(_) => "gateway namespace must contain safe nonempty components",
            Self::DataRoot => "gateway data root must be absolute and normalized",
            Self::Port => "gateway port must be nonzero",
            Self::ClaudeConfigDirectory => {
                "Claude config directory must be absolute and normalized"
            }
        })
    }
}

impl Error for ServiceConfigurationError {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        match self {
            Self::Namespace(source) => Some(source),
            Self::DataRoot | Self::Port | Self::ClaudeConfigDirectory => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn absolute(name: &str) -> PathBuf {
        if cfg!(windows) {
            PathBuf::from(format!(r"C:\{name}"))
        } else {
            PathBuf::from(format!("/{name}"))
        }
    }

    #[test]
    fn rejects_non_durable_service_inputs() {
        assert!(
            ServiceConfiguration::new("../prod".into(), absolute("nessa"), None, 7420, None,)
                .is_err()
        );
        assert!(
            ServiceConfiguration::new("prod".into(), "relative".into(), None, 7420, None,).is_err()
        );
        assert!(ServiceConfiguration::new(
            "prod".into(),
            absolute("nessa"),
            None,
            7420,
            Some("relative".into()),
        )
        .is_err());
    }
}
