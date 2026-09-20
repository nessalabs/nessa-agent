//! The port a decoder outside this crate answers on, and what it answers with.
//!
//! [`PlatformDecoder`] is implemented by an adapter around a system decoder, and
//! by a test's substitute; [`DecodedImage`] is what one hands back. The crate
//! documentation says which encodings reach one and what stands in front of it.
use crate::Error;
use image::RgbaImage;

/// Pixels a [`PlatformDecoder`] produced: upright, eight bits a channel, with
/// straight (not premultiplied) alpha.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DecodedImage {
    /// The pixels, already turned upright.
    pub pixels: RgbaImage,
    /// Whether the source can hold exact pixels, as a screenshot does. HEIC and
    /// camera RAW are photographs and answer false.
    pub lossless: bool,
}

/// A decoder this crate does not contain, asked only for encodings its own
/// decoders cannot read.
///
/// An implementation wraps a system decoder and is handed the bytes in memory;
/// it opens no file and reaches no network. [`crate::platform_decoder`] names
/// the one for the running system, and a test substitutes its own.
pub trait PlatformDecoder: Send + Sync {
    /// Decode the first image in `input`, upright, with its long edge at most
    /// `max_long_edge_px`. Scaling here rather than afterwards is what keeps a
    /// hundred-megapixel RAW from being held whole in memory.
    ///
    /// [`Error::UnsupportedEncoding`] when the system cannot read it either.
    fn decode(&self, input: &[u8], max_long_edge_px: u32) -> Result<DecodedImage, Error>;
}
