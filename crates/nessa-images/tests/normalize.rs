//! Every rule in the crate documentation, against images built here. No files.
use image::{
    codecs::gif::GifEncoder, ColorType, Delay, DynamicImage, Frame, ImageFormat, Rgb, RgbImage,
    Rgba, RgbaImage,
};
use nessa_images::{normalize, Encoding, Error, Limits, LimitsError, Normalized};
use std::io::Cursor;

const ALL: [Encoding; 4] = [Encoding::Png, Encoding::Jpeg, Encoding::Gif, Encoding::Webp];

fn limits(accepted: &[Encoding], max_bytes: u64, max_long_edge_px: u32) -> Limits {
    Limits::new(accepted.to_vec(), max_bytes, max_long_edge_px).unwrap()
}

/// Pixels no encoder can compress: what a photograph looks like to PNG.
fn noise(width: u32, height: u32) -> RgbImage {
    let mut state = 0x2545_f491_u32;
    RgbImage::from_fn(width, height, |_, _| {
        let mut channel = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        };
        Rgb([channel(), channel(), channel()])
    })
}

/// Left half red, right half blue: tiny as PNG, and orientation is visible.
fn halves(width: u32, height: u32) -> RgbImage {
    RgbImage::from_fn(width, height, |x, _| {
        if x < width / 2 {
            Rgb([255, 0, 0])
        } else {
            Rgb([0, 0, 255])
        }
    })
}

