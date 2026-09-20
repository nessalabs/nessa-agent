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
            normalize_with(&HEIC, &limits(&ALL, 1 << 20, 1200), Some(&platform)),
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

#[test]
fn only_what_begins_like_a_named_encoding_is_put_to_the_system_decoder() {
    let ftyp = |brand: &[u8; 4], compatible: &[u8; 4]| {
        let mut bytes = vec![0, 0, 0, 20];
        bytes.extend(b"ftyp");
        bytes.extend(brand);
        bytes.extend([0; 4]);
        bytes.extend(compatible);
        bytes
    };
    let jpeg_xl_container = [0, 0, 0, 12, b'J', b'X', b'L', b' ', 13, 10, 0x87, 10];
    for (name, input) in [
        ("heic", HEIC.to_vec()),
        ("avif", ftyp(b"avif", b"mif1")),
        ("heif by a compatible brand", ftyp(b"XXXX", b"mif1")),
        ("cr3", ftyp(b"crx ", b"isom")),
        ("jpeg xl", vec![0xff, 0x0a, 0, 0]),
        ("jpeg xl container", jpeg_xl_container.to_vec()),
        ("psd", b"8BPS\0\x01".to_vec()),
        ("raf", b"FUJIFILMCCD-RAW 0201".to_vec()),
        ("rw2", b"IIU\0\x08\0\0\0".to_vec()),
        ("orf", b"IIRO\x08\0\0\0".to_vec()),
        ("x3f", b"FOVb\0\0\0\0".to_vec()),
    ] {
        let platform = Substitute::answering(Ok(photo(40, 30)));
        let fitted = normalize_with(&input, &limits(&ALL, 1 << 20, 1200), Some(&platform));
        assert_eq!(platform.asked(), [(input.len(), 1200)], "{name}");
        assert!(fitted.is_ok(), "{name}");
    }
    // A substitute that would render anything, as ImageIO renders a PDF, is
    // never asked about any of these.
    for (name, input) in [
        ("nothing", Vec::new()),
        ("text", b"not an image at all".to_vec()),
        (
            "pdf",
            b"%PDF-1.7\n1 0 obj\n<< /Type /Catalog >>\nendobj\n".to_vec(),
        ),
        ("zip", b"PK\x03\x04\x14\0\0\0\x08\0".to_vec()),
        (
            "svg",
            b"<svg xmlns=\"http://www.w3.org/2000/svg\"></svg>".to_vec(),
        ),
        ("mp4 video", ftyp(b"isom", b"mp42")),
        (
            "a box too short to hold a brand",
            vec![0, 0, 0, 8, b'f', b't', b'y', b'p'],
        ),
    ] {
        let platform = Substitute::answering(Ok(photo(40, 30)));
        assert_eq!(
            normalize_with(&input, &limits(&ALL, 1 << 20, 1200), Some(&platform)),
            Err(Error::UnsupportedEncoding),
            "{name}"
        );
        assert_eq!(platform.asked(), [], "{name}");
    }
}

/// One TIFF directory entry holding a single SHORT or LONG.
#[derive(Clone, Copy)]
struct Entry {
    tag: u16,
    long: bool,
    value: u32,
}
const fn long(tag: u16, value: u32) -> Entry {
    Entry {
        tag,
        long: true,
        value,
    }
}
const fn short(tag: u16, value: u32) -> Entry {
    Entry {
        tag,
        long: false,
        value,
    }
}
const NEW_SUBFILE_TYPE: u16 = 0x00fe;
const IMAGE_WIDTH: u16 = 0x0100;
const IMAGE_LENGTH: u16 = 0x0101;
const MAKE: u16 = 0x010f;
const SUB_DIRECTORIES: u16 = 0x014a;
const DNG_VERSION: u16 = 0xc612;

/// A TIFF header and one directory of `entries`, in either byte order. There
/// are no pixels: read as a TIFF it fails, which is how these tests tell that
/// it stayed here.
fn tiff_with(little_endian: bool, entries: &[Entry]) -> Vec<u8> {
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
    bytes.extend(u16_bytes(entries.len() as u16));
    for entry in entries {
        bytes.extend(u16_bytes(entry.tag));
        bytes.extend(u16_bytes(if entry.long { 4 } else { 3 }));
        bytes.extend(u32_bytes(1));
        if entry.long {
            bytes.extend(u32_bytes(entry.value));
        } else {
            // A SHORT sits at the start of the four-byte value field.
            bytes.extend(u16_bytes(entry.value as u16));
            bytes.extend([0, 0]);
        }
    }
    bytes.extend(u32_bytes(0));
    bytes
}

