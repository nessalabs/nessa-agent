//! A declared media type: what a file says it is, as a lowercase
//! `type/subtype`.
#![deny(missing_docs)]

use super::ImageMediaType;
use std::{error::Error, fmt};

/// The text is not a lowercase `type/subtype` of at most
/// [`MediaType::MAX_BYTES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct MediaTypeError;
impl fmt::Display for MediaTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            f,
            "media type must be a lowercase `type/subtype` of at most {} bytes",
            MediaType::MAX_BYTES
        )
    }
}
impl Error for MediaTypeError {}

/// A declared media type: lowercase `type/subtype`, no parameters. Each half
/// starts with a lowercase letter or digit and holds only lowercase letters,
/// digits and `!#$&^_.+-`.
///
/// It is what a file was declared as, not what its bytes are; the feature that
/// accepts one decides which types it allows.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaType(Box<str>);
impl MediaType {
    /// Longest accepted text, in bytes.
    pub const MAX_BYTES: usize = 127;

    /// Accept exactly a well-formed type; nothing is trimmed, lowercased, or
    /// stripped of parameters on the caller's behalf.
    pub fn parse(value: &str) -> Result<Self, MediaTypeError> {
        let well_formed = value.len() <= Self::MAX_BYTES
            && value
                .split_once('/')
                .is_some_and(|(kind, subtype)| is_token(kind) && is_token(subtype));
        if !well_formed {
            return Err(MediaTypeError);
        }
        Ok(Self(value.into()))
    }
    /// The media type of an image encoding.
    pub fn of_image(image: ImageMediaType) -> Self {
        Self(image.as_str().into())
    }
    /// The text as given.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(&self.0)
    }
}

/// One half of a media type: a lowercase alphanumeric, then restricted-name characters.
fn is_token(value: &str) -> bool {
    let mut bytes = value.bytes();
    bytes
        .next()
        .is_some_and(|first| first.is_ascii_lowercase() || first.is_ascii_digit())
        && bytes.all(|byte| {
            byte.is_ascii_lowercase() || byte.is_ascii_digit() || b"!#$&^_.+-".contains(&byte)
        })
}
