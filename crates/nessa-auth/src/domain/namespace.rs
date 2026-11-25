//! Owning app namespaces for gateway resources and sessions.
//!
//! `AppNamespace::Nessa` is the built-in default app, represented as a fixed
//! enum variant compiled into the binary ([`DEFAULT_APP_NAMESPACE`] = `"nessa"`).
//! Every other installed app or external client is an [`ExtensionAppId`];
//! [`ExtensionAppId::new`] rejects `"nessa"` with
//! [`DomainError::ReservedAppNamespace`]
//! (`extension_id_rejects_reserved_default_namespace`).
use super::DomainError;

/// The built-in default app namespace compiled into the binary.
pub const DEFAULT_APP_NAMESPACE: &str = "nessa";

/// Maximum UTF-8 bytes in an extension app identifier.
pub const MAX_EXTENSION_APP_ID_BYTES: usize = 64;

/// Validated identifier for an extension app or external client.
///
/// Constrained by construction to lowercase ASCII `[a-z0-9][a-z0-9_-]*` up to
/// [`MAX_EXTENSION_APP_ID_BYTES`] bytes, and never equal to
/// [`DEFAULT_APP_NAMESPACE`] (`extension_id_rejects_reserved_default_namespace`).
#[derive(Clone, Debug, Eq, Hash, Ord, PartialEq, PartialOrd)]
pub struct ExtensionAppId(Box<str>);

impl ExtensionAppId {
    /// Validate an extension app identifier without normalizing it.
    ///
    /// # Errors
    /// Returns [`DomainError::ReservedAppNamespace`] for `"nessa"`, or
    /// [`DomainError::InvalidValue`] for empty, oversized, or disallowed text.
    pub fn new(value: impl Into<String>) -> Result<Self, DomainError> {
        let value = value.into();
        if value.is_empty() {
            return Err(DomainError::InvalidValue {
                field: "extension app id",
                reason: "must not be empty",
            });
        }
        if value.len() > MAX_EXTENSION_APP_ID_BYTES {
            return Err(DomainError::InvalidValue {
                field: "extension app id",
                reason: "is too long",
            });
        }
        let bytes = value.as_bytes();
        if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
            return Err(DomainError::InvalidValue {
                field: "extension app id",
                reason: "must start with a lowercase ASCII letter or digit",
            });
        }
        if !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-' || *byte == b'_'
        }) {
            return Err(DomainError::InvalidValue {
                field: "extension app id",
                reason:
                    "must contain only lowercase ASCII letters, digits, hyphens, or underscores",
            });
        }
        if value == DEFAULT_APP_NAMESPACE {
            return Err(DomainError::ReservedAppNamespace);
        }
        Ok(Self(value.into_boxed_str()))
    }

    /// Borrow the validated extension app identifier.
    pub fn as_str(&self) -> &str {
        &self.0
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
        ExtensionAppId::new(value).map(Self::Extension)
    }

    /// Restore a namespace from a trusted persisted or serialized value.
    ///
    /// `"nessa"` restores [`Self::Nessa`]; any other value must be a valid
    /// [`ExtensionAppId`].
    pub fn parse(value: &str) -> Result<Self, DomainError> {
        if value == DEFAULT_APP_NAMESPACE {
            return Ok(Self::Nessa);
        }
        ExtensionAppId::new(value).map(Self::Extension)
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

    #[test]
    fn default_namespace_is_hardcoded_nessa() {
        assert_eq!(AppNamespace::default(), AppNamespace::Nessa);
        assert_eq!(AppNamespace::DEFAULT.as_str(), DEFAULT_APP_NAMESPACE);
        assert!(AppNamespace::Nessa.is_default());
    }

    #[test]
    fn extension_id_rejects_reserved_default_namespace() {
        assert_eq!(
            ExtensionAppId::new("nessa"),
            Err(DomainError::ReservedAppNamespace)
        );
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
    fn rejects_invalid_extension_identifiers() {
        for bad in [
            "",
            " ",
            "Nessa",
            "Space-Junk",
            "-leading-hyphen",
            "_leading_underscore",
            "has space",
            "has/slash",
            "has:colon",
            "has\nnewline",
            "éxtension",
        ] {
            assert!(
                matches!(
                    ExtensionAppId::new(bad),
                    Err(DomainError::InvalidValue { .. })
                ),
                "expected invalid value for {bad:?}"
            );
        }
        let max = "a".repeat(MAX_EXTENSION_APP_ID_BYTES);
        assert!(ExtensionAppId::new(max).is_ok());
        let too_long = "a".repeat(MAX_EXTENSION_APP_ID_BYTES + 1);
        assert!(matches!(
            ExtensionAppId::new(too_long),
            Err(DomainError::InvalidValue { .. })
        ));
    }

    #[test]
    fn retains_only_the_validated_text() {
        let mut oversized = String::with_capacity(4096);
        oversized.push_str("tracker-1");
        let id = ExtensionAppId::new(oversized).unwrap();
        assert_eq!(String::from(id.0).capacity(), "tracker-1".len());
    }
}
