//! The pipeline itself: decide whether an image can be handed back untouched,
//! and otherwise decode it, turn it upright, scale it, and encode it until it
//! is inside the consumer's limits.
//!
//! ```text
//! normalize_with ─► header only: size, orientation, pixel and memory budget
//!        │
//!        ├─ acceptable as it is ─► proved to decode, one frame ─► the same bytes
//!        │
//!        └─ otherwise ─► decode ─► fit: scale ► turn ► encode ─► fits? ─► bytes
//!                                        ▲                          │
//!                                        └──── one step smaller ◄── no
//! ```
//!
//! Arrows are the order of work. Memory is counted before each step that
//! allocates: once from the header for the decode and the first pass, and again
//! before every smaller size, so nothing is charged for work that is not done.
use crate::{
    budget::{check_pixels, check_working_bytes, MAX_PLATFORM_LONG_EDGE_PX},
    jpeg::check_whole,
    platform_decoder,
    sniff::{names_a_system_encoding, tiff_holds_only_a_preview},
    Encoding, Error, Limits, PlatformDecoder,
};
use image::{
    codecs::{
        gif::GifDecoder,
        jpeg::JpegEncoder,
        png::{PngDecoder, PngEncoder},
        webp::WebPDecoder,
    },
    imageops::FilterType,
    metadata::Orientation,
    AnimationDecoder, ColorType, DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader,
    Limits as DecoderLimits, RgbImage, RgbaImage,
};
use std::{borrow::Cow, io::Cursor};

/// Largest input read at all. Beyond this the bytes are not even sniffed.
const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// Most memory a decoder may allocate for itself, for the decoders that accept
/// such a limit (PNG, JPEG, GIF, TIFF). The others are bounded by what is
/// counted before they run: see [`working_bytes`].
const MAX_DECODER_ALLOC_BYTES: u64 = 512 * 1024 * 1024;
/// JPEG qualities tried at each size, best first. Below the last, text in a
/// screenshot stops being legible, so the image is scaled down instead.
const JPEG_QUALITIES: [u8; 3] = [90, 80, 70];
/// Each failed size is followed by one this fraction of it.
const SCALE_STEP: f64 = 0.8;
/// Scaling stops once the long edge would fall below this: smaller than it, a
/// result would fit and show nothing.
const MIN_LONG_EDGE_PX: u32 = 256;
/// Bytes a pixel in the buffer the scaler holds between its two passes: four
/// channels of `f32`, whatever the source was.
const SCALER_BYTES_PER_PIXEL: u64 = 16;

/// What kind of picture the bytes held, which decides the encoding tried first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Source {
    /// Exact pixels, as a screenshot or a diagram has: PNG keeps them exact.
    Exact,
    /// Already lossy, or a photograph: JPEG, since PNG would be several times
    /// the size for nothing.
    Photograph,
}

/// An image inside every limit it was fitted to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Normalized {
    /// The image, ready to hand to the consumer.
    pub bytes: Vec<u8>,
    /// How `bytes` is encoded.
    pub encoding: Encoding,
    /// Width in pixels, as the consumer will see it.
    pub width: u32,
    /// Height in pixels, as the consumer will see it.
    pub height: u32,
    /// False when `bytes` is the input, byte for byte.
    pub changed: bool,
}

/// Fit `input` to `limits`, reading what this crate cannot with the running
/// system's decoder. See the crate documentation for the rules.
///
/// This decodes and encodes on the calling thread and can take hundreds of
/// milliseconds for a large image; an async caller runs it on a blocking thread.
/// An image that passes through unchanged is still decoded once, to prove that
/// it is one.
pub fn normalize(input: &[u8], limits: &Limits) -> Result<Normalized, Error> {
    normalize_with(input, limits, platform_decoder())
}

