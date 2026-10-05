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
        claude_config_directory_is_durable(claude_config_directory.as_deref())?;
        Ok(Self {
            namespace,
            data_root,
            port,
            claude_config_directory,
        })
    }

    /// A new configuration with only the Claude directory replaced.
    ///
    /// The directory rule stays in [`claude_config_directory_is_durable`]; this
    /// rebuilds through [`Self::new`] so a caller cannot store a path that
    /// construction would have refused.
    #[cfg(any(test, target_os = "macos", target_os = "linux"))]
    pub fn with_claude_config_directory(
        self,
        claude_config_directory: Option<PathBuf>,
    ) -> Result<Self, ServiceConfigurationError> {
        let stage = self.stage().to_owned();
        let instance = self.instance().map(str::to_owned);
        Self::new(
            stage,
            self.data_root,
            instance,
            self.port,
            claude_config_directory,
        )
    }

    /// The value to store when the Claude directory becomes `directory`.
    ///
    /// `Ok(None)` means that directory is already published, so the caller must
    /// not reconcile. Validity stays in [`Self::new`].
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn replacing_claude_config_directory(
        &self,
        directory: Option<PathBuf>,
    ) -> Result<Option<Self>, ServiceConfigurationError> {
        if self.claude_config_directory == directory {
            return Ok(None);
        }
        self.clone()
            .with_claude_config_directory(directory)
            .map(Some)
    }

    /// The value to store when a failed reconciliation still owns `expected`.
    ///
    /// `Ok(None)` means the live directory is no longer `expected`, or it is
    /// already `previous`, so the caller must not write. A newer settings save
    /// keeps the directory it published
    /// (`a_failed_registration_does_not_restore_a_newer_directory`).
    #[cfg(any(test, target_os = "macos", target_os = "linux"))]
    pub fn restoring_claude_config_directory(
        &self,
        expected: &Option<PathBuf>,
        previous: Option<PathBuf>,
    ) -> Result<Option<Self>, ServiceConfigurationError> {
        if &self.claude_config_directory != expected || self.claude_config_directory == previous {
            return Ok(None);
        }
        self.clone()
            .with_claude_config_directory(previous)
            .map(Some)
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

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn port(&self) -> u16 {
        self.port
    }

    #[cfg(any(target_os = "macos", target_os = "linux"))]
    pub fn claude_config_directory(&self) -> Option<&Path> {
        self.claude_config_directory.as_deref()
    }
}

/// Whether a Claude configuration directory is durable service input.
///
/// `None` is the provider default. A present path must be absolute and free of
/// `.` and `..` components, the same rule [`ServiceConfiguration::new`] applies.
pub fn claude_config_directory_is_durable(
    directory: Option<&Path>,
) -> Result<(), ServiceConfigurationError> {
    if directory.is_some_and(|directory| !normalized_absolute(directory)) {
        Err(ServiceConfigurationError::ClaudeConfigDirectory)
    } else {
        Ok(())
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

    #[test]
    fn restoring_writes_the_previous_directory_only_while_the_attempt_still_owns_it() {
        let original =
            ServiceConfiguration::new("prod".into(), absolute("nessa"), None, 7420, None).unwrap();
        let updated = original
            .clone()
            .with_claude_config_directory(Some(absolute("claude-a")))
            .unwrap();
        let newer = updated
            .clone()
            .with_claude_config_directory(Some(absolute("claude-b")))
            .unwrap();

        let restored = updated
            .restoring_claude_config_directory(&Some(absolute("claude-a")), None)
            .unwrap()
            .unwrap();
        assert_eq!(restored.claude_config_directory, None);
        assert!(newer
            .restoring_claude_config_directory(&Some(absolute("claude-a")), None)
            .unwrap()
            .is_none());
        assert!(updated
            .restoring_claude_config_directory(
                &Some(absolute("claude-a")),
                Some(absolute("claude-a"))
            )
            .unwrap()
            .is_none());
    }
}
