//! What the strict decoder must keep accepting, and what it is there to refuse.
//!
//! Strict mode is the one thing in this crate that can refuse a file no decoder
//! has anything against, so the shapes real cameras and editors write are
//! written out here and checked. Each is a whole file: if one of these ever
//! starts failing, images people take every day stop passing through untouched.
use super::check_whole;
use crate::Error;
use image::{DynamicImage, ImageFormat, Rgb, RgbImage};
use std::io::Cursor;

/// A plain baseline JPEG of `width` by `height`, as this crate's own encoder
/// writes one: three components, no restarts, nothing between the headers.
fn plain(width: u32, height: u32) -> Vec<u8> {
    let pixels = RgbImage::from_fn(width, height, |x, y| {
        Rgb([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8])
    });
    let mut bytes = Cursor::new(Vec::new());
    DynamicImage::from(pixels)
        .write_to(&mut bytes, ImageFormat::Jpeg)
        .unwrap();
    bytes.into_inner()
}

/// Where `input`'s scan begins: the start of its `SOS` marker.
fn scan_start(input: &[u8]) -> usize {
    input
        .windows(2)
        .position(|pair| pair == [0xff, 0xda])
        .expect("a baseline JPEG has a scan")
}

/// Whether `input` holds a marker segment of `marker`, reading only the header
/// segments in front of the scan.
fn has_marker(input: &[u8], marker: u8) -> bool {
    let mut at = 2;
    while at + 4 <= input.len() && input[at] == 0xff {
        if input[at + 1] == marker {
            return true;
        }
        if input[at + 1] == 0xda {
            return false;
        }
        at += 2 + usize::from(u16::from_be_bytes([input[at + 2], input[at + 3]]));
    }
    false
}

#[test]
fn a_whole_jpeg_is_vouched_for() {
    let whole = plain(64, 48);
    assert_eq!(check_whole(&whole, 64, 48), Ok(()));
}

#[test]
fn a_jpeg_cut_short_inside_its_scan_is_refused_although_it_still_renders() {
    let whole = plain(400, 400);
    let cut = &whole[..whole.len() * 3 / 4];
    // This is the whole reason the check exists: the forgiving decoder every
    // browser behaves like hands back a picture for these bytes.
    assert!(image::load_from_memory(cut).is_ok());
    assert_eq!(check_whole(cut, 400, 400), Err(Error::Undecodable));
}

#[test]
fn padding_and_a_comment_between_the_headers_are_not_damage() {
    // Fill bytes before a marker are allowed by the format and written by more
    // than one encoder, and a comment segment can appear anywhere a marker can.
    let whole = plain(32, 32);
    let scan = scan_start(&whole);

    let mut padded = whole.clone();
    padded.splice(scan..scan, [0xff; 8]);
    assert_eq!(check_whole(&padded, 32, 32), Ok(()));

    let mut commented = whole.clone();
    let mut segment = vec![0xff, 0xfe, 0x00, 0x10];
    segment.extend(b"written by hand");
    commented.splice(scan..scan, segment);
    assert_eq!(check_whole(&commented, 32, 32), Ok(()));
}

/// A CMYK JPEG written by the system, carrying what a print-bound file carries:
/// an `Adobe` APP14 marker, an embedded ICC profile, an EXIF block, a Photoshop
/// resource block, and a restart interval. `sips` is the system's own converter,
/// so this is a file macOS itself considers ordinary.
#[cfg(target_os = "macos")]
fn adobe_cmyk(width: u32, height: u32) -> Vec<u8> {
    use std::{fs, process::Command};
    let directory = tempfile::tempdir().unwrap();
    let source = directory.path().join("source.png");
    let target = directory.path().join("target.jpg");
    let pixels = RgbImage::from_fn(width, height, |x, y| {
        Rgb([(x * 4 % 256) as u8, (y * 4 % 256) as u8, 128])
    });
    DynamicImage::from(pixels)
        .save_with_format(&source, ImageFormat::Png)
        .unwrap();
    let converted = Command::new("/usr/bin/sips")
        .args([
            "--matchTo",
            "/System/Library/ColorSync/Profiles/Generic CMYK Profile.icc",
        ])
        .args(["-s", "format", "jpeg"])
        .arg(&source)
        .arg("--out")
        .arg(&target)
        .output()
        .unwrap();
    assert!(
        converted.status.success(),
        "{}",
        String::from_utf8_lossy(&converted.stderr)
    );
    fs::read(target).unwrap()
}

#[cfg(target_os = "macos")]
#[test]
fn an_adobe_cmyk_jpeg_with_restart_markers_is_vouched_for() {
    let cmyk = adobe_cmyk(64, 64);
    // The fixture is only worth anything if it really is that kind of file.
    assert!(has_marker(&cmyk, 0xee), "no Adobe APP14 marker");
    assert!(has_marker(&cmyk, 0xe2), "no embedded ICC profile");
    assert!(has_marker(&cmyk, 0xdd), "no restart interval");
    assert!(has_marker(&cmyk, 0xe1), "no EXIF block");
    assert_eq!(check_whole(&cmyk, 64, 64), Ok(()));
}