/// [`normalize`] with the platform decoder chosen by the caller: a substitute in
/// a test, or `None` to read only what this crate reads itself.
///
/// A platform decoder is asked only about bytes that begin like one of the
/// encodings the crate documentation hands to it. Anything else, a PDF or an
/// archive or text, is [`Error::UnsupportedEncoding`] without asking.
pub fn normalize_with(
    input: &[u8],
    limits: &Limits,
    platform: Option<&dyn PlatformDecoder>,
) -> Result<Normalized, Error> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(Error::TooLargeToDecode);
    }
    let mut reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| Error::UnsupportedEncoding)?;
    let source = match reader.format() {
        Some(ImageFormat::Png) => Some(Encoding::Png),
        Some(ImageFormat::Jpeg) => Some(Encoding::Jpeg),
        Some(ImageFormat::Gif) => Some(Encoding::Gif),
        Some(ImageFormat::WebP) => Some(Encoding::Webp),
        // Most camera RAW files are TIFF containers whose first image is a small
        // preview. Reading one as a TIFF would quietly return that preview.
        Some(ImageFormat::Tiff) if tiff_holds_only_a_preview(input) => {
            return fit_with_platform(input, limits, platform);
        }
        Some(
            ImageFormat::Tiff
            | ImageFormat::Bmp
            | ImageFormat::Ico
            | ImageFormat::Qoi
            | ImageFormat::Pnm
            | ImageFormat::Hdr,
        ) => None,
        // HEIC, AVIF, JPEG XL, PSD, and camera RAW: what only the system reads.
        _ if names_a_system_encoding(input) => return fit_with_platform(input, limits, platform),
        _ => return Err(Error::UnsupportedEncoding),
    };
    let mut decode_limits = DecoderLimits::default();
    decode_limits.max_alloc = Some(MAX_DECODER_ALLOC_BYTES);
    reader.limits(decode_limits);
    // This reads the header and no pixels.
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(Error::Undecodable);
    }
    check_pixels(width, height)?;
    let decoded_bytes = decoder.total_bytes();
    check_working_bytes(decoded_bytes)?;
    // A decoder that cannot read orientation has none recorded to apply.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);
    let inside_every_limit = orientation == Orientation::NoTransforms
        && input.len() as u64 <= limits.max_bytes()
        && width.max(height) <= limits.max_long_edge_px();
    let untouched = source
        .filter(|encoding| limits.accepts(*encoding) && inside_every_limit)
        // Only the first frame of an animation is kept, so an animation is
        // never handed back as it came.
        .filter(|encoding| !is_animated(*encoding, input));
    match untouched {
        // A header is not an image, so the bytes are proved to be one before
        // they are handed back. A JPEG is proved strictly, because its decoder
        // below shows a file cut short as its top half. One that fails that
        // proof may still be a picture every viewer shows (CMYK, a stray
        // marker); it is decoded and written again rather than refused.
        Some(Encoding::Jpeg) if check_whole(input, width, height).is_ok() => {
            return Ok(unchanged(input, Encoding::Jpeg, width, height));
        }
        Some(Encoding::Jpeg) | None => {}
        Some(encoding) => {
            DynamicImage::from_decoder(decoder).map_err(decode_error)?;
            return Ok(unchanged(input, encoding, width, height));
        }
    }

    check_working_bytes(working_bytes(
        (width, height),
        decoded_bytes,
        decoder.color_type(),
        orientation,
        limits.max_long_edge_px(),
    ))?;
    let pixels = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
    let kind = if source == Some(Encoding::Jpeg) {
        Source::Photograph
    } else {
        Source::Exact
    };
    fit(pixels, orientation, kind, limits)
}

/// Whether `input`, already known to be `encoding`, holds more than one frame.
/// Bytes that cannot be read this way are not an animation here; whether they
/// are an image at all is settled by the decode that follows.
fn is_animated(encoding: Encoding, input: &[u8]) -> bool {
    let bytes = Cursor::new(input);
    match encoding {
        Encoding::Gif => GifDecoder::new(bytes)
            .is_ok_and(|decoder| decoder.into_frames().take(2).flatten().count() > 1),
        Encoding::Webp => WebPDecoder::new(bytes).is_ok_and(|decoder| decoder.has_animation()),
        Encoding::Png => {
            PngDecoder::new(bytes).is_ok_and(|decoder| decoder.is_apng().unwrap_or(false))
        }
        Encoding::Jpeg => false,
    }
}

