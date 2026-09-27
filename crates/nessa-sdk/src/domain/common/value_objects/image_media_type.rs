//! The closed set of image encodings Nessa names.
#![deny(missing_docs)]

use std::{error::Error, fmt};

/// The text is not one of the supported image media types, written exactly.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ImageMediaTypeError;
impl fmt::Display for ImageMediaTypeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "unsupported image media type")
    }
}
impl Error for ImageMediaTypeError {}

/// Image encodings Nessa can name. A closed set shared by what a user message
/// refers to and what a model is published to accept; a feature narrows it, and
/// an encoding is added here only once something can produce and deliver it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum ImageMediaType {
    /// `image/png`
    Png,
    /// `image/jpeg`
    Jpeg,
    /// `image/gif`
    Gif,
    /// `image/webp`
    Webp,
}
impl ImageMediaType {
    /// Every encoding, in declaration order.
    pub const ALL: [Self; 4] = [Self::Png, Self::Jpeg, Self::Gif, Self::Webp];

    /// Parse the exact lowercase media type. Parameters, other casing, and
    /// every other type are rejected rather than normalized.
    pub fn parse(value: &str) -> Result<Self, ImageMediaTypeError> {
        Self::ALL
            .into_iter()
            .find(|kind| kind.as_str() == value)
            .ok_or(ImageMediaTypeError)
    }
    /// The media type as written in a content header.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}
