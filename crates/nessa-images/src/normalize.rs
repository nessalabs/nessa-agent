use crate::{platform_decoder, Encoding, Error, Limits, PlatformDecoder};
use image::{
    codecs::{jpeg::JpegEncoder, png::PngEncoder},
    imageops::FilterType,
    metadata::Orientation,
    DynamicImage, ImageDecoder, ImageError, ImageFormat, ImageReader, RgbImage, RgbaImage,
};
use std::io::Cursor;

/// Largest input read at all. Beyond this the bytes are not even sniffed.
const MAX_INPUT_BYTES: usize = 64 * 1024 * 1024;
/// Longest edge decoded. A larger image is refused before any pixel is read.
const MAX_DECODE_EDGE_PX: u32 = 16_384;
/// Most memory a decode may allocate.
const MAX_DECODE_ALLOC_BYTES: u64 = 512 * 1024 * 1024;
/// JPEG qualities tried at each size, best first. Below the last, text in a
/// screenshot stops being legible, so the image is scaled down instead.
const JPEG_QUALITIES: [u8; 3] = [90, 80, 70];
/// Each failed size is followed by one this fraction of it.
const SCALE_STEP: f64 = 0.8;
/// Scaling stops once the long edge would fall below this: smaller than it, a
/// result would fit and show nothing.
const MIN_LONG_EDGE_PX: u32 = 256;

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
pub fn normalize(input: &[u8], limits: &Limits) -> Result<Normalized, Error> {
    normalize_with(input, limits, platform_decoder())
}

/// [`normalize`] with the platform decoder chosen by the caller: a substitute in
/// a test, or `None` to read only what this crate reads itself.
pub fn normalize_with(
    input: &[u8],
    limits: &Limits,
    platform: Option<&dyn PlatformDecoder>,
) -> Result<Normalized, Error> {
    if input.len() > MAX_INPUT_BYTES {
        return Err(Error::TooLargeToDecode);
    }
    let reader = ImageReader::new(Cursor::new(input))
        .with_guessed_format()
        .map_err(|_| Error::UnsupportedEncoding)?;
    let source = match reader.format() {
        Some(ImageFormat::Png) => Some(Encoding::Png),
        Some(ImageFormat::Jpeg) => Some(Encoding::Jpeg),
        Some(ImageFormat::Gif) => Some(Encoding::Gif),
        Some(ImageFormat::WebP) => Some(Encoding::Webp),
        // Most camera RAW files are TIFF containers whose first image is a small
        // preview. Reading one as a TIFF would quietly return that preview.
        Some(ImageFormat::Tiff) if !tiff_holds_only_a_preview(input) => None,
        Some(
            ImageFormat::Bmp
            | ImageFormat::Ico
            | ImageFormat::Qoi
            | ImageFormat::Pnm
            | ImageFormat::Hdr,
        ) => None,
        // HEIC, AVIF, camera RAW, and whatever else only the system reads.
        _ => {
            let decoder = platform.ok_or(Error::UnsupportedEncoding)?;
            let decoded = decoder.decode(input, limits.max_long_edge_px())?;
            let pixels = DynamicImage::ImageRgba8(decoded.pixels);
            return fit(&pixels, decoded.lossless, limits);
        }
    };
    let mut reader = reader;
    let mut decode_limits = image::Limits::default();
    decode_limits.max_image_width = Some(MAX_DECODE_EDGE_PX);
    decode_limits.max_image_height = Some(MAX_DECODE_EDGE_PX);
    decode_limits.max_alloc = Some(MAX_DECODE_ALLOC_BYTES);
    reader.limits(decode_limits);
    let mut decoder = reader.into_decoder().map_err(decode_error)?;
    let (width, height) = decoder.dimensions();
    if width == 0 || height == 0 {
        return Err(Error::Undecodable);
    }
    // A decoder that cannot read orientation has none recorded to apply.
    let orientation = decoder.orientation().unwrap_or(Orientation::NoTransforms);

    let upright = orientation == Orientation::NoTransforms;
    if let Some(encoding) = source.filter(|encoding| limits.accepts(*encoding)) {
        if upright
            && input.len() as u64 <= limits.max_bytes()
            && width.max(height) <= limits.max_long_edge_px()
        {
            return Ok(Normalized {
                bytes: input.to_vec(),
                encoding,
                width,
                height,
                changed: false,
            });
        }
    }

    let mut pixels = DynamicImage::from_decoder(decoder).map_err(decode_error)?;
    pixels.apply_orientation(orientation);
    fit(&pixels, source != Some(Encoding::Jpeg), limits)
}