fn unchanged(input: &[u8], encoding: Encoding, width: u32, height: u32) -> Normalized {
    Normalized {
        bytes: input.to_vec(),
        encoding,
        width,
        height,
        changed: false,
    }
}

/// Decode `input` with the system's decoder, then fit what it returns. What it
/// returns is held to the same budget as anything decoded here, because a
/// decoder is free to ignore the edge it was asked for.
fn fit_with_platform(
    input: &[u8],
    limits: &Limits,
    platform: Option<&dyn PlatformDecoder>,
) -> Result<Normalized, Error> {
    let decoder = platform.ok_or(Error::UnsupportedEncoding)?;
    let long_edge = limits.max_long_edge_px().min(MAX_PLATFORM_LONG_EDGE_PX);
    let decoded = decoder.decode(input, long_edge)?;
    let (width, height) = decoded.pixels.dimensions();
    if width == 0 || height == 0 {
        return Err(Error::Undecodable);
    }
    check_pixels(width, height)?;
    check_working_bytes(working_bytes(
        (width, height),
        u64::from(width) * u64::from(height) * 4,
        ColorType::Rgba8,
        Orientation::NoTransforms,
        limits.max_long_edge_px(),
    ))?;
    // What a system decoder is asked about (HEIC, AVIF, camera RAW) is a
    // photograph, and it comes back already upright.
    let pixels = DynamicImage::ImageRgba8(decoded.pixels);
    fit(
        pixels,
        Orientation::NoTransforms,
        Source::Photograph,
        limits,
    )
}

/// The most memory [`fit`] holds up to the end of its first pass, for an image
/// of `size` that decodes to `decoded_bytes` of `color`, counted before anything
/// is decoded. Each term is one moment in `fit`. A smaller size is counted when
/// it is about to be made, by [`scaling_bytes`], and not here: an image that fits
/// at its first size never pays for a scaling it does not do.
///
/// Saturating arithmetic: a figure too large to count is over any budget.
fn working_bytes(
    size: (u32, u32),
    decoded_bytes: u64,
    color: ColorType,
    orientation: Orientation,
    max_long_edge_px: u32,
) -> u64 {
    let (width, height) = size;
    let pixels = u64::from(width) * u64::from(height);
    let channels = channels_of(color);
    // The eight-bit copy everything after decoding works from.
    let working = pixels.saturating_mul(channels);
    // Made beside the decoded pixels, unless they already are that copy.
    let converting = if matches!(color, ColorType::Rgb8 | ColorType::Rgba8) {
        decoded_bytes
    } else {
        decoded_bytes.saturating_add(working)
    };
    // An alpha channel that hides nothing is dropped: both copies, briefly.
    let dropping_alpha = if color.has_alpha() {
        pixels.saturating_mul(4 + 3)
    } else {
        0
    };
    let first_pass = if width.max(height) > max_long_edge_px {
        // Scaled first, and turned upright afterwards, when it is small.
        scaling_bytes(size, channels, max_long_edge_px)
    } else if orientation != Orientation::NoTransforms {
        // Used whole, so the whole image is turned: two of it.
        working.saturating_mul(2)
    } else {
        0
    };
    // Unscaled, a see-through image is flattened onto white beside itself.
    let flattening = working.saturating_add(pixels.saturating_mul(3));
    [converting, dropping_alpha, first_pass, flattening]
        .into_iter()
        .fold(decoded_bytes, u64::max)
}

