//! Memory is bounded from the header, for every decoder, before a pixel is read.
//!
//! Each input here is a header and nothing else. A decoder that went on to
//! read pixels would answer `Undecodable`, because there are none; answering
//! `TooLargeToDecode` shows the refusal came first, from the size alone.
mod support;

use image::{ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use nessa_images::{normalize, normalize_with, DecodedImage, Error, Limits, PlatformDecoder};
use std::sync::Mutex;
use support::{crc32, encoded, turned_a_quarter, ALL, HEIC};

/// Room for any of these headers, so only the pixel count can refuse one.
fn limits(max_long_edge_px: u32) -> Limits {
    support::limits(&ALL, 1 << 20, max_long_edge_px)
}

/// A PNG signature and header chunk claiming `width` by `height`, then a data
/// chunk with nothing in it: a PNG's header is read as far as its first data.
fn png_header(width: u32, height: u32) -> Vec<u8> {
    let mut header = b"IHDR".to_vec();
    header.extend(width.to_be_bytes());
    header.extend(height.to_be_bytes());
    header.extend([8, 6, 0, 0, 0]); // eight bits, RGBA, no interlace
    let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
    for chunk in [header.as_slice(), b"IDAT"] {
        bytes.extend((chunk.len() as u32 - 4).to_be_bytes());
        bytes.extend(chunk);
        bytes.extend(crc32(chunk).to_be_bytes());
    }
    bytes
}

fn qoi_header(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"qoif".to_vec();
    bytes.extend(width.to_be_bytes());
    bytes.extend(height.to_be_bytes());
    bytes.extend([4, 0]); // RGBA, sRGB
    bytes
}

fn bmp_header(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"BM".to_vec();
    bytes.extend(54_u32.to_le_bytes()); // file size: the headers alone
    bytes.extend(0_u32.to_le_bytes());
    bytes.extend(54_u32.to_le_bytes()); // where pixels would start
    bytes.extend(40_u32.to_le_bytes()); // BITMAPINFOHEADER
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes()); // planes
    bytes.extend(24_u16.to_le_bytes()); // bits a pixel
    bytes.extend([0; 24]); // no compression, and nothing else stated
    bytes
}

/// Binary RGB, at one byte a channel for `max_value` 255 and two above it.
fn pnm_header(width: u32, height: u32, max_value: u32) -> Vec<u8> {
    format!("P6\n{width} {height}\n{max_value}\n").into_bytes()
}

fn hdr_header(width: u32, height: u32) -> Vec<u8> {
    format!("#?RADIANCE\nFORMAT=32-bit_rle_rgbe\n\n-Y {height} +X {width}\n").into_bytes()
}

/// An icon whose one entry is a PNG header. An entry states at most 256 px and
/// the image inside states its own size, which is the one that is decoded.
fn ico_header(width: u32, height: u32) -> Vec<u8> {
    let inner = png_header(width, height);
    let mut bytes = vec![0, 0, 1, 0, 1, 0]; // an icon, holding one image
    bytes.extend([0, 0, 0, 0]); // 256 by 256, no palette
    bytes.extend(1_u16.to_le_bytes()); // planes
    bytes.extend(32_u16.to_le_bytes()); // bits a pixel
    bytes.extend((inner.len() as u32).to_le_bytes());
    bytes.extend(22_u32.to_le_bytes()); // the image follows this entry
    bytes.extend(inner);
    bytes
}

/// An extended WebP header, whose canvas size is twenty-four bits each less
/// one, followed by a lossless image chunk with nothing in it.
fn webp_header(width: u32, height: u32) -> Vec<u8> {
    let mut bytes = b"RIFF".to_vec();
    bytes.extend(30_u32.to_le_bytes()); // everything after this field
    bytes.extend(b"WEBPVP8X");
    bytes.extend(10_u32.to_le_bytes());
    bytes.extend([0; 4]); // no alpha, animation, or metadata
    bytes.extend(&(width - 1).to_le_bytes()[..3]);
    bytes.extend(&(height - 1).to_le_bytes()[..3]);
    bytes.extend(b"VP8L");
    bytes.extend(0_u32.to_le_bytes());
    bytes
}

/// A real JPEG cut off where its pixels would begin, with the size in its frame
/// header overwritten.
fn jpeg_header(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = encoded(
        RgbImage::from_pixel(8, 8, Rgb([1, 2, 3])),
        ImageFormat::Jpeg,
    );
    let at = |marker: u8| {
        bytes
            .windows(2)
            .position(|pair| pair == [0xff, marker])
            .unwrap()
    };
    let (frame, scan) = (at(0xc0), at(0xda));
    let scan_header = usize::from(u16::from_be_bytes([bytes[scan + 2], bytes[scan + 3]]));
    bytes.truncate(scan + 2 + scan_header);
    // Marker, length, precision, then height and width.
    bytes[frame + 5..frame + 7].copy_from_slice(&height.to_be_bytes());
    bytes[frame + 7..frame + 9].copy_from_slice(&width.to_be_bytes());
    bytes
}

fn gif_header(width: u16, height: u16) -> Vec<u8> {
    let mut bytes = b"GIF89a".to_vec();
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.extend([0, 0, 0]); // no colour table
    bytes.push(0x2c); // one image, covering the whole screen
    bytes.extend([0, 0, 0, 0]);
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.push(0);
    bytes
}

