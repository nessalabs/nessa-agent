//! What goes to the platform decoder, what never does, and what happens to
//! what it returns. A substitute stands in, so these run on every system.
use image::{DynamicImage, ImageFormat, Rgb, RgbImage, Rgba, RgbaImage};
use nessa_images::{normalize_with, DecodedImage, Encoding, Error, Limits, PlatformDecoder};
use std::{io::Cursor, sync::Mutex};

const ALL: [Encoding; 4] = [Encoding::Png, Encoding::Jpeg, Encoding::Gif, Encoding::Webp];

/// Answers every decode with `answer` and remembers what it was asked.
struct Substitute {
    answer: Result<DecodedImage, Error>,
    asked: Mutex<Vec<(usize, u32)>>,
}
impl Substitute {
    fn answering(answer: Result<DecodedImage, Error>) -> Self {
        Self {
            answer,
            asked: Mutex::new(Vec::new()),
        }
    }
    fn asked(&self) -> Vec<(usize, u32)> {
        self.asked.lock().unwrap().clone()
    }
}
impl PlatformDecoder for Substitute {
    fn decode(&self, input: &[u8], max_long_edge_px: u32) -> Result<DecodedImage, Error> {
        self.asked
            .lock()
            .unwrap()
            .push((input.len(), max_long_edge_px));
        self.answer.clone()
    }
}

fn photo(width: u32, height: u32) -> DecodedImage {
    DecodedImage {
        pixels: RgbaImage::from_fn(width, height, |x, y| {
            Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 255])
        }),
        lossless: false,
    }
}

fn limits(accepted: &[Encoding], max_bytes: u64, max_long_edge_px: u32) -> Limits {
    Limits::new(accepted.to_vec(), max_bytes, max_long_edge_px).unwrap()
}

/// The first bytes of an iPhone photo: an ISO media file of brand `heic`.
const HEIC: [u8; 16] = [
    0, 0, 0, 24, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0, 0, 0, 0,
];

#[test]
fn an_encoding_only_the_system_reads_is_decoded_there_and_fitted_here() {
    let platform = Substitute::answering(Ok(photo(300, 200)));
    let fitted = normalize_with(&HEIC, &limits(&ALL, 1 << 20, 1200), Some(&platform)).unwrap();
    // Asked once, for the whole input, bounded to what is worth sending.
    assert_eq!(platform.asked(), [(HEIC.len(), 1200)]);
    // A photograph: JPEG, not a PNG several times the size.
    assert_eq!(
        (fitted.encoding, fitted.width, fitted.height, fitted.changed),
        (Encoding::Jpeg, 300, 200, true)
    );
    assert_eq!(image::load_from_memory(&fitted.bytes).unwrap().width(), 300);
}

#[test]
fn what_the_system_returns_still_obeys_every_limit() {
    // A decoder that ignored the bound it was given is scaled here anyway.
    let platform = Substitute::answering(Ok(photo(900, 600)));
    let fitted = normalize_with(&HEIC, &limits(&ALL, 1 << 20, 300), Some(&platform)).unwrap();
    assert_eq!((fitted.width, fitted.height), (300, 200));

    // See-through pixels keep PNG; a consumer of PNG alone gets PNG.
    let mut see_through = photo(64, 64);
    see_through.pixels.put_pixel(0, 0, Rgba([0, 0, 0, 0]));
    let platform = Substitute::answering(Ok(see_through));
    let fitted = normalize_with(&HEIC, &limits(&ALL, 1 << 20, 300), Some(&platform)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Png);
    let platform = Substitute::answering(Ok(photo(64, 64)));
    let only_png = limits(&[Encoding::Png], 1 << 20, 300);
    let fitted = normalize_with(&HEIC, &only_png, Some(&platform)).unwrap();
    assert_eq!(fitted.encoding, Encoding::Png);
}

#[test]
fn without_a_system_decoder_those_encodings_are_refused_not_guessed() {
    assert_eq!(
        normalize_with(&HEIC, &limits(&ALL, 1 << 20, 1200), None),
        Err(Error::UnsupportedEncoding)
    );
}

