//! What the consumer of an image accepts.
//!
//! [`Limits`] is the caller's half of the bargain, and this crate invents none
//! of it: the encodings the consumer takes, the largest result in bytes, and the
//! longest edge worth sending. [`Encoding`] names the four encodings that can be
//! accepted; only PNG and JPEG are ever produced, which is why [`Limits::new`]
//! insists that one of those two is among them.
use std::{error, fmt};

/// An image encoding a consumer may accept unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Encoding {
    /// `image/png`
    Png,
    /// `image/jpeg`
    Jpeg,
    /// `image/gif`
    Gif,
    /// `image/webp`
    Webp,
}
impl Encoding {
    /// The media type as written in a content header.
    pub fn media_type(self) -> &'static str {
        match self {
            Self::Png => "image/png",
            Self::Jpeg => "image/jpeg",
            Self::Gif => "image/gif",
            Self::Webp => "image/webp",
        }
    }
}

/// The limits were contradictory or empty.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LimitsError {
    /// A byte or pixel limit was zero.
    Zero,
    /// Neither PNG nor JPEG is accepted, so nothing could be produced when an
    /// image needs re-encoding.
    NoOutputEncoding,
}
impl fmt::Display for LimitsError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Zero => "image limits must be positive",
            Self::NoOutputEncoding => "image limits must accept PNG or JPEG",
        })
    }
}
impl error::Error for LimitsError {}

/// What the consumer of an image accepts. Immutable once built.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Limits {
    accepted: Vec<Encoding>,
    max_bytes: u64,
    max_long_edge_px: u32,
}
impl Limits {
    /// - `accepted`: encodings the consumer takes. An image already in one of
    ///   them, and inside the other two limits, passes through untouched. It must
    ///   include PNG or JPEG, the only encodings produced.
    /// - `max_bytes`: largest result, in bytes as stored. A consumer that
    ///   measures base64 text passes three quarters of that figure.
    /// - `max_long_edge_px`: longest width or height worth sending. Larger
    ///   images are scaled down to it; smaller ones are never scaled up.
    pub fn new(
        accepted: Vec<Encoding>,
        max_bytes: u64,
        max_long_edge_px: u32,
    ) -> Result<Self, LimitsError> {
        if max_bytes == 0 || max_long_edge_px == 0 {
            return Err(LimitsError::Zero);
        }
        if !accepted.contains(&Encoding::Png) && !accepted.contains(&Encoding::Jpeg) {
            return Err(LimitsError::NoOutputEncoding);
        }
        Ok(Self {
            accepted,
            max_bytes,
            max_long_edge_px,
        })
    }
    /// Whether the consumer takes `encoding` unchanged.
    pub fn accepts(&self, encoding: Encoding) -> bool {
        self.accepted.contains(&encoding)
    }
    /// Largest result in bytes.
    pub fn max_bytes(&self) -> u64 {
        self.max_bytes
    }
    /// Longest width or height worth sending.
    pub fn max_long_edge_px(&self) -> u32 {
        self.max_long_edge_px
    }
}