/// The most memory held while an image of `size` and `channels` is scaled so
/// that its long edge is `long_edge`: the source, a buffer of the source's
/// width and the new height between the scaler's two passes, and the result,
/// which is then turned or flattened onto white, either of which holds two.
fn scaling_bytes(size: (u32, u32), channels: u64, long_edge: u32) -> u64 {
    let (width, height) = size;
    let source = (u64::from(width) * u64::from(height)).saturating_mul(channels);
    let (scaled_width, scaled_height) = scaled_size(size, long_edge);
    let scaled = (u64::from(scaled_width) * u64::from(scaled_height)).saturating_mul(channels);
    let between_passes =
        (u64::from(width) * u64::from(scaled_height)).saturating_mul(SCALER_BYTES_PER_PIXEL);
    source.saturating_add(
        between_passes
            .saturating_add(scaled)
            .max(scaled.saturating_mul(2)),
    )
}

fn channels_of(color: ColorType) -> u64 {
    if color.has_alpha() {
        4
    } else {
        3
    }
}

/// The long edge tried after `long_edge` produced nothing that fits.
fn next_smaller(long_edge: u32) -> u32 {
    (f64::from(long_edge) * SCALE_STEP) as u32
}

/// Scale and encode `pixels` until they are inside `limits`. `orientation` is
/// the turn still owed to them.
///
/// Work is ordered to hold as little as possible: the decoded pixels become one
/// eight-bit copy and are dropped; an image that must be scaled is scaled first
/// and turned upright afterwards, when it is small; and every smaller size is
/// scaled from the source, never from an earlier result, so quality is lost once.
fn fit(
    pixels: DynamicImage,
    orientation: Orientation,
    source: Source,
    limits: &Limits,
) -> Result<Normalized, Error> {
    let mut pixels = eight_bit(pixels);
    let mut orientation = orientation;
    if pixels.width().max(pixels.height()) <= limits.max_long_edge_px() {
        pixels.apply_orientation(orientation);
        orientation = Orientation::NoTransforms;
    }
    let transparent = has_transparency(&pixels);
    if !transparent {
        // Most screenshots carry an alpha channel that hides nothing. It is
        // dropped once, here, so no encoder copies the image to drop it again.
        pixels = DynamicImage::ImageRgb8(pixels.into_rgb8());
    }
    let try_jpeg = limits.accepts(Encoding::Jpeg);
    // PNG first for what was lossless or see-through: screenshots and diagrams,
    // where JPEG smears exactly the detail that matters. And PNG always for a
    // consumer that takes nothing else.
    let try_png =
        limits.accepts(Encoding::Png) && (source == Source::Exact || transparent || !try_jpeg);

    let mut long_edge = pixels
        .width()
        .max(pixels.height())
        .min(limits.max_long_edge_px());
    loop {
        let mut sized = scaled_to(&pixels, long_edge);
        if orientation != Orientation::NoTransforms {
            // Only a scaled copy still owes its turn: an image used whole was
            // turned above, so this never copies the whole image.
            sized.to_mut().apply_orientation(orientation);
        }
        let (width, height) = (sized.width(), sized.height());
        if try_png {
            let bytes = encode_png(&sized)?;
            if bytes.len() as u64 <= limits.max_bytes() {
                return Ok(fitted(bytes, Encoding::Png, width, height));
            }
        }
        if try_jpeg {
            let opaque = flattened_onto_white(&sized);
            for quality in JPEG_QUALITIES {
                let bytes = encode_jpeg(&opaque, quality)?;
                if bytes.len() as u64 <= limits.max_bytes() {
                    return Ok(fitted(bytes, Encoding::Jpeg, width, height));
                }
            }
        }
        let next = next_smaller(long_edge);
        if next < MIN_LONG_EDGE_PX {
            return Err(Error::CannotFit);
        }
        // The smaller size is about to be made, so this is where it is counted.
        let size = (pixels.width(), pixels.height());
        check_working_bytes(scaling_bytes(size, channels_of(pixels.color()), next))?;
        long_edge = next;
    }
}

fn fitted(bytes: Vec<u8>, encoding: Encoding, width: u32, height: u32) -> Normalized {
    Normalized {
        bytes,
        encoding,
        width,
        height,
        changed: true,
    }
}

