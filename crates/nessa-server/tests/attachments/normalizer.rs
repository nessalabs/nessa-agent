//! The real image library behind the port, with limits shaped like the
//! catalog's, and the system decoder that only composition supplies.
use super::*;
use crate::attachments::application::{ImageNormalizer, NormalizeError};
use image::{Rgba, RgbaImage};
use nessa_images::DecodedImage;

/// What only the operating system can read: the first bytes of an iPhone
/// photograph, an ISO media file of brand `heic`. Nothing else reaches a
/// system decoder, so this is what a substitute is asked about.
const HEIC: [u8; 16] = [
    0, 0, 0, 24, b'f', b't', b'y', b'p', b'h', b'e', b'i', b'c', 0, 0, 0, 0,
];

/// A system decoder that always refuses, as one does for a file its own
/// operating system cannot read either.
struct Refusing;
impl PlatformDecoder for Refusing {
    fn decode(&self, _: &[u8], _: u32) -> Result<DecodedImage, Error> {
        Err(Error::UnsupportedEncoding)
    }
}
static REFUSING: Refusing = Refusing;

/// A system decoder that ignores the edge it was asked for and hands back
/// far more pixels than that, as a decoder scaling on its own terms may.
struct Oversized;
impl PlatformDecoder for Oversized {
    fn decode(&self, _: &[u8], _: u32) -> Result<DecodedImage, Error> {
        Ok(DecodedImage {
            pixels: RgbaImage::from_fn(900, 600, |x, y| {
                Rgba([(x % 251) as u8, (y % 241) as u8, ((x + y) % 239) as u8, 255])
            }),
        })
    }
}
static OVERSIZED: Oversized = Oversized;

/// A system decoder that stops dead, as an operating system library handed
/// bytes it did not expect can.
struct Panicking;
impl PlatformDecoder for Panicking {
    fn decode(&self, _: &[u8], _: u32) -> Result<DecodedImage, Error> {
        panic!("the system decoder stopped")
    }
}
static PANICKING: Panicking = Panicking;

fn fitting(model: &ImageInputLimits) -> ModelImageNormalizer {
    ModelImageNormalizer::new(Some(model), None).unwrap()
}

fn model(
    media_types: Vec<ImageMediaType>,
    max_encoded_bytes: u64,
    long_edge: u32,
) -> ImageInputLimits {
    ImageInputLimits::new(media_types, max_encoded_bytes, 8000, 2000, long_edge).unwrap()
}
fn claude() -> ImageInputLimits {
    use ImageMediaType::{Gif, Jpeg, Png, Webp};
    model(vec![Jpeg, Png, Gif, Webp], 5_000_000, 2576)
}

/// An uncompressed bitmap: the one image encoding simple enough to write by hand.
fn bitmap(width: u32, height: u32, mut pixel: impl FnMut() -> [u8; 3]) -> Vec<u8> {
    let row = (width * 3).div_ceil(4) * 4;
    let mut bytes = b"BM".to_vec();
    bytes.extend((54 + row * height).to_le_bytes());
    bytes.extend([0; 4]);
    bytes.extend(54_u32.to_le_bytes());
    bytes.extend(40_u32.to_le_bytes());
    bytes.extend(width.to_le_bytes());
    bytes.extend(height.to_le_bytes());
    bytes.extend(1_u16.to_le_bytes());
    bytes.extend(24_u16.to_le_bytes());
    bytes.extend([0; 24]);
    for _ in 0..height {
        for _ in 0..width {
            bytes.extend(pixel());
        }
        bytes.resize(bytes.len() + (row - width * 3) as usize, 0);
    }
    bytes
}
fn noise() -> impl FnMut() -> [u8; 3] {
    let mut state = 0x2545_f491_u32;
    move || {
        let mut channel = || {
            state = state.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            (state >> 24) as u8
        };
        [channel(), channel(), channel()]
    }
}

const PNG_MAGIC: &[u8] = b"\x89PNG\r\n\x1a\n";

#[tokio::test]
async fn an_encoding_the_model_does_not_take_is_kept_as_one_it_does() {
    // What an upload is comes from its bytes: these say bitmap, and the model
    // takes no bitmaps.
    let kept = fitting(&claude())
        .normalize(bitmap(40, 20, || [10, 20, 30]))
        .await
        .unwrap();
    assert_eq!(kept.media_type, "image/png");
    assert!(kept.bytes.starts_with(PNG_MAGIC));

    // An image already inside every limit is kept byte for byte.
    let again = fitting(&claude())
        .normalize(kept.bytes.clone())
        .await
        .unwrap();
    assert_eq!(again, kept);
}