#[test]
fn the_system_decoders_refusal_is_the_answer() {
    for refusal in [
        Error::UnsupportedEncoding,
        Error::Undecodable,
        Error::TooLargeToDecode,
    ] {
        let platform = Substitute::answering(Err(refusal));
        assert_eq!(
            normalize_with(
                b"not an image",
                &limits(&ALL, 1 << 20, 1200),
                Some(&platform)
            ),
            Err(refusal)
        );
    }
}

#[test]
fn encodings_read_here_never_reach_the_system_decoder() {
    let pixels = RgbImage::from_pixel(32, 16, Rgb([9, 8, 7]));
    for format in [
        ImageFormat::Png,
        ImageFormat::Jpeg,
        ImageFormat::Gif,
        ImageFormat::WebP,
        ImageFormat::Bmp,
        ImageFormat::Tiff,
        ImageFormat::Ico,
        ImageFormat::Qoi,
        ImageFormat::Pnm,
    ] {
        let mut input = Cursor::new(Vec::new());
        // An icon holds a PNG, and its decoder insists on one with an alpha channel.
        let source = if format == ImageFormat::Ico {
            DynamicImage::from(DynamicImage::from(pixels.clone()).to_rgba8())
        } else {
            DynamicImage::from(pixels.clone())
        };
        source.write_to(&mut input, format).unwrap();
        let platform = Substitute::answering(Err(Error::Undecodable));
        let fitted = normalize_with(
            input.get_ref(),
            &limits(&ALL, 1 << 20, 1200),
            Some(&platform),
        );
        assert_eq!(platform.asked(), [], "{format:?}");
        let fitted = fitted.unwrap_or_else(|error| panic!("{format:?}: {error:?}"));
        assert_eq!((fitted.width, fitted.height), (32, 16), "{format:?}");
    }
}

/// A TIFF header and one directory entry, in either byte order.
fn tiff_with_entry(little_endian: bool, tag: u16, value: u32) -> Vec<u8> {
    let u16_bytes = |value: u16| {
        if little_endian {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    };
    let u32_bytes = |value: u32| {
        if little_endian {
            value.to_le_bytes()
        } else {
            value.to_be_bytes()
        }
    };
    let mut bytes = if little_endian {
        b"II".to_vec()
    } else {
        b"MM".to_vec()
    };
    bytes.extend(u16_bytes(42));
    bytes.extend(u32_bytes(8));
    bytes.extend(u16_bytes(1));
    bytes.extend(u16_bytes(tag));
    bytes.extend(u16_bytes(4)); // LONG
    bytes.extend(u32_bytes(1));
    bytes.extend(u32_bytes(value));
    bytes.extend(u32_bytes(0));
    bytes
}

#[test]
fn a_camera_raw_in_a_tiff_container_is_not_read_as_its_small_preview() {
    for little_endian in [true, false] {
        for (tag, value) in [(0x00fe, 1), (0x014a, 64), (0xc612, 1)] {
            let raw = tiff_with_entry(little_endian, tag, value);
            let platform = Substitute::answering(Ok(photo(400, 300)));
            let fitted =
                normalize_with(&raw, &limits(&ALL, 1 << 20, 1200), Some(&platform)).unwrap();
            assert_eq!(platform.asked(), [(raw.len(), 1200)], "{tag:#x}");
            assert_eq!((fitted.width, fitted.height), (400, 300));
            // And with nothing to read it, it is refused rather than mistaken.
            assert_eq!(
                normalize_with(&raw, &limits(&ALL, 1 << 20, 1200), None),
                Err(Error::UnsupportedEncoding),
                "{tag:#x}"
            );
        }
        // A full-resolution first image is a plain TIFF and stays here: this
        // one is only a header, so it fails to decode rather than being sent on.
        let plain = tiff_with_entry(little_endian, 0x00fe, 0);
        let platform = Substitute::answering(Ok(photo(400, 300)));
        let refused = normalize_with(&plain, &limits(&ALL, 1 << 20, 1200), Some(&platform));
        assert_eq!(platform.asked(), []);
        assert!(refused.is_err());
    }
}
