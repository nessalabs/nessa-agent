use crate::attachments::domain::AttachmentError;
use nessa_sdk::domain::common::value_objects::{ImageMediaType, MediaType as Declared};
use std::fmt;

/// A declared media type: lowercase `type/subtype`, no parameters. The rule
/// is the SDK's [`Declared`], the one every declared file follows.
///
/// Storage accepts any well-formed type. Which types a message may refer to is
/// the prompt's rule, not this one's.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct MediaType(Declared);
impl MediaType {
    /// Longest accepted text, in bytes.
    pub const MAX_BYTES: usize = Declared::MAX_BYTES;

    /// Accept exactly what the wire schema's pattern accepts; nothing is
    /// trimmed, lowercased, or stripped of parameters on the caller's behalf.
    pub fn parse(value: &str) -> Result<Self, AttachmentError> {
        Declared::parse(value)
            .map(Self)
            .map_err(|_| AttachmentError::MediaType)
    }
    /// The media type of an image a message refers to.
    pub fn of_image(image: ImageMediaType) -> Self {
        Self(Declared::of_image(image))
    }
    pub fn as_str(&self) -> &str {
        self.0.as_str()
    }
    /// Whether this declares an image of any encoding.
    pub fn is_image(&self) -> bool {
        self.as_str().starts_with("image/")
    }
}
impl fmt::Display for MediaType {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}
