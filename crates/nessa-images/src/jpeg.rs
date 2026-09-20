//! Whether a JPEG is whole.
//!
//! The JPEG decoder this crate fits images with is forgiving on purpose, as
//! browsers are: a file cut short decodes to its top half above a grey band,
//! and damaged data decodes to whatever it happens to spell. That is no proof
//! an image is intact, and a consumer that is strict would refuse the same
//! bytes later, where nobody can see why. So a JPEG is first read once by the
//! same decoder told to be strict, which refuses data that ends early, codes
//! that do not decode, and markers where none belong.
use crate::Error;
use zune_core::{bytestream::ZCursor, options::DecoderOptions};
use zune_jpeg::JpegDecoder;

/// Refuse a JPEG that is cut short or damaged. `width` and `height` are what
/// its header was already read to say, and already inside the pixel budget; the
/// decoder is held to them, and holds one decoded copy while it checks.
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
