//! The real ImageIO decoder, against files the system's own `sips` tool writes.
//! The adapter's own pixel arithmetic is covered beside it, in
//! `tests/unit/platform/macos.rs`, where no file has to be opaque.
#![cfg(target_os = "macos")]
mod support;

use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use nessa_images::{normalize, platform_decoder, Encoding, Error};
use std::{fs, process::Command};
use support::{limits, ALL};

/// Left half red, right half blue, written as PNG and converted by `sips`.
fn converted(width: u32, height: u32, format: &str, extension: &str) -> Vec<u8> {
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.png");
    let target = directory.path().join(format!("target.{extension}"));
    let pixels = RgbImage::from_fn(width, height, |x, _| {
        if x < width / 2 {
            Rgb([255, 0, 0])
        } else {
            Rgb([0, 0, 255])
        }
    });
    DynamicImage::from(pixels)
        .save_with_format(&source, ImageFormat::Png)
        .unwrap();
    let status = Command::new("/usr/bin/sips")
        .args(["-s", "format", format])
        .arg(&source)
        .arg("--out")
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        status.status.success(),
        "{}",
        String::from_utf8_lossy(&status.stderr)
    );
    fs::read(target).unwrap()
}

#[test]
fn a_heic_photo_becomes_an_upright_jpeg_inside_the_limits() {
    assert!(platform_decoder().is_some());
    let heic = converted(1600, 800, "heic", "heic");
    assert_eq!(&heic[4..8], b"ftyp");

    let limits = limits(&ALL, 1 << 20, 400);
    let fitted = normalize(&heic, &limits).unwrap();
    assert_eq!(
        (fitted.encoding, fitted.width, fitted.height, fitted.changed),
        (Encoding::Jpeg, 400, 200, true)
    );
    let pixels = image::load_from_memory(&fitted.bytes).unwrap().to_rgb8();
    let (left, right) = (pixels.get_pixel(50, 100).0, pixels.get_pixel(350, 100).0);
    assert!(left[0] > 200 && left[2] < 60, "red on the left: {left:?}");
    assert!(
        right[2] > 200 && right[0] < 60,
        "blue on the right: {right:?}"
    );
}

#[test]
fn a_small_heic_is_not_scaled_up() {
    let heic = converted(120, 60, "heic", "heic");
    let limits = limits(&ALL, 1 << 20, 2000);
    let fitted = normalize(&heic, &limits).unwrap();
    assert_eq!((fitted.width, fitted.height), (120, 60));
}

#[test]
fn bytes_the_system_cannot_read_either_are_refused() {
    let limits = limits(&ALL, 1 << 20, 2000);
    for input in [&b"not an image at all"[..], b"%PDF-1.7\n", b""] {
        assert_eq!(
            normalize(input, &limits),
            Err(Error::UnsupportedEncoding),
            "{input:?}"
        );
    }
    // A HEIC cut short: recognised, then unreadable.
    let heic = converted(400, 200, "heic", "heic");
    let refused = normalize(&heic[..heic.len() / 3], &limits);
    assert!(
        matches!(
            refused,
            Err(Error::Undecodable | Error::UnsupportedEncoding)
        ),
        "{refused:?}"
    );
}

