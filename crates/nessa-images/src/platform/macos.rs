//! ImageIO: the decoder macOS itself uses. It reads HEIC and HEIF, AVIF, JPEG XL,
//! PSD, and camera RAW from several hundred cameras, under the system's own codec
//! licences.
//!
//! This file holds the second of the two gates the crate documentation describes:
//! it asks ImageIO what it takes the bytes for and goes on only for the types
//! listed below. Camera RAW is several hundred vendor types, all declared by the
//! system as kinds of `public.camera-raw-image`, so that one is asked of the
//! system rather than listed.
use crate::{
    budget::{check_pixels, MAX_PLATFORM_LONG_EDGE_PX},
    DecodedImage, Error, PlatformDecoder,
};
use image::RgbaImage;
use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFNumber, CFRetained, CFString, CFType, CGPoint, CGRect,
    CGSize,
};
use objc2_core_graphics::{
    kCGColorSpaceSRGB, CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
};
use objc2_image_io::{
    kCGImagePropertyPixelHeight, kCGImagePropertyPixelWidth,
    kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform,
    kCGImageSourceShouldCache, kCGImageSourceThumbnailMaxPixelSize, CGImageSource,
};

/// The types handed on to ImageIO by name. Camera RAW is matched by kind below.
const LISTED_TYPES: [&str; 8] = [
    "public.heic",
    "public.heif",
    "public.heics",
    "public.avif",
    "public.avis",
    "public.jpeg-xl",
    "com.adobe.photoshop-image",
    // DNG. Also a kind of camera RAW; named so the list reads whole.
    "com.adobe.raw-image",
];
/// The kind every vendor's RAW type is declared under.
const CAMERA_RAW: &str = "public.camera-raw-image";

// `UTTypeConformsTo` is the C call behind uniform type identifiers. Apple marks
// it deprecated in favour of an Objective-C class; it remains exported, and it
// is the one way to ask this without an Objective-C runtime dependency.
#[link(name = "CoreServices", kind = "framework")]
extern "C" {
    fn UTTypeConformsTo(identifier: &CFString, kind: &CFString) -> u8;
}

pub(super) struct ImageIo;