#[test]
fn a_camera_raw_in_a_tiff_container_is_not_read_as_its_small_preview() {
    let camera_thumbnail = [
        short(IMAGE_WIDTH, 160),
        long(IMAGE_LENGTH, 120),
        long(MAKE, 64),
        long(SUB_DIRECTORIES, 64),
    ];
    let camera_photo = [
        long(IMAGE_WIDTH, 6000),
        long(IMAGE_LENGTH, 4000),
        long(MAKE, 64),
        long(SUB_DIRECTORIES, 64),
    ];
    for little_endian in [true, false] {
        for (name, entries) in [
            (
                "says it is a reduced copy",
                &[long(NEW_SUBFILE_TYPE, 1)][..],
            ),
            ("says it is a DNG", &[long(DNG_VERSION, 1)]),
            (
                "a camera's thumbnail over other directories",
                &camera_thumbnail,
            ),
        ] {
            let raw = tiff_with(little_endian, entries);
            let platform = Substitute::answering(Ok(photo(400, 300)));
            let fitted =
                normalize_with(&raw, &limits(&ALL, 1 << 20, 1200), Some(&platform)).unwrap();
            assert_eq!(platform.asked(), [(raw.len(), 1200)], "{name}");
            assert_eq!((fitted.width, fitted.height), (400, 300));
            // And with nothing to read it, it is refused rather than mistaken.
            assert_eq!(
                normalize_with(&raw, &limits(&ALL, 1 << 20, 1200), None),
                Err(Error::UnsupportedEncoding),
                "{name}"
            );
        }
        // None of these proves a preview, so each stays here. They are only
        // headers, so they fail to decode rather than being sent on.
        for (name, entries) in [
            (
                "a full-resolution first image",
                &[long(NEW_SUBFILE_TYPE, 0)][..],
            ),
            (
                "other directories alone, as a pyramid has",
                &[long(SUB_DIRECTORIES, 64)],
            ),
            (
                "a thumbnail-sized plain TIFF",
                &[short(IMAGE_WIDTH, 160), short(IMAGE_LENGTH, 120)],
            ),
            (
                "a camera's whole photo over other directories",
                &camera_photo,
            ),
        ] {
            let plain = tiff_with(little_endian, entries);
            let platform = Substitute::answering(Ok(photo(400, 300)));
            let refused = normalize_with(&plain, &limits(&ALL, 1 << 20, 1200), Some(&platform));
            assert_eq!(platform.asked(), [], "{name}");
            assert!(refused.is_err(), "{name}");
        }
    }
    // Canon's CR2 marks itself only with two letters after the header.
    let mut cr2 = tiff_with(true, &[long(IMAGE_WIDTH, 5184)]);
    cr2.splice(8..8, *b"CR\x02\0");
    cr2[4..8].copy_from_slice(&12_u32.to_le_bytes());
    let platform = Substitute::answering(Ok(photo(400, 300)));
    normalize_with(&cr2, &limits(&ALL, 1 << 20, 1200), Some(&platform)).unwrap();
    assert_eq!(platform.asked(), [(cr2.len(), 1200)]);
}

/// `tiff`, a real encoded TIFF, with `extra` LONG entries added to its first
/// directory. The directory is rewritten at the end of the file, in tag order
/// as TIFF requires, and the header pointed at it; the pixels are untouched.
fn with_directory_entries(tiff: &[u8], extra: &[(u16, u32)]) -> Vec<u8> {
    assert_eq!(
        &tiff[..4],
        b"II*\0",
        "the encoder writes little-endian TIFF"
    );
    let directory = u32::from_le_bytes(tiff[4..8].try_into().unwrap()) as usize;
    let count = usize::from(u16::from_le_bytes([tiff[directory], tiff[directory + 1]]));
    let mut entries: Vec<[u8; 12]> = tiff[directory + 2..directory + 2 + count * 12]
        .as_chunks::<12>()
        .0
        .to_vec();
    for (tag, value) in extra {
        let mut entry = [0; 12];
        entry[..2].copy_from_slice(&tag.to_le_bytes());
        entry[2..4].copy_from_slice(&4_u16.to_le_bytes());
        entry[4..8].copy_from_slice(&1_u32.to_le_bytes());
        entry[8..].copy_from_slice(&value.to_le_bytes());
        entries.push(entry);
    }
    entries.sort_by_key(|entry| u16::from_le_bytes([entry[0], entry[1]]));
    let mut bytes = tiff.to_vec();
    if bytes.len() % 2 == 1 {
        bytes.push(0);
    }
    let moved = bytes.len() as u32;
    bytes[4..8].copy_from_slice(&moved.to_le_bytes());
    bytes.extend((entries.len() as u16).to_le_bytes());
    bytes.extend(entries.concat());
    bytes.extend(0_u32.to_le_bytes());
    bytes
}

#[test]
fn a_pyramidal_tiff_is_read_here_and_whole() {
    // A real TIFF whose first directory also lists other directories, as the
    // smaller levels of a pyramid are listed. The first image is the picture.
    let pixels = RgbImage::from_fn(1100, 8, |x, _| Rgb([(x % 256) as u8, 7, 9]));
    let mut plain = Cursor::new(Vec::new());
    DynamicImage::from(pixels.clone())
        .write_to(&mut plain, ImageFormat::Tiff)
        .unwrap();
    let first_directory = u32::from_le_bytes(plain.get_ref()[4..8].try_into().unwrap());
    for (name, extra) in [
        (
            "other directories",
            &[(SUB_DIRECTORIES, first_directory)][..],
        ),
        (
            "other directories and a scanner's name",
            &[(SUB_DIRECTORIES, first_directory), (MAKE, 0x0041_4243)],
        ),
    ] {
        let pyramid = with_directory_entries(plain.get_ref(), extra);
        let platform = Substitute::answering(Err(Error::Undecodable));
        let fitted = normalize_with(
            &pyramid,
            &limits(&[Encoding::Png], 1 << 20, 4096),
            Some(&platform),
        )
        .unwrap_or_else(|error| panic!("{name}: {error:?}"));
        assert_eq!(platform.asked(), [], "{name}");
        assert_eq!(
            image::load_from_memory(&fitted.bytes).unwrap().to_rgb8(),
            pixels,
            "{name}"
        );
    }
}
