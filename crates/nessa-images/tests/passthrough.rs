//! When an image is handed back as it came, when it is not, and that memory is
//! counted for the work actually done.
use image::{
    codecs::gif::GifEncoder, Delay, DynamicImage, Frame, ImageFormat, Rgb, RgbImage, Rgba,
    RgbaImage,
};
use nessa_images::{normalize_with, Encoding, Error, Limits};
use std::io::Cursor;

const ALL: [Encoding; 4] = [Encoding::Png, Encoding::Jpeg, Encoding::Gif, Encoding::Webp];

fn limits(accepted: &[Encoding], max_bytes: u64, max_long_edge_px: u32) -> Limits {
    Limits::new(accepted.to_vec(), max_bytes, max_long_edge_px).unwrap()
}

fn encoded(pixels: impl Into<DynamicImage>, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    pixels.into().write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

fn animation(frames: &[[u8; 4]]) -> Vec<u8> {
    let mut bytes = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut bytes);
        for colour in frames {
            let frame = RgbaImage::from_pixel(32, 32, Rgba(*colour));
            let delay = Delay::from_numer_denom_ms(100, 1);
            encoder
                .encode_frame(Frame::from_parts(frame, 0, 0, delay))
                .unwrap();
        }
    }
    bytes
}

#[test]
fn an_animation_inside_every_limit_is_still_reduced_to_its_first_frame() {
    let moving = animation(&[[255, 0, 0, 255], [0, 255, 0, 255], [0, 0, 255, 255]]);
    // GIF is accepted and the bytes are inside both limits: everything that
    // would let a still image through untouched.
    let fitted = normalize_with(&moving, &limits(&ALL, 1 << 20, 4096), None).unwrap();
    assert!(fitted.changed);
    assert_eq!(fitted.encoding, Encoding::Png);
    let pixels = image::load_from_memory(&fitted.bytes).unwrap().to_rgb8();
    assert_eq!(pixels.get_pixel(16, 16).0, [255, 0, 0]);

    // One frame is not an animation, and goes through as it came.
    let still = animation(&[[255, 0, 0, 255]]);
    let fitted = normalize_with(&still, &limits(&ALL, 1 << 20, 4096), None).unwrap();
    assert_eq!((fitted.changed, fitted.encoding), (false, Encoding::Gif));
    assert_eq!(fitted.bytes, still);
}

#[test]
fn an_image_that_fits_at_its_own_size_is_not_charged_for_a_scaling_it_never_does() {
    // 64 megapixels of one colour: a small PNG, and nothing about it needs
    // scaling when the consumer takes an edge this long. Counting a smaller
    // size it never makes would put it over the memory budget.
    let wide = encoded(
        RgbImage::from_pixel(8000, 8000, Rgb([9, 9, 9])),
        ImageFormat::Png,
    );
    let fitted = normalize_with(&wide, &limits(&ALL, 1 << 20, 8000), None).unwrap();
    assert_eq!(
        (fitted.width, fitted.height, fitted.changed),
        (8000, 8000, false)
    );

    // Re-encoded at its own size, the same holds: QOI is not accepted, so it is
    // decoded and written as PNG without being made smaller.
    let qoi = encoded(
        RgbImage::from_pixel(8000, 8000, Rgb([9, 9, 9])),
        ImageFormat::Qoi,
    );
    let fitted = normalize_with(&qoi, &limits(&ALL, 1 << 20, 8000), None).unwrap();
    assert_eq!((fitted.width, fitted.encoding), (8000, Encoding::Png));
}

#[test]
fn a_smaller_size_is_counted_when_it_is_about_to_be_made() {
    // The same image, but nothing this size fits 2 000 bytes, so it must be made
    // smaller, and making 64 megapixels smaller is over the memory budget. That
    // is a refusal to do the work, not a claim that nothing could ever fit.
    let qoi = encoded(
        RgbImage::from_pixel(8000, 8000, Rgb([9, 9, 9])),
        ImageFormat::Qoi,
    );
    assert_eq!(
        normalize_with(&qoi, &limits(&[Encoding::Png], 2_000, 8000), None),
        Err(Error::TooLargeToDecode)
    );
}

/// A JPEG the strict reader refuses and every viewer shows: bytes after the end
/// marker are fine, so this one carries a second, unfinished image after it.
fn jpeg_a_strict_reader_refuses(pixels: RgbImage) -> Vec<u8> {
    let mut bytes = encoded(pixels, ImageFormat::Jpeg);
    let whole = bytes.len();
    // Cut the scan short by a quarter: the top of the picture still decodes.
    bytes.truncate(whole - whole / 4);
    bytes
}

#[test]
fn a_jpeg_that_is_not_proved_whole_is_written_again_and_never_handed_back() {
    let pixels = RgbImage::from_fn(64, 64, |x, y| Rgb([x as u8 * 3, y as u8 * 3, 90]));
    let input = jpeg_a_strict_reader_refuses(pixels);
    let result = normalize_with(&input, &limits(&ALL, 1 << 20, 4096), None);
    // Strictness decides only whether bytes go back untouched. What a forgiving
    // decoder can still show is written again; what it cannot is refused.
    match result {
        Ok(fitted) => {
            assert!(fitted.changed);
            assert_ne!(fitted.bytes, input);
            assert_eq!((fitted.width, fitted.height), (64, 64));
        }
        Err(error) => assert_eq!(error, Error::Undecodable),
    }
}