#[test]
fn every_decoder_refuses_more_pixels_than_are_ever_decoded_from_the_header_alone() {
    // 16,384 px square is 268 megapixels, which is 1 GiB or more once decoded.
    // QOI, BMP, PNM, HDR, ICO, and WebP take no limit of their own, so nothing
    // but the count made here stands in front of them.
    for (name, header) in [
        ("png", png_header(16_384, 16_384)),
        ("qoi", qoi_header(16_384, 16_384)),
        ("bmp", bmp_header(16_384, 16_384)),
        ("pnm", pnm_header(16_384, 16_384, 255)),
        ("hdr", hdr_header(16_384, 16_384)),
        ("ico", ico_header(16_384, 16_384)),
        ("webp", webp_header(16_384, 16_384)),
        ("jpeg", jpeg_header(16_384, 16_384)),
        ("gif", gif_header(16_384, 16_384)),
    ] {
        for max_long_edge_px in [2_000, 20_000] {
            assert_eq!(
                normalize(&header, &limits(max_long_edge_px)),
                Err(Error::TooLargeToDecode),
                "{name} to {max_long_edge_px} px"
            );
        }
    }
}

#[test]
fn a_header_inside_the_pixel_limit_is_read_on_and_found_to_hold_no_image() {
    // The counterpart of the test above: the same headers claiming a size that
    // is allowed are decoded, and that is where they fail.
    for (name, header) in [
        ("png", png_header(64, 64)),
        ("qoi", qoi_header(64, 64)),
        ("bmp", bmp_header(64, 64)),
        ("pnm", pnm_header(64, 64, 255)),
        ("hdr", hdr_header(64, 64)),
        ("webp", webp_header(64, 64)),
    ] {
        assert_eq!(
            normalize(&header, &limits(2_000)),
            Err(Error::Undecodable),
            "{name}"
        );
    }
}

#[test]
fn pixels_that_are_few_enough_but_too_wide_to_hold_are_refused_too() {
    // 81 megapixels is inside the pixel limit. As floating point it decodes to
    // 12 bytes a pixel, 0.9 GiB, which is over the budget by itself.
    assert_eq!(
        normalize(&hdr_header(9_000, 9_000), &limits(2_000)),
        Err(Error::TooLargeToDecode)
    );
    // 96 megapixels at sixteen bits decodes to 0.54 GiB, inside the budget by
    // itself, and over it beside the eight-bit copy that is made from it.
    assert_eq!(
        normalize(&pnm_header(12_000, 8_000, 65_535), &limits(2_000)),
        Err(Error::TooLargeToDecode)
    );
    // The same pixels at eight bits are inside it: this one is read on, and
    // fails only because there is nothing after the header.
    assert_eq!(
        normalize(&pnm_header(12_000, 8_000, 255), &limits(2_000)),
        Err(Error::Undecodable)
    );
    // Asked for whole rather than scaled down, nothing is scaled at first, so
    // nothing is counted for scaling: this too is read on. Making it smaller
    // is counted when that is about to happen (see `tests/passthrough.rs`).
    assert_eq!(
        normalize(&pnm_header(12_000, 8_000, 255), &limits(12_000)),
        Err(Error::Undecodable)
    );
}

/// Hands back whatever it was built with, ignoring the edge it was asked for.
struct Oversized(u32, u32);
impl PlatformDecoder for Oversized {
    fn decode(&self, _: &[u8], _: u32) -> Result<DecodedImage, Error> {
        Ok(DecodedImage {
            pixels: RgbaImage::from_pixel(self.0, self.1, Rgba([1, 2, 3, 255])),
            lossless: false,
        })
    }
}

#[test]
fn a_system_decoder_is_never_asked_for_more_than_this_crate_will_hold() {
    struct Asked(Mutex<Vec<u32>>);
    impl PlatformDecoder for Asked {
        fn decode(&self, _: &[u8], edge: u32) -> Result<DecodedImage, Error> {
            self.0.lock().unwrap().push(edge);
            Err(Error::Undecodable)
        }
    }
    let asked = Asked(Mutex::new(Vec::new()));
    for max_long_edge_px in [1_200, 8_192, 8_193, u32::MAX] {
        let _ = normalize_with(&HEIC, &limits(max_long_edge_px), Some(&asked));
    }
    assert_eq!(*asked.0.lock().unwrap(), [1_200, 8_192, 8_192, 8_192]);

    // And one that returns more than it was asked for is scaled like anything else.
    let fitted = normalize_with(&HEIC, &limits(100), Some(&Oversized(400, 200))).unwrap();
    assert_eq!((fitted.width, fitted.height), (100, 50));
}

#[test]
fn turning_and_scaling_a_photo_gives_the_same_picture_in_either_order() {
    // Scaling happens before the turn, to hold less. The result is the one the
    // other order gives: red above blue, 200 by 400.
    let halves = RgbImage::from_fn(800, 400, |x, _| {
        if x < 400 {
            Rgb([255, 0, 0])
        } else {
            Rgb([0, 0, 255])
        }
    });
    let input = turned_a_quarter(&encoded(halves, ImageFormat::Jpeg));

    let fitted = normalize(&input, &limits(400)).unwrap();
    assert_eq!((fitted.width, fitted.height), (200, 400));
    let pixels = image::load_from_memory(&fitted.bytes).unwrap().to_rgb8();
    assert!(pixels.get_pixel(100, 50).0[0] > 200, "red on top");
    assert!(pixels.get_pixel(100, 350).0[2] > 200, "blue below");
}
