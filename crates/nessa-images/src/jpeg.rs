//! Whether a JPEG is whole.
//!
//! The JPEG decoder this crate fits images with is forgiving on purpose, as
//! browsers are: a file cut short decodes to its top half above a grey band,
//! and damaged data decodes to whatever it happens to spell. That is no proof
//! an image is intact, and a consumer that is strict would refuse the same
//! bytes later, where nobody can see why. So a JPEG is first read once by the
//! same decoder told to be strict, which refuses data that ends early, codes
//! that do not decode, and markers where none belong.
//!
//! Strictness is a licence to hand bytes back unchanged and nothing more. A
//! JPEG that is going to be re-encoded is read by the forgiving decoder like
//! every other encoding, so what is refused here can only ever cost an image
//! its passage through untouched. The tests hold the strict decoder to the
//! shapes real cameras and editors write, which it must keep accepting.
use crate::Error;
use zune_core::{bytestream::ZCursor, options::DecoderOptions};
use zune_jpeg::JpegDecoder;

/// Whether these exact bytes may be handed back untouched: a JPEG that is cut
/// short or damaged may not. `width` and `height` are what its header was
/// already read to say, and already inside the pixel budget; the decoder is
/// held to them, and holds one decoded copy while it checks.
///
/// # Errors
///
/// [`Error::Undecodable`] when the strict decoder will not read the file to its
/// end. That is a refusal to vouch for the bytes, not a verdict that no decoder
/// could show a picture, so only the untouched return may depend on it.
pub(crate) fn check_whole(input: &[u8], width: u32, height: u32) -> Result<(), Error> {
    let options = DecoderOptions::default()
        .set_strict_mode(true)
        .set_max_width(width as usize)
        .set_max_height(height as usize);
    JpegDecoder::new_with_options(ZCursor::new(input), options)
        .decode()
        .map(drop)
        .map_err(|_| Error::Undecodable)
}

#[cfg(test)]
#[path = "../tests/unit/jpeg.rs"]
mod tests;