impl PlatformDecoder for ImageIo {
    fn decode(&self, input: &[u8], max_long_edge_px: u32) -> Result<DecodedImage, Error> {
        // The result is drawn into memory sized from this, so it is bounded here
        // and not only by whoever called.
        let long_edge = max_long_edge_px.clamp(1, MAX_PLATFORM_LONG_EDGE_PX);
        let data = CFData::from_bytes(input);
        // SAFETY: `data` is a live CFData, and no options dictionary is passed,
        // so there are no generics to get wrong.
        let source =
            unsafe { CGImageSource::with_data(&data, None) }.ok_or(Error::UnsupportedEncoding)?;
        // SAFETY: `source` is a live image source; this only reads it.
        if unsafe { source.count() } == 0 {
            return Err(Error::UnsupportedEncoding);
        }
        // SAFETY: as above.
        let kind = unsafe { source.r#type() }.ok_or(Error::UnsupportedEncoding)?;
        if !is_handed_on(&kind) {
            return Err(Error::UnsupportedEncoding);
        }
        // Read from the file's header, before any pixel is decoded.
        let (source_width, source_height) = pixel_size(&source).ok_or(Error::Undecodable)?;
        check_pixels(source_width, source_height)?;

        // "Thumbnail" is ImageIO's name for a decode to a bounded size. Made from
        // the full image always, never from a small preview a file may embed, and
        // with the recorded rotation applied.
        let yes: &CFType = CFBoolean::new(true).as_ref();
        let no: &CFType = CFBoolean::new(false).as_ref();
        // At most 8,192, so it fits.
        let size = CFNumber::new_i32(long_edge as i32);
        // SAFETY: these statics are valid CFStrings for the life of the process.
        let keys: [&CFString; 4] = unsafe {
            [
                kCGImageSourceCreateThumbnailFromImageAlways,
                kCGImageSourceCreateThumbnailWithTransform,
                kCGImageSourceThumbnailMaxPixelSize,
                kCGImageSourceShouldCache,
            ]
        };
        let values: [&CFType; 4] = [yes, yes, size.as_ref(), no];
        let options = CFDictionary::from_slices(&keys, &values);
        // SAFETY: every key is a CFString and every value the CFBoolean or
        // CFNumber that key is documented to take.
        let decoded = unsafe { source.thumbnail_at_index(0, Some(options.as_opaque())) }
            .ok_or(Error::Undecodable)?;

        let width = CGImage::width(Some(&decoded));
        let height = CGImage::height(Some(&decoded));
        if width == 0 || height == 0 {
            return Err(Error::Undecodable);
        }
        let (Ok(pixel_width), Ok(pixel_height)) = (u32::try_from(width), u32::try_from(height))
        else {
            return Err(Error::TooLargeToDecode);
        };
        // ImageIO was asked for at most `long_edge`; hold it to that.
        if pixel_width.max(pixel_height) > long_edge {
            return Err(Error::TooLargeToDecode);
        }
        let row_bytes = width.checked_mul(4).ok_or(Error::TooLargeToDecode)?;
        let buffer_bytes = row_bytes
            .checked_mul(height)
            .ok_or(Error::TooLargeToDecode)?;

        // Draw into memory of a known layout, whatever the source's was: sRGB,
        // eight bits a channel, red first, alpha last and premultiplied.
        let mut buffer = vec![0_u8; buffer_bytes];
        // SAFETY: the static is a valid CFString naming a colour space.
        let space = CGColorSpace::with_name(Some(unsafe { kCGColorSpaceSRGB }))
            .ok_or(Error::Undecodable)?;
        // SAFETY: `buffer` is `height` rows of `row_bytes`, which is `width`
        // pixels of four bytes: exactly the layout described to the context, in
        // checked arithmetic. It outlives `context`, which is dropped below
        // before `buffer` is read or moved.
        let context = unsafe {
            CGBitmapContextCreate(
                buffer.as_mut_ptr().cast(),
                width,
                height,
                8,
                row_bytes,
                Some(&space),
                CGImageAlphaInfo::PremultipliedLast.0,
            )
        }
        .ok_or(Error::Undecodable)?;
        let whole = CGRect {
            origin: CGPoint { x: 0.0, y: 0.0 },
            size: CGSize {
                width: width as f64,
                height: height as f64,
            },
        };
        CGContext::draw_image(Some(&context), whole, Some(&decoded));
        drop(context);

        straighten_alpha(&mut buffer);
        let pixels =
            RgbaImage::from_raw(pixel_width, pixel_height, buffer).ok_or(Error::Undecodable)?;
        // Everything ImageIO is asked about here is a photograph or a rendering
        // of one, so the result is treated as one and fitted as JPEG first.
        Ok(DecodedImage {
            pixels,
            lossless: false,
        })
    }
}

/// Whether ImageIO's name for what it found is one this crate hands on to it.
fn is_handed_on(kind: &CFString) -> bool {
    let name = kind.to_string();
    if LISTED_TYPES.contains(&name.as_str()) {
        return true;
    }
    let camera_raw = CFString::from_static_str(CAMERA_RAW);
    // SAFETY: both arguments are live CFStrings, which is all the call reads.
    unsafe { UTTypeConformsTo(kind, &camera_raw) != 0 }
}

/// The first image's width and height in pixels as its header states them,
/// before any rotation. `None` when ImageIO cannot say, as for a file cut short
/// inside its header.
fn pixel_size(source: &CGImageSource) -> Option<(u32, u32)> {
    // SAFETY: `source` is a live image source and no options are passed.
    let properties = unsafe { source.properties_at_index(0, None) }?;
    // SAFETY: an image property dictionary is keyed by CFString, and every
    // value in it is some CoreFoundation object, which is all `CFType` claims.
    // Each value's real type is checked before use.
    let properties = unsafe { properties.cast_unchecked::<CFString, CFType>() };
    let number = |key: &CFString| -> Option<u32> {
        let value: CFRetained<CFType> = properties.get(key)?;
        u32::try_from(value.downcast_ref::<CFNumber>()?.as_i64()?).ok()
    };
    // SAFETY: these statics are valid CFStrings for the life of the process.
    let (width, height) = unsafe { (kCGImagePropertyPixelWidth, kCGImagePropertyPixelHeight) };
    Some((number(width)?, number(height)?))
}

/// Core Graphics draws premultiplied alpha; everything after this works in
/// straight alpha. Opaque pixels, which is nearly all of them, are untouched.
fn straighten_alpha(buffer: &mut [u8]) {
    for pixel in buffer.as_chunks_mut::<4>().0 {
        *pixel = straightened(*pixel);
    }
}

/// One premultiplied RGBA pixel as straight alpha: each colour channel divided
/// by the alpha it was multiplied by, rounded to nearest, and held to full
/// intensity where rounding or a channel brighter than its own alpha would pass
/// it. Fully opaque and fully transparent pixels are their own answer: there is
/// nothing to undo, and nothing to divide by.
fn straightened(pixel: [u8; 4]) -> [u8; 4] {
    let [red, green, blue, alpha] = pixel;
    if alpha == 0 || alpha == u8::MAX {
        return pixel;
    }
    let divisor = u32::from(alpha);
    let straighten =
        |channel: u8| ((u32::from(channel) * 255 + divisor / 2) / divisor).min(255) as u8;
    [straighten(red), straighten(green), straighten(blue), alpha]
}

#[cfg(test)]
#[path = "../../tests/unit/platform/macos.rs"]
mod tests;
