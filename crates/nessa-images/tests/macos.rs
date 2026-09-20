//! The real ImageIO decoder, against files the system's own `sips` tool writes.
#![cfg(target_os = "macos")]
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use nessa_images::{normalize, platform_decoder, Encoding, Error, Limits};
use std::{fs, process::Command};

const ALL: [Encoding; 4] = [Encoding::Png, Encoding::Jpeg, Encoding::Gif, Encoding::Webp];

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

    let limits = Limits::new(ALL.to_vec(), 1 << 20, 400).unwrap();
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
    let limits = Limits::new(ALL.to_vec(), 1 << 20, 2000).unwrap();
    let fitted = normalize(&heic, &limits).unwrap();
    assert_eq!((fitted.width, fitted.height), (120, 60));
}

#[test]
fn bytes_the_system_cannot_read_either_are_refused() {
    let limits = Limits::new(ALL.to_vec(), 1 << 20, 2000).unwrap();
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
