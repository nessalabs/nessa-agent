use std::{error, fmt};

/// Why an image could not be fitted. Nothing is returned alongside an error:
/// there is no partially fitted image.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Error {
    /// The bytes are not an image in an encoding this crate or the running
    /// system's decoder reads. Without a system decoder that includes HEIC,
    /// AVIF, and camera RAW.
    UnsupportedEncoding,
    /// The bytes claim an encoding but do not decode as it: truncated or corrupt.
    Undecodable,
    /// The image is larger than this crate will decode, in bytes or in pixels.
    /// This bounds memory before any pixel is read; it is not a consumer limit.
    TooLargeToDecode,
    /// No readable version of the image fits the byte limit. Scaling stops at a
    /// floor rather than returning something too small to make out.
    CannotFit,
}
impl fmt::Display for Error {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::UnsupportedEncoding => "not an image in a readable encoding",
            Self::Undecodable => "the image is truncated or corrupt",
            Self::TooLargeToDecode => "the image is too large to decode",
            Self::CannotFit => "no readable version of the image fits the byte limit",
        })
    }
}
impl error::Error for Error {}
