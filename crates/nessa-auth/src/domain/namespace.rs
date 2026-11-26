//! Owning app namespaces for gateway resources and sessions.
//!
//! `AppNamespace::Nessa` is the built-in default app, represented as a fixed
//! enum variant compiled into the binary ([`DEFAULT_APP_NAMESPACE`] = `"nessa"`).
//! Every other installed app or external client is an [`ExtensionAppId`];
//! [`ExtensionAppId::new`] rejects `"nessa"` with
//! [`ExtensionAppIdError::Reserved`]; namespace construction translates it to
//! [`DomainError::ReservedAppNamespace`].
use super::DomainError;
use nessa_types::app_identity::{ExtensionAppId, ExtensionAppIdError, DEFAULT_APP_NAMESPACE};

fn namespace_error(error: ExtensionAppIdError) -> DomainError {
    match error {
        ExtensionAppIdError::Reserved => DomainError::ReservedAppNamespace,
        ExtensionAppIdError::Empty
        | ExtensionAppIdError::TooLong
        | ExtensionAppIdError::InvalidStart
        | ExtensionAppIdError::InvalidCharacter => DomainError::InvalidValue {
            field: "extension app id",
            reason: error.reason(),
        },
    }
}

/// The owning app namespace of a gateway resource, principal, or session.
///
/// [`Self::Nessa`] is the built-in default app, compiled into the binary and
/// never configurable. Every other app or external client is
/// [`Self::Extension`].
#[derive(Clone, Debug, Default, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub enum AppNamespace {
    /// The built-in default Nessa app, compiled into the binary.
    #[default]
    Nessa,
    /// An installed app or external SDK client.
    Extension(ExtensionAppId),
}

impl AppNamespace {
    /// The hardcoded built-in default app namespace.
    pub const DEFAULT: Self = Self::Nessa;

    /// Construct an extension namespace. Cannot construct [`Self::Nessa`]
    /// (`extension_constructor_cannot_claim_default_namespace`).
    pub fn extension(value: impl Into<String>) -> Result<Self, DomainError> {
        ExtensionAppId::new(value)
            .map(Self::Extension)
            .map_err(namespace_error)
    }

    /// Restore a namespace from a trusted persisted or serialized value.
    ///
    /// `"nessa"` restores [`Self::Nessa`]; any other value must be a valid
    /// [`ExtensionAppId`].
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        if value == DEFAULT_APP_NAMESPACE {
            return Ok(Self::Nessa);
        }
        ExtensionAppId::new(value)
            .map(Self::Extension)
            .map_err(namespace_error)
    }

    /// Canonical wire and storage spelling of this namespace.
    pub fn as_str(&self) -> &str {
        match self {
            Self::Nessa => DEFAULT_APP_NAMESPACE,
            Self::Extension(id) => id.as_str(),
        }
    }

    /// Whether this is the built-in default Nessa app namespace.
    pub fn is_default(&self) -> bool {
        matches!(self, Self::Nessa)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_types::app_identity::MAX_EXTENSION_APP_ID_BYTES;

    #[test]
    fn default_namespace_is_hardcoded_nessa() {
        assert_eq!(AppNamespace::default(), AppNamespace::Nessa);
        assert_eq!(AppNamespace::DEFAULT.as_str(), DEFAULT_APP_NAMESPACE);
        assert!(AppNamespace::Nessa.is_default());
    }

    #[test]
    fn extension_constructor_cannot_claim_default_namespace() {
        assert_eq!(
            AppNamespace::extension("nessa"),
            Err(DomainError::ReservedAppNamespace)
        );
    }

    #[test]
    fn parse_restores_default_and_valid_extensions() {
        assert_eq!(AppNamespace::parse("nessa").unwrap(), AppNamespace::Nessa);
        let ext = AppNamespace::parse("space-junk_1").unwrap();
        assert!(!ext.is_default());
        assert_eq!(ext.as_str(), "space-junk_1");
    }

    #[test]
    fn invalid_namespace_preserves_the_auth_error_for_both_constructors() {
        for (text, reason) in [
            ("", "must not be empty"),
            (
                "Calendar",
                "must start with a lowercase ASCII letter or digit",
            ),
            (
                "calendar/",
                "must contain only lowercase ASCII letters, digits, hyphens, or underscores",
            ),
        ] {
            let expected = DomainError::InvalidValue {
                field: "extension app id",
                reason,
            };
            assert_eq!(AppNamespace::extension(text), Err(expected.clone()));
            assert_eq!(AppNamespace::parse(text), Err(expected));
        }
        assert_eq!(
            AppNamespace::extension("a".repeat(MAX_EXTENSION_APP_ID_BYTES + 1)),
            Err(DomainError::InvalidValue {
                field: "extension app id",
                reason: "is too long",
            })
        );
    }
}