fn decode_error(error: ImageError) -> Error {
    match error {
        ImageError::Limits(_) => Error::TooLargeToDecode,
        ImageError::Unsupported(_) => Error::UnsupportedEncoding,
        _ => Error::Undecodable,
    }
}

/// `pixels` at eight bits a channel, as RGB or RGBA: the only two layouts
/// anything after decoding works in. Pixels already in one are not copied.
fn eight_bit(pixels: DynamicImage) -> DynamicImage {
    match pixels {
        DynamicImage::ImageRgb8(_) | DynamicImage::ImageRgba8(_) => pixels,
        other if other.color().has_alpha() => DynamicImage::ImageRgba8(other.into_rgba8()),
        other => DynamicImage::ImageRgb8(other.into_rgb8()),
    }
}

/// Whether any pixel of an [`eight_bit`] image is actually see-through. An
/// alpha channel that is opaque throughout, as most screenshots carry, is not
/// transparency. The pixels are read where they are; nothing is copied.
fn has_transparency(pixels: &DynamicImage) -> bool {
    pixels
        .as_rgba8()
        .is_some_and(|rgba| rgba.pixels().any(|pixel| pixel.0[3] != u8::MAX))
}

/// The size of a `width` by `height` image once its long edge is at most
/// `long_edge`, never scaled up.
fn scaled_size((width, height): (u32, u32), long_edge: u32) -> (u32, u32) {
    let current = width.max(height);
    if current <= long_edge {
        return (width, height);
    }
    let scale = f64::from(long_edge) / f64::from(current);
    (
        ((f64::from(width) * scale).round() as u32).max(1),
        ((f64::from(height) * scale).round() as u32).max(1),
    )
}

/// The image with its long edge at most `long_edge`: the image itself, not a
/// copy of it, when it is already that small.
fn scaled_to(pixels: &DynamicImage, long_edge: u32) -> Cow<'_, DynamicImage> {
    let size = (pixels.width(), pixels.height());
    let (width, height) = scaled_size(size, long_edge);
    if (width, height) == size {
        return Cow::Borrowed(pixels);
    }
    Cow::Owned(pixels.resize_exact(width, height, FilterType::Lanczos3))
}

/// Written as it is held: RGBA when something is see-through, RGB otherwise.
fn encode_png(pixels: &DynamicImage) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let encoder = PngEncoder::new(&mut bytes);
    let written = match pixels.as_rgb8() {
        Some(rgb) => rgb.write_with_encoder(encoder),
        None => rgba_of(pixels).write_with_encoder(encoder),
    };
    written.map_err(|_| Error::Undecodable)?;
    Ok(bytes)
}

fn encode_jpeg(pixels: &RgbImage, quality: u8) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    pixels
        .write_with_encoder(JpegEncoder::new_with_quality(&mut bytes, quality))
        .map_err(|_| Error::Undecodable)?;
    Ok(bytes)
}

/// JPEG has no transparency. See-through pixels are blended onto white, the
/// ground most documents and interfaces are drawn on, rather than turning black.
/// An image with nothing see-through is already RGB and is not copied.
fn flattened_onto_white(pixels: &DynamicImage) -> Cow<'_, RgbImage> {
    if let Some(rgb) = pixels.as_rgb8() {
        return Cow::Borrowed(rgb);
    }
    let rgba = rgba_of(pixels);
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (from, to) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = u32::from(from.0[3]);
        for channel in 0..3 {
            let blended = u32::from(from.0[channel]) * alpha + 255 * (255 - alpha);
            to.0[channel] = ((blended + 127) / 255) as u8;
        }
    }
    Cow::Owned(rgb)
}

/// The RGBA pixels of an [`eight_bit`] image that is not RGB, where they are.
/// The copy is for a layout `eight_bit` never produces.
fn rgba_of(pixels: &DynamicImage) -> Cow<'_, RgbaImage> {
    pixels
        .as_rgba8()
        .map_or_else(|| Cow::Owned(pixels.to_rgba8()), Cow::Borrowed)
}
