use std::{error::Error, fmt};

/// A local agent for which Nessa explicitly stores a credential.
///
/// This is deliberately narrower than the gateway's full agent identity. Codex
/// manages its own sign-in, so it cannot accidentally acquire a Nessa keychain
/// account through this type.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum CredentialAgent {
    /// Anthropic's Claude Code agent.
    Claude,
    /// The OpenCode agent.
    Opencode,
}

impl CredentialAgent {
    /// The canonical protocol name used by infrastructure mappings.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Claude => "claude",
            Self::Opencode => "opencode",
        }
    }
}

/// The durable stage and optional instance that scope Nessa-owned credentials.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CredentialNamespace {
    stage: String,
    instance: Option<String>,
}

impl CredentialNamespace {
    /// Build a namespace whose values are each one safe account component.
    ///
    /// # Errors
    /// Returns [`CredentialNamespaceError`] when either supplied value is
    /// empty or contains anything except ASCII letters, digits, `-`, and `_`.
    pub fn new(stage: String, instance: Option<String>) -> Result<Self, CredentialNamespaceError> {
        if !segment(&stage) || instance.as_deref().is_some_and(|value| !segment(value)) {
            return Err(CredentialNamespaceError);
        }
        Ok(Self { stage, instance })
    }

    /// The packaged service stage.
    pub fn stage(&self) -> &str {
        &self.stage
    }

    /// The explicit service instance, when this is not the default instance.
    pub fn instance(&self) -> Option<&str> {
        self.instance.as_deref()
    }

    /// Scope one canonical keychain item to this stage and instance.
    ///
    /// # Errors
    /// Returns [`CredentialNamespaceError`] when `item` is not one safe
    /// account component. Infrastructure parses this from the bundled mapping
    /// before calling this method.
    pub fn account(&self, item: &str) -> Result<String, CredentialNamespaceError> {
        if !segment(item) {
            return Err(CredentialNamespaceError);
        }
        Ok(match &self.instance {
            Some(instance) => format!("{}:{instance}:{item}", self.stage),
            None => format!("{}:{item}", self.stage),
        })
    }
}

fn segment(value: &str) -> bool {
    !value.is_empty()
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}

/// A credential namespace or mapped item is not one safe account component.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CredentialNamespaceError;

impl fmt::Display for CredentialNamespaceError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("credential namespace must contain safe nonempty components")
    }
}

impl Error for CredentialNamespaceError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn account_names_are_scoped_once_by_stage_and_instance() {
        let namespace = CredentialNamespace::new("ci".into(), Some("one".into())).unwrap();
        assert_eq!(
            namespace.account("claude-api-key").unwrap(),
            "ci:one:claude-api-key"
        );
        assert_eq!(namespace.stage(), "ci");
        assert_eq!(namespace.instance(), Some("one"));
    }

    #[test]
    fn unsafe_namespace_and_item_components_are_refused() {
        assert_eq!(
            CredentialNamespace::new("../prod".into(), None),
            Err(CredentialNamespaceError)
        );
        assert_eq!(
            CredentialNamespace::new("prod".into(), Some("".into())),
            Err(CredentialNamespaceError)
        );
        let namespace = CredentialNamespace::new("prod".into(), None).unwrap();
        assert_eq!(namespace.account("../key"), Err(CredentialNamespaceError));
    }
}