fn encoded(pixels: impl Into<DynamicImage>, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    pixels.into().write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

fn decoded(bytes: &[u8]) -> DynamicImage {
    image::load_from_memory(bytes).unwrap()
}

#[test]
fn an_acceptable_image_passes_through_byte_for_byte() {
    for (format, encoding) in [
        (ImageFormat::Png, Encoding::Png),
        (ImageFormat::Jpeg, Encoding::Jpeg),
        (ImageFormat::Gif, Encoding::Gif),
        (ImageFormat::WebP, Encoding::Webp),
    ] {
        let pixels = DynamicImage::from(halves(64, 32)).to_rgba8();
        let input = encoded(DynamicImage::from(pixels).to_rgb8(), format);
        // Exactly at both limits is inside them.
        let fitted = normalize(&input, &limits(&ALL, input.len() as u64, 64)).unwrap();
        assert_eq!(fitted.bytes, input, "{encoding:?}");
        assert_eq!(
            (fitted.encoding, fitted.width, fitted.height, fitted.changed),
            (encoding, 64, 32, false)
        );
    }
}

#[test]
fn an_encoding_the_consumer_does_not_take_becomes_png_without_losing_a_pixel() {
    let pixels = halves(64, 32);
    for format in [
        ImageFormat::Bmp,
        ImageFormat::Tiff,
        ImageFormat::Gif,
        ImageFormat::WebP,
    ] {
        let input = encoded(pixels.clone(), format);
        let only = [Encoding::Png, Encoding::Jpeg];
        let fitted = normalize(&input, &limits(&only, 1 << 20, 4096)).unwrap();
        assert_eq!(
            (fitted.encoding, fitted.changed),
            (Encoding::Png, true),
            "{format:?}"
        );
        assert_eq!(decoded(&fitted.bytes).to_rgb8(), pixels, "{format:?}");
    }
}

#[test]
fn a_long_edge_over_the_limit_is_scaled_to_it_and_nothing_is_scaled_up() {
    for (width, height, expected) in [
        (400, 100, (200, 50)),
        (100, 400, (50, 200)),
        (333, 111, (200, 67)),
    ] {
        let input = encoded(halves(width, height), ImageFormat::Png);
        let fitted = normalize(&input, &limits(&ALL, 1 << 20, 200)).unwrap();
        assert_eq!((fitted.width, fitted.height), expected);
        let pixels = decoded(&fitted.bytes);
        assert_eq!((pixels.width(), pixels.height()), expected);
        assert!(fitted.changed);
    }
    let small = encoded(halves(40, 10), ImageFormat::Png);
    let fitted = normalize(&small, &limits(&ALL, 1 << 20, 200)).unwrap();
    assert_eq!(
        (fitted.width, fitted.height, fitted.changed),
        (40, 10, false)
    );
}

#[test]
fn an_image_over_the_byte_limit_is_compressed_and_then_scaled_until_it_fits() {
    let input = encoded(noise(600, 600), ImageFormat::Png);
    assert!(input.len() > 1_000_000);

    // PNG cannot fit this noise in 550 KB, and nor can JPEG at its best
    // quality; a slightly lower quality at full size can.
    let fitted = normalize(&input, &limits(&ALL, 550_000, 4096)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Jpeg);
    assert!(fitted.bytes.len() <= 550_000);
    assert_eq!((fitted.width, fitted.height), (600, 600));

    // Too small for full-size JPEG at a legible quality: scaled down, not smeared.
    let fitted = normalize(&input, &limits(&ALL, 150_000, 4096)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Jpeg);
    assert!(fitted.bytes.len() <= 150_000);
    assert!(fitted.width < 600 && fitted.width == fitted.height);
    assert_eq!(decoded(&fitted.bytes).width(), fitted.width);

    // The same bytes and limits always give the same answer.
    assert_eq!(
        fitted,
        normalize(&input, &limits(&ALL, 150_000, 4096)).unwrap()
    );
}

#[test]
fn a_consumer_that_takes_only_png_never_receives_jpeg() {
    let input = encoded(noise(600, 600), ImageFormat::Png);
    let fitted = normalize(&input, &limits(&[Encoding::Png], 400_000, 4096)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Png);
    assert!(fitted.bytes.len() <= 400_000);
    assert!(fitted.width < 600);
}

#[test]
fn a_photograph_stays_jpeg_when_it_is_scaled() {
    let input = encoded(noise(800, 400), ImageFormat::Jpeg);
    let fitted = normalize(&input, &limits(&ALL, 1 << 22, 400)).unwrap();
    assert_eq!(
        (fitted.encoding, fitted.width, fitted.height),
        (Encoding::Jpeg, 400, 200)
    );
}

#[test]
fn nothing_readable_fits_is_a_refusal_not_a_thumbnail() {
    let input = encoded(noise(600, 600), ImageFormat::Png);
    assert_eq!(
        normalize(&input, &limits(&ALL, 2_000, 4096)),
        Err(Error::CannotFit)
    );
}

#[test]
fn transparency_survives_in_png_and_is_flattened_onto_white_for_jpeg() {
    let see_through = RgbaImage::from_fn(300, 300, |x, _| {
        if x < 150 {
            Rgba([0, 0, 0, 0])
        } else {
            Rgba([0, 0, 255, 255])
        }
    });
    let input = encoded(see_through, ImageFormat::Png);
    let fitted = normalize(&input, &limits(&ALL, 1 << 20, 100)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Png);
    let pixels = decoded(&fitted.bytes).to_rgba8();
    assert_eq!(pixels.get_pixel(10, 50).0[3], 0);
    assert_eq!(pixels.get_pixel(90, 50).0, [0, 0, 255, 255]);

    let fitted = normalize(&input, &limits(&[Encoding::Jpeg], 1 << 20, 100)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Jpeg);
    let pixels = decoded(&fitted.bytes).to_rgb8();
    // White, not the black that discarding alpha would leave.
    assert!(pixels
        .get_pixel(10, 50)
        .0
        .iter()
        .all(|channel| *channel > 240));
    assert!(pixels.get_pixel(90, 50).0[2] > 200);
}

#[test]
fn an_alpha_channel_that_hides_nothing_is_not_written_back() {
    let opaque = RgbaImage::from_pixel(300, 100, Rgba([10, 20, 30, 255]));
    let input = encoded(opaque, ImageFormat::Png);
    let fitted = normalize(&input, &limits(&ALL, 1 << 20, 150)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Png);
    assert_eq!(decoded(&fitted.bytes).color(), ColorType::Rgb8);
}

/// A JPEG whose metadata says "rotate a quarter turn clockwise to view".
fn rotated_jpeg(pixels: RgbImage) -> Vec<u8> {
    let plain = encoded(pixels, ImageFormat::Jpeg);
    let tiff: &[u8] = &[
        b'I', b'I', 42, 0, 8, 0, 0, 0, // little-endian TIFF, first directory at 8
        1, 0, // one entry
        0x12, 0x01, 3, 0, 1, 0, 0, 0, 6, 0, 0, 0, // Orientation (0x0112), SHORT, 1 value: 6
        0, 0, 0, 0, // no further directory
    ];
    let mut segment = b"Exif\0\0".to_vec();
    segment.extend_from_slice(tiff);
    let mut bytes = vec![0xff, 0xd8, 0xff, 0xe1];
    bytes.extend_from_slice(&(segment.len() as u16 + 2).to_be_bytes());
    bytes.extend_from_slice(&segment);
    bytes.extend_from_slice(&plain[2..]);
    bytes
}

#[test]
fn recorded_rotation_is_applied_to_the_pixels_even_when_nothing_else_needs_changing() {
    // Stored 80 wide with red on the left; viewed upright it is 40 wide with red on top.
    let input = rotated_jpeg(halves(80, 40));
    let fitted = normalize(&input, &limits(&ALL, 1 << 20, 4096)).unwrap();
    assert!(fitted.changed);
    assert_eq!((fitted.width, fitted.height), (40, 80));
    let pixels = decoded(&fitted.bytes).to_rgb8();
    assert!(pixels.get_pixel(20, 10).0[0] > 200, "red on top");
    assert!(pixels.get_pixel(20, 70).0[2] > 200, "blue below");
}

#[test]
fn only_the_first_frame_of_an_animation_is_kept() {
    let mut input = Vec::new();
    {
        let mut encoder = GifEncoder::new(&mut input);
        for colour in [[255, 0, 0, 255], [0, 255, 0, 255]] {
            let frame = RgbaImage::from_pixel(32, 32, Rgba(colour));
            encoder
                .encode_frame(Frame::from_parts(
                    frame,
                    0,
                    0,
                    Delay::from_numer_denom_ms(100, 1),
                ))
                .unwrap();
        }
    }
    let fitted = normalize(&input, &limits(&[Encoding::Png], 1 << 20, 4096)).unwrap();
    let pixels = decoded(&fitted.bytes).to_rgb8();
    assert_eq!(pixels.get_pixel(16, 16).0, [255, 0, 0]);
}

#[test]
fn bytes_that_are_not_a_readable_image_are_refused_by_kind() {
    let heic = [
        0, 0, 0, 24, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0, 0, 0, 0,
    ];
    for unsupported in [
        &b""[..],
        b"not an image at all",
        b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>",
        b"%PDF-1.7",
        &heic,
    ] {
        assert_eq!(
            normalize(unsupported, &limits(&ALL, 1 << 20, 4096)),
            Err(Error::UnsupportedEncoding),
            "{unsupported:?}"
        );
    }

    let whole = encoded(noise(64, 64), ImageFormat::Png);
    let truncated = &whole[..whole.len() / 2];
    // Over the byte limit, so it must be decoded rather than passed through.
    assert_eq!(
        normalize(truncated, &limits(&ALL, 100, 4096)),
        Err(Error::Undecodable)
    );
}

/// Damaged bytes are never the answer. A JPEG is the one encoding whose decoder
/// still shows part of a damaged file, as every viewer does, so it may be
/// written again; anything else that is damaged is refused.
fn never_handed_back(
    result: Result<Normalized, Error>,
    damaged: &[u8],
    format: ImageFormat,
    how: &str,
) {
    match result {
        Ok(image) if format == ImageFormat::Jpeg => {
            assert!(image.changed && image.bytes != damaged, "jpeg {how}");
        }
        other => assert_eq!(other, Err(Error::Undecodable), "{format:?} {how}"),
    }
}

#[test]
fn an_image_that_would_pass_through_must_first_be_shown_to_decode() {
    // Every one of these is in an accepted encoding, upright, and inside both
    // limits by its header. None holds a whole image, so none is "unchanged".
    let roomy = limits(&ALL, 1 << 20, 4096);
    for (format, name) in [
        (ImageFormat::Png, "png"),
        (ImageFormat::Jpeg, "jpeg"),
        (ImageFormat::Gif, "gif"),
        (ImageFormat::WebP, "webp"),
    ] {
        let whole = encoded(noise(64, 64), format);
        assert!(!normalize(&whole, &roomy).unwrap().changed, "{name}");

        let truncated = &whole[..whole.len() / 2];
        never_handed_back(normalize(truncated, &roomy), truncated, format, "cut short");

        // The header is kept so the size still reads; the pixels are overwritten.
        let mut corrupt = whole.clone();
        let body = corrupt.len() / 2;
        for byte in &mut corrupt[body..] {
            *byte = 0x55;
        }
        let result = normalize(&corrupt, &roomy);
        if format == ImageFormat::Gif {
            // GIF carries no checksum, and nearly any bytes are valid LZW
            // codes: overwritten pixels can still be a whole image as far as
            // any decoder can tell. What is promised is that such a file was
            // decoded to its end, not that damage is always detectable.
            assert!(
                matches!(&result, Ok(image) if !image.changed) || result == Err(Error::Undecodable),
                "{name} overwritten"
            );
        } else {
            never_handed_back(result, &corrupt, format, "overwritten");
        }
    }

    // A PNG of 75 bytes: a true header, then a data chunk of noise.
    let mut header = b"IHDR".to_vec();
    header.extend(32_u32.to_be_bytes());
    header.extend(32_u32.to_be_bytes());
    header.extend([8, 2, 0, 0, 0]);
    let mut data = b"IDAT".to_vec();
    data.extend([0x13, 0x37, 0xc0, 0xff, 0xee, 0x00, 0x42, 0x99, 0x10, 0x20]);
    let mut png = b"\x89PNG\r\n\x1a\n".to_vec();
    for chunk in [header.as_slice(), data.as_slice(), b"IEND"] {
        png.extend((chunk.len() as u32 - 4).to_be_bytes());
        png.extend(chunk);
        png.extend(crc32(chunk).to_be_bytes());
    }
    assert_eq!(normalize(&png, &roomy), Err(Error::Undecodable));
}

fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

#[test]
fn an_image_claiming_enormous_dimensions_is_refused_before_it_is_decoded() {
    let mut bomb = encoded(halves(2, 2), ImageFormat::Png);
    // IHDR data starts at byte 16: width then height, big-endian.
    bomb[16..20].copy_from_slice(&60_000_u32.to_be_bytes());
    bomb[20..24].copy_from_slice(&60_000_u32.to_be_bytes());
    let crc = crc32(&bomb[12..29]);
    bomb[29..33].copy_from_slice(&crc.to_be_bytes());
    assert_eq!(
        normalize(&bomb, &limits(&ALL, 1 << 20, 4096)),
        Err(Error::TooLargeToDecode)
    );
}

#[test]
fn limits_must_be_positive_and_able_to_produce_something() {
    assert_eq!(Limits::new(ALL.to_vec(), 0, 1), Err(LimitsError::Zero));
    assert_eq!(Limits::new(ALL.to_vec(), 1, 0), Err(LimitsError::Zero));
    for unproducible in [vec![], vec![Encoding::Gif, Encoding::Webp]] {
        assert_eq!(
            Limits::new(unproducible, 1, 1),
            Err(LimitsError::NoOutputEncoding)
        );
    }
    let only_jpeg = Limits::new(vec![Encoding::Jpeg], 10, 20).unwrap();
    assert!(only_jpeg.accepts(Encoding::Jpeg) && !only_jpeg.accepts(Encoding::Png));
    assert_eq!(
        (only_jpeg.max_bytes(), only_jpeg.max_long_edge_px()),
        (10, 20)
    );
    assert_eq!(Encoding::Webp.media_type(), "image/webp");
}
