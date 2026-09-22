use std::{error::Error, fmt};

/// The provider meaning of a credential's private text.
///
/// Provider adapters must preserve this distinction when selecting the child
/// process environment variable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum AgentCredentialKind {
    /// A provider API key.
    ApiKey,
    /// A vendor OAuth token.
    OAuthToken,
}

/// One credential that is safe to represent at every supported launch boundary.
///
/// The text stays private and is never serialized or used as diagnostic text.
/// The operating system, allocator, and provider APIs can retain transient
/// memory copies, so this value does not claim secure erasure.
pub struct AgentCredential {
    kind: AgentCredentialKind,
    secret: String,
}

impl AgentCredential {
    /// Build a bounded, nonblank plain-text credential.
    ///
    /// The exact text is retained; surrounding whitespace is not trimmed or
    /// otherwise changed.
    ///
    /// # Errors
    /// Returns [`AgentCredentialError`] when `secret` is empty, larger than 16
    /// KiB, not UTF-8, entirely whitespace, or contains a control character.
    pub fn new(kind: AgentCredentialKind, secret: Vec<u8>) -> Result<Self, AgentCredentialError> {
        if secret.is_empty() || secret.len() > 16 * 1024 {
            return Err(AgentCredentialError);
        }
        let secret = String::from_utf8(secret).map_err(|_| AgentCredentialError)?;
        if secret.chars().all(char::is_whitespace) || secret.chars().any(char::is_control) {
            return Err(AgentCredentialError);
        }
        Ok(Self { kind, secret })
    }

    /// The environment meaning the provider adapter must preserve.
    pub const fn kind(&self) -> AgentCredentialKind {
        self.kind
    }

    /// Borrow the private text only at the keychain or process-launch boundary.
    pub fn expose(&self) -> &str {
        self.secret.as_str()
    }
}

/// A candidate credential cannot be represented consistently at every boundary.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct AgentCredentialError;

impl fmt::Display for AgentCredentialError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("agent credential must be bounded nonblank plain text")
    }
}

impl Error for AgentCredentialError {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn credentials_accept_only_bounded_plain_utf8_text() {
        for secret in [
            Vec::new(),
            b"line\nbreak".to_vec(),
            b"   ".to_vec(),
            vec![0xff],
        ] {
            assert_eq!(
                AgentCredential::new(AgentCredentialKind::ApiKey, secret).err(),
                Some(AgentCredentialError)
            );
        }
        assert_eq!(
            AgentCredential::new(AgentCredentialKind::ApiKey, vec![b'x'; 16 * 1024 + 1]).err(),
            Some(AgentCredentialError)
        );
    }

    #[test]
    fn a_valid_credential_preserves_its_kind_and_exact_text() {
        let credential =
            AgentCredential::new(AgentCredentialKind::OAuthToken, b" token ".to_vec()).unwrap();

        assert_eq!(credential.kind(), AgentCredentialKind::OAuthToken);
        assert_eq!(credential.expose(), " token ");
    }
}
