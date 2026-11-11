//! The immutable identity of one saved attachment generation.
use sha2::{Digest, Sha256};

/// An opaque registration identity encoded from the minted hold generation.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ArtifactId(Box<str>);

/// An artifact identity was not canonical lowercase hexadecimal.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidArtifactId;

impl ArtifactId {
    /// Parse the canonical identity used by the attachment source.
    pub fn parse(value: &str) -> Result<Self, InvalidArtifactId> {
        if value.len() != 64
            || !value
                .bytes()
                .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        {
            return Err(InvalidArtifactId);
        }
        Ok(Self(value.into()))
    }

    /// Borrow the canonical opaque identity.
    pub fn as_str(&self) -> &str {
        &self.0
    }

    pub(crate) fn from_generation(generation: &str) -> Self {
        let mut digest = Sha256::new();
        digest.update(b"nessa attachment registration\0");
        digest.update(generation.as_bytes());
        Self(format!("{:x}", digest.finalize()).into_boxed_str())
    }
}
