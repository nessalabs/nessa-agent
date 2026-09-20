use crate::attachments::domain::AttachmentError;
use nessa_sdk::domain::agent_execution::prompts::ImageMediaType;
use std::fmt;

/// A declared media type: lowercase `type/subtype`, no parameters.
///
/// Storage accepts any well-formed type. Which types a message may refer to is
/// the prompt's rule, not this one's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaType(Box<str>);
impl MediaType {
    /// Longest accepted text, in bytes.
    pub const MAX_BYTES: usize = 127;

    /// Accept exactly what the wire schema's pattern accepts; nothing is
    /// trimmed, lowercased, or stripped of parameters on the caller's behalf.
    pub fn parse(value: &str) -> Result<Self, AttachmentError> {
        let well_formed = value.len() <= Self::MAX_BYTES
            && value
                .split_once('/')
                .is_some_and(|(kind, subtype)| is_token(kind) && is_token(subtype));
        if !well_formed {
            return Err(AttachmentError::MediaType);
        }
        Ok(Self(value.into()))
    }
    /// The media type of an image a message refers to.
    pub fn of_image(image: ImageMediaType) -> Self {
        Self(image.as_str().into())
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
    /// Whether this declares an image of any encoding.
    pub fn is_image(&self) -> bool {
        self.0.starts_with("image/")
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
