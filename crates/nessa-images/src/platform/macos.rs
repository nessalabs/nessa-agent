//! ImageIO: the decoder macOS itself uses. It reads HEIC and HEIF, AVIF, JPEG XL,
//! PSD, and camera RAW from several hundred cameras, under the system's own
//! codec licences.
use crate::{DecodedImage, Error, PlatformDecoder};
use image::RgbaImage;
use objc2_core_foundation::{
    CFBoolean, CFData, CFDictionary, CFNumber, CFString, CFType, CGPoint, CGRect, CGSize,
};
use objc2_core_graphics::{
    CGBitmapContextCreate, CGColorSpace, CGContext, CGImage, CGImageAlphaInfo,
};
use objc2_image_io::{
    kCGImageSourceCreateThumbnailFromImageAlways, kCGImageSourceCreateThumbnailWithTransform,
    kCGImageSourceShouldCache, kCGImageSourceThumbnailMaxPixelSize, CGImageSource,
};
pub(super) struct ImageIo;

impl PlatformDecoder for ImageIo {
    fn decode(&self, input: &[u8], max_long_edge_px: u32) -> Result<DecodedImage, Error> {
        let max_pixel_size = i32::try_from(max_long_edge_px).unwrap_or(i32::MAX);
        let data = CFData::from_bytes(input);
        // SAFETY: no options dictionary is passed, so there are no generics to get wrong.
        let source =
            unsafe { CGImageSource::with_data(&data, None) }.ok_or(Error::UnsupportedEncoding)?;
        // SAFETY: `source` is a live image source; these only read it.
        if unsafe { source.count() } == 0 {
            return Err(Error::UnsupportedEncoding);
        }

        // "Thumbnail" is ImageIO's name for a decode to a bounded size. Made from
        // the full image always, never from a small preview a file may embed, and
        // with the recorded rotation applied.
        let yes: &CFType = CFBoolean::new(true).as_ref();
        let no: &CFType = CFBoolean::new(false).as_ref();
        let size = CFNumber::new_i32(max_pixel_size);
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

        // Draw into memory of a known layout, whatever the source's was: sRGB,
        // eight bits a channel, red first, alpha last and premultiplied.
        let mut buffer = vec![0_u8; width * height * 4];
        // SAFETY: the static is a valid CFString naming a colour space.
        let space =
            CGColorSpace::with_name(Some(unsafe { objc2_core_graphics::kCGColorSpaceSRGB }))
                .ok_or(Error::Undecodable)?;
        // SAFETY: `buffer` is exactly `height` rows of `width * 4` bytes and
        // outlives `context`, which is dropped before `buffer` is read.
        let context = unsafe {
            CGBitmapContextCreate(
                buffer.as_mut_ptr().cast(),
                width,
                height,
                8,
                width * 4,
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
        Ok(DecodedImage {
            pixels,
            lossless: false,
        })
    }
}

/// Core Graphics draws premultiplied alpha; everything after this works in
/// straight alpha. Opaque pixels, which is nearly all of them, are untouched.
fn straighten_alpha(buffer: &mut [u8]) {
    for [red, green, blue, alpha] in buffer.as_chunks_mut::<4>().0 {
        let alpha = u32::from(*alpha);
        if alpha == 0 || alpha == 255 {
            continue;
        }
        for channel in [red, green, blue] {
            *channel = ((u32::from(*channel) * 255 + alpha / 2) / alpha).min(255) as u8;
        }
    }
}