#[test]
fn the_system_decoder_itself_refuses_what_this_crate_never_said_it_reads() {
    // Straight to the adapter, past the first-bytes check `normalize` makes, so
    // this is ImageIO's own answer being held to the list. ImageIO renders
    // every one of the first four perfectly well, which is the point.
    let decoder = platform_decoder().expect("macOS wraps ImageIO");
    let pdf = converted(200, 200, "pdf", "pdf");
    assert!(pdf.starts_with(b"%PDF"));
    let png = converted(200, 200, "png", "png");
    let tiff = converted(200, 200, "tiff", "tiff");
    let bmp = converted(200, 200, "bmp", "bmp");
    // An empty archive, and text.
    let mut zip = b"PK\x05\x06".to_vec();
    zip.extend([0; 18]);
    for (name, input) in [
        ("pdf", pdf.as_slice()),
        ("png", &png),
        ("tiff", &tiff),
        ("bmp", &bmp),
        ("zip", &zip),
        ("text", b"just some words, not an image"),
    ] {
        assert_eq!(
            decoder.decode(input, 4096).err(),
            Some(Error::UnsupportedEncoding),
            "{name}"
        );
    }
    // The same PDF through the front door is refused before ImageIO is asked.
    let limits = limits(&ALL, 1 << 20, 2000);
    assert_eq!(normalize(&pdf, &limits), Err(Error::UnsupportedEncoding));
    // And what is on the list still decodes.
    let heic = converted(400, 200, "heic", "heic");
    assert_eq!(decoder.decode(&heic, 4096).unwrap().pixels.width(), 400);
}

/// A whole Photoshop file of `width` by `height` white pixels, run-length
/// packed so that a hundred megapixels are a few megabytes.
fn white_psd(width: u32, height: u32) -> Vec<u8> {
    let mut psd = b"8BPS".to_vec();
    psd.extend(1_u16.to_be_bytes());
    psd.extend([0; 6]);
    psd.extend(3_u16.to_be_bytes());
    psd.extend(height.to_be_bytes());
    psd.extend(width.to_be_bytes());
    psd.extend(8_u16.to_be_bytes());
    psd.extend(3_u16.to_be_bytes());
    // Colour mode data, image resources, layers: all empty.
    psd.extend([0; 12]);
    // PackBits: a count byte of `1 - n` repeats the next byte `n` times.
    let mut row = Vec::new();
    let mut left = width;
    while left > 0 {
        let run = left.min(128);
        row.extend([(1 - run as i32) as u8, 0xff]);
        left -= run;
    }
    psd.extend(1_u16.to_be_bytes());
    let rows = height as usize * 3;
    for _ in 0..rows {
        psd.extend((row.len() as u16).to_be_bytes());
    }
    for _ in 0..rows {
        psd.extend(&row);
    }
    psd
}

#[test]
fn a_source_claiming_more_pixels_than_are_ever_decoded_is_refused_from_its_header() {
    let decoder = platform_decoder().expect("macOS wraps ImageIO");
    // The same file at a size inside the budget decodes, so the refusal below
    // is about the size and not about the file.
    let small = decoder.decode(&white_psd(300, 200), 4096).unwrap();
    assert_eq!(small.pixels.dimensions(), (300, 200));
    assert_eq!(small.pixels.get_pixel(150, 100).0, [255; 4]);

    // 108 megapixels. Asked for 256 px, ImageIO would still have to decode
    // them all to scale them down; it is never asked.
    let huge = white_psd(12_000, 9_000);
    assert_eq!(
        decoder.decode(&huge, 256).err(),
        Some(Error::TooLargeToDecode)
    );
    let limits = limits(&ALL, 1 << 20, 256);
    assert_eq!(normalize(&huge, &limits), Err(Error::TooLargeToDecode));
    // Cut short after its header and row table, there is nothing to decode:
    // an attempt would answer `Undecodable`. The size is still what is refused.
    let header_only = &huge[..26 + 12 + 2 + 2 * 27_000];
    assert_eq!(
        decoder.decode(header_only, 256).err(),
        Some(Error::TooLargeToDecode)
    );
}

#[test]
fn the_edge_asked_of_the_system_is_bounded_whatever_the_caller_asks() {
    let decoder = platform_decoder().expect("macOS wraps ImageIO");
    let heic = converted(400, 200, "heic", "heic");
    // An absurd request is neither an overflow nor an allocation: it is clamped
    // to what this crate will hold, and nothing is scaled up to meet it.
    for (asked, expected) in [(100, (100, 50)), (u32::MAX, (400, 200))] {
        let decoded = decoder.decode(&heic, asked).unwrap();
        assert_eq!(decoded.pixels.dimensions(), expected, "{asked}");
    }
}