/// Scale and encode upright `pixels` until they are inside `limits`.
fn fit(pixels: &DynamicImage, lossless_source: bool, limits: &Limits) -> Result<Normalized, Error> {
    let transparent = has_transparency(pixels);
    let try_jpeg = limits.accepts(Encoding::Jpeg);
    // PNG first for what was lossless or see-through: screenshots and diagrams,
    // where JPEG smears exactly the detail that matters. And PNG always for a
    // consumer that takes nothing else.
    let try_png = limits.accepts(Encoding::Png) && (lossless_source || transparent || !try_jpeg);

    let mut long_edge = pixels
        .width()
        .max(pixels.height())
        .min(limits.max_long_edge_px());
    loop {
        let sized = scaled_to(pixels, long_edge);
        let (width, height) = (sized.width(), sized.height());
        if try_png {
            let bytes = encode_png(&sized, transparent)?;
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
        let next = (f64::from(long_edge) * SCALE_STEP) as u32;
        if next < MIN_LONG_EDGE_PX {
            return Err(Error::CannotFit);
        }
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

/// Whether any pixel is actually see-through. An alpha channel that is opaque
/// throughout, as most screenshots carry, is not transparency.
fn has_transparency(pixels: &DynamicImage) -> bool {
    pixels.color().has_alpha()
        && pixels
            .to_rgba8()
            .pixels()
            .any(|pixel| pixel.0[3] != u8::MAX)
}

/// The image with its long edge at most `long_edge`, never scaled up.
fn scaled_to(pixels: &DynamicImage, long_edge: u32) -> DynamicImage {
    let current = pixels.width().max(pixels.height());
    if current <= long_edge {
        return pixels.clone();
    }
    let scale = f64::from(long_edge) / f64::from(current);
    let width = ((f64::from(pixels.width()) * scale).round() as u32).max(1);
    let height = ((f64::from(pixels.height()) * scale).round() as u32).max(1);
    pixels.resize_exact(width, height, FilterType::Lanczos3)
}

fn encode_png(pixels: &DynamicImage, transparent: bool) -> Result<Vec<u8>, Error> {
    let mut bytes = Vec::new();
    let encoder = PngEncoder::new(&mut bytes);
    // Eight bits a channel, and no alpha channel unless something is see-through.
    let written = if transparent {
        let rgba: RgbaImage = pixels.to_rgba8();
        rgba.write_with_encoder(encoder)
    } else {
        let rgb: RgbImage = pixels.to_rgb8();
        rgb.write_with_encoder(encoder)
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
fn flattened_onto_white(pixels: &DynamicImage) -> RgbImage {
    if !pixels.color().has_alpha() {
        return pixels.to_rgb8();
    }
    let rgba = pixels.to_rgba8();
    let mut rgb = RgbImage::new(rgba.width(), rgba.height());
    for (from, to) in rgba.pixels().zip(rgb.pixels_mut()) {
        let alpha = u32::from(from.0[3]);
        for channel in 0..3 {
            let blended = u32::from(from.0[channel]) * alpha + 255 * (255 - alpha);
            to.0[channel] = ((blended + 127) / 255) as u8;
        }
    }
    rgb
}

/// Whether a TIFF's first image is marked as a reduced-resolution copy of
/// another, which is how camera RAW files (NEF, CR2, ARW, DNG, and the rest)
/// lay themselves out. A plain TIFF's first image is the image.
fn tiff_holds_only_a_preview(input: &[u8]) -> bool {
    let read = |offset: usize, length: usize| input.get(offset..offset.checked_add(length)?);
    let little_endian = match read(0, 2) {
        Some(b"II") => true,
        Some(b"MM") => false,
        _ => return false,
    };
    let u16_at = |offset: usize| {
        read(offset, 2).map(|bytes| {
            let bytes = [bytes[0], bytes[1]];
            if little_endian {
                u16::from_le_bytes(bytes)
            } else {
                u16::from_be_bytes(bytes)
            }
        })
    };
    let u32_at = |offset: usize| {
        read(offset, 4).map(|bytes| {
            let bytes = [bytes[0], bytes[1], bytes[2], bytes[3]];
            if little_endian {
                u32::from_le_bytes(bytes)
            } else {
                u32::from_be_bytes(bytes)
            }
        })
    };
    let Some(directory) = u32_at(4).map(|offset| offset as usize) else {
        return false;
    };
    let Some(entries) = u16_at(directory) else {
        return false;
    };
    (0..usize::from(entries)).any(|index| {
        let entry = directory + 2 + index * 12;
        match u16_at(entry) {
            // NewSubfileType with its lowest bit set: a reduced-resolution image.
            Some(0x00fe) => u32_at(entry + 8).is_some_and(|value| value & 1 == 1),
            // SubIFDs or DNGVersion: the real image lives in another directory.
            Some(0x014a | 0xc612) => true,
            _ => false,
        }
    })
}