#[tokio::test]
async fn the_long_edge_kept_is_the_smaller_of_what_the_model_sees_and_the_many_image_ceiling() {
    // Sees 2576 px, but a long conversation's images are held to 2000.
    let kept = fitting(&claude())
        .normalize(bitmap(2400, 10, || [1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(
        &kept.bytes[16..24],
        [2000_u32.to_be_bytes(), 8_u32.to_be_bytes()].concat()
    );

    // A model that sees less is sent less.
    let standard = model(vec![ImageMediaType::Png], 5_000_000, 1568);
    let kept = fitting(&standard)
        .normalize(bitmap(2400, 10, || [1, 2, 3]))
        .await
        .unwrap();
    assert_eq!(kept.bytes[16..20], 1568_u32.to_be_bytes());
}

#[tokio::test]
async fn the_byte_limit_is_the_models_base64_limit_counted_in_stored_bytes() {
    // 400 000 base64 characters hold 300 000 bytes. Noise this size is 480 KB
    // as a bitmap and over a megabyte as PNG, so only JPEG gets under.
    let tight = model(
        vec![ImageMediaType::Png, ImageMediaType::Jpeg],
        400_000,
        2576,
    );
    let kept = fitting(&tight)
        .normalize(bitmap(400, 400, noise()))
        .await
        .unwrap();
    assert_eq!(kept.media_type, "image/jpeg");
    assert!(kept.bytes.len() <= 300_000, "{}", kept.bytes.len());
    assert!(kept.bytes.len().div_ceil(3) * 4 <= 400_000);
}

#[tokio::test]
async fn refusals_are_typed_by_whether_the_image_or_its_size_is_the_problem() {
    let normalizer = fitting(&claude());
    for unreadable in [&b"not an image"[..], b"", b"%PDF-1.7"] {
        assert_eq!(
            normalizer.normalize(unreadable.to_vec()).await,
            Err(NormalizeError::Unsupported),
            "{unreadable:?}"
        );
    }
    let truncated = bitmap(400, 400, noise())[..1000].to_vec();
    assert_eq!(
        normalizer.normalize(truncated).await,
        Err(NormalizeError::Unsupported)
    );

    // Nothing legible fits 1 500 bytes.
    let tiny = model(vec![ImageMediaType::Png, ImageMediaType::Jpeg], 2_000, 2576);
    assert_eq!(
        fitting(&tiny).normalize(bitmap(400, 400, noise())).await,
        Err(NormalizeError::TooLarge)
    );
}

#[tokio::test]
async fn a_model_with_no_recorded_image_limits_is_offered_none_and_prepares_none() {
    let normalizer = ModelImageNormalizer::new(None, None).unwrap();
    assert!(!normalizer.offers_images());
    assert_eq!(
        normalizer.normalize(bitmap(4, 4, || [0, 0, 0])).await,
        // Not a bad image: a perfectly good one, for a model offered none.
        Err(NormalizeError::NotOffered)
    );
    assert!(fitting(&claude()).offers_images());
}

#[test]
fn a_model_that_takes_neither_png_nor_jpeg_cannot_be_prepared_for() {
    let only_gif = model(vec![ImageMediaType::Gif], 5_000_000, 2576);
    assert!(ModelImageNormalizer::new(Some(&only_gif), None).is_err());
}

#[tokio::test]
async fn an_encoding_only_the_system_reads_goes_to_the_decoder_composition_supplied() {
    // Without one, a photograph nothing here can read is simply unreadable.
    assert_eq!(
        ModelImageNormalizer::new(Some(&claude()), None)
            .unwrap()
            .normalize(HEIC.to_vec())
            .await,
        Err(NormalizeError::Unsupported)
    );

    // What the system hands back is fitted here, to this model's own edge,
    // however many pixels it chose to return.
    let normalizer = ModelImageNormalizer::new(Some(&claude()), Some(&OVERSIZED)).unwrap();
    let kept = normalizer.normalize(HEIC.to_vec()).await.unwrap();
    assert_eq!(kept.media_type, "image/jpeg");
    let decoded = image::load_from_memory(&kept.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (900, 600));

    // A model that sees a shorter edge than the decoder returned gets less.
    let narrow = model(
        vec![ImageMediaType::Jpeg, ImageMediaType::Png],
        5_000_000,
        300,
    );
    let normalizer = ModelImageNormalizer::new(Some(&narrow), Some(&OVERSIZED)).unwrap();
    let kept = normalizer.normalize(HEIC.to_vec()).await.unwrap();
    let decoded = image::load_from_memory(&kept.bytes).unwrap();
    assert_eq!((decoded.width(), decoded.height()), (300, 200));
}

#[tokio::test]
async fn a_system_decoder_that_refuses_or_stops_is_this_uploads_failure_only() {
    // The system cannot read it either: not a readable image.
    let refusing = ModelImageNormalizer::new(Some(&claude()), Some(&REFUSING)).unwrap();
    assert_eq!(
        refusing.normalize(HEIC.to_vec()).await,
        Err(NormalizeError::Unsupported)
    );
    // And the next upload is answered as ever; nothing was left broken.
    assert_eq!(
        refusing
            .normalize(bitmap(40, 20, || [10, 20, 30]))
            .await
            .unwrap()
            .media_type,
        "image/png"
    );

    // A decoder that stops dead is a failure the same upload may survive next
    // time, and it takes nothing else with it.
    let panicking = ModelImageNormalizer::new(Some(&claude()), Some(&PANICKING)).unwrap();
    assert_eq!(
        panicking.normalize(HEIC.to_vec()).await,
        Err(NormalizeError::Failed)
    );
    assert_eq!(
        panicking
            .normalize(bitmap(40, 20, || [10, 20, 30]))
            .await
            .unwrap()
            .media_type,
        "image/png"
    );
}
