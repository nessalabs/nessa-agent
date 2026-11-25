/// The built-in default app namespace compiled into the binary.
pub const DEFAULT_APP_NAMESPACE: &str = "nessa";

/// Maximum UTF-8 bytes in an extension app identifier.
pub const MAX_EXTENSION_APP_ID_BYTES: usize = 64;

/// Why a candidate extension app identifier was rejected.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ExtensionAppIdError {
    /// No bytes were supplied.
    Empty,
    /// The input exceeds the byte limit.
    TooLong,
    /// The first byte is not a lowercase ASCII letter or digit.
    InvalidStart,
    /// A later byte is outside the allowed alphabet.
    InvalidCharacter,
    /// The name belongs to the built-in Nessa app.
    Reserved,
}

impl ExtensionAppIdError {
    /// Stable human-readable reason, without echoing the rejected input.
    pub const fn reason(self) -> &'static str {
        match self {
            Self::Empty => "must not be empty",
            Self::TooLong => "is too long",
            Self::InvalidStart => "must start with a lowercase ASCII letter or digit",
            Self::InvalidCharacter => {
                "must contain only lowercase ASCII letters, digits, hyphens, or underscores"
            }
            Self::Reserved => "is reserved for the built-in Nessa app",
        }
    }
}

impl std::fmt::Display for ExtensionAppIdError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.reason())
    }
}

impl std::error::Error for ExtensionAppIdError {}

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
    /// Returns [`ExtensionAppIdError::Reserved`] for `"nessa"`, or
    /// a typed validation error for empty, oversized, or disallowed text.
    pub fn new(value: impl Into<String>) -> Result<Self, ExtensionAppIdError> {
        let value = value.into();
        if value.is_empty() {
            return Err(ExtensionAppIdError::Empty);
        }
        if value.len() > MAX_EXTENSION_APP_ID_BYTES {
            return Err(ExtensionAppIdError::TooLong);
        }
        let bytes = value.as_bytes();
        if !(bytes[0].is_ascii_lowercase() || bytes[0].is_ascii_digit()) {
            return Err(ExtensionAppIdError::InvalidStart);
        }
        if !bytes.iter().all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || *byte == b'-' || *byte == b'_'
        }) {
            return Err(ExtensionAppIdError::InvalidCharacter);
        }
        if value == DEFAULT_APP_NAMESPACE {
            return Err(ExtensionAppIdError::Reserved);
        }
        Ok(Self(value.into_boxed_str()))
    }

    /// Borrow the validated extension app identifier.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extension_id_rejects_reserved_default_namespace() {
        assert_eq!(
            ExtensionAppId::new(DEFAULT_APP_NAMESPACE),
            Err(ExtensionAppIdError::Reserved)
        );
    }

    #[test]
    fn rejects_invalid_identifiers_without_normalizing() {
        use ExtensionAppIdError::{Empty, InvalidCharacter, InvalidStart, TooLong};

        for (text, error) in [
            ("", Empty),
            (" ", InvalidStart),
            ("Nessa", InvalidStart),
            ("Space-Junk", InvalidStart),
            ("-leading-hyphen", InvalidStart),
            ("_leading_underscore", InvalidStart),
            ("éxtension", InvalidStart),
            ("has space", InvalidCharacter),
            ("has/slash", InvalidCharacter),
            ("has:colon", InvalidCharacter),
            ("has\nnewline", InvalidCharacter),
        ] {
            assert_eq!(ExtensionAppId::new(text), Err(error), "{text:?}");
        }
        assert_eq!(ExtensionAppId::new("9-cal_1").unwrap().as_str(), "9-cal_1");
        let max = "a".repeat(MAX_EXTENSION_APP_ID_BYTES);
        assert_eq!(ExtensionAppId::new(max.clone()).unwrap().as_str(), max);
        let too_long = "a".repeat(MAX_EXTENSION_APP_ID_BYTES + 1);
        assert_eq!(ExtensionAppId::new(too_long), Err(TooLong));
    }

    #[test]
    fn retains_only_the_validated_text() {
        let mut oversized = String::with_capacity(4096);
        oversized.push_str("tracker-1");
        let id = ExtensionAppId::new(oversized).unwrap();
        assert_eq!(String::from(id.0).capacity(), "tracker-1".len());
    }
}
