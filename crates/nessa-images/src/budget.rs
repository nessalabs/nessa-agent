//! How much this crate will hold in memory for one image, in one place, so its
//! own decoders and a system decoder are held to the same numbers.
use crate::Error;

/// Most pixels decoded. A larger image is refused before any pixel is read.
///
/// A pixel count rather than an edge, because memory follows the count: a
/// 16,384 px square is 268 megapixels, and a panorama with one long edge is
/// not. One hundred million covers the largest cameras people actually attach
/// from (61 and 100 megapixel sensors) with nothing above them.
pub(crate) const MAX_DECODE_PIXELS: u64 = 100_000_000;

/// Most memory fitting one image may hold at once: the decoded pixels and every
/// working copy made from them, counted before anything is decoded.
///
/// A 100 megapixel photograph at eight bits a channel needs about 0.6 GiB to be
/// decoded and scaled down, and fits. The same pixel count at sixteen bits or as
/// floating point does not, and is refused rather than attempted. What a decoder
/// allocates for itself while it works is outside this count; those that accept
/// a limit of their own are given one.
pub(crate) const MAX_WORKING_BYTES: u64 = 768 * 1024 * 1024;

/// Longest edge ever asked of a system decoder, whatever the caller's limit.
/// Its result is held here as four bytes a pixel, so the request is bounded
/// here too and not only by the caller. No consumer this crate has met takes
/// an edge over 8,000 px.
pub(crate) const MAX_PLATFORM_LONG_EDGE_PX: u32 = 8_192;

/// Refuse an image of `width` by `height` that holds more pixels than are ever
/// decoded. The product of two `u32` cannot overflow a `u64`.
pub(crate) fn check_pixels(width: u32, height: u32) -> Result<(), Error> {
    if u64::from(width) * u64::from(height) > MAX_DECODE_PIXELS {
        return Err(Error::TooLargeToDecode);
    }
    Ok(())
}

/// Refuse work whose peak memory, counted by the caller, is over the budget.
pub(crate) fn check_working_bytes(bytes: u64) -> Result<(), Error> {
    if bytes > MAX_WORKING_BYTES {
        return Err(Error::TooLargeToDecode);
    }
    Ok(())
}
