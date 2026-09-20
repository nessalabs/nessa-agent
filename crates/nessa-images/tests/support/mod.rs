//! Fixtures the integration tests share: the limits they fit images to, and the
//! few byte sequences that have to be written out by hand.
//!
//! Each test binary compiles this module separately and uses the part of it that
//! its own subject needs, so unused helpers here are expected rather than dead.
#![allow(dead_code)]

use image::{DynamicImage, ImageFormat};
use nessa_images::{Encoding, Limits};
use std::io::Cursor;

/// Every encoding a consumer can accept. Most tests are about what happens to
/// an image rather than about a consumer being fussy.
pub const ALL: [Encoding; 4] = [Encoding::Png, Encoding::Jpeg, Encoding::Gif, Encoding::Webp];

/// The first bytes of an iPhone photo: an ISO media file of brand `heic`. Only
/// the header is needed, because nothing reaches a decoder without one.
pub const HEIC: [u8; 16] = [
    0, 0, 0, 24, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0, 0, 0, 0,
];

/// Limits that accept `accepted` and fit within the two numbers given.
pub fn limits(accepted: &[Encoding], max_bytes: u64, max_long_edge_px: u32) -> Limits {
    Limits::new(accepted.to_vec(), max_bytes, max_long_edge_px).unwrap()
}

/// `pixels` written in `format`.
pub fn encoded(pixels: impl Into<DynamicImage>, format: ImageFormat) -> Vec<u8> {
    let mut bytes = Cursor::new(Vec::new());
    pixels.into().write_to(&mut bytes, format).unwrap();
    bytes.into_inner()
}

/// The CRC-32 a PNG chunk carries, which is also what a hand-built PNG header
/// has to end with before any decoder will read it.
pub fn crc32(bytes: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in bytes {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320 & 0u32.wrapping_sub(crc & 1));
        }
    }
    !crc
}

/// `jpeg` with EXIF metadata in front of it saying "rotate a quarter turn
/// clockwise to view", which is how a camera records a photo taken on its side.
pub fn turned_a_quarter(jpeg: &[u8]) -> Vec<u8> {
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
    bytes.extend_from_slice(&jpeg[2..]);
    bytes
}
