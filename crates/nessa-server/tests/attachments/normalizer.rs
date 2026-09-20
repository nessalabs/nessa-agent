//! The real image library behind the port, with limits shaped like the catalog's.
use super::*;
use crate::attachments::application::{ImageNormalizer, NormalizeError};

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
    let normalizer = ModelImageNormalizer::new(Some(&claude())).unwrap();
    let upload = bitmap(40, 20, || [10, 20, 30]);
    // Whatever the client called it: the bytes say bitmap.
    for declared in ["image/bmp", "image/png", "image/heic"] {
        let kept = normalizer
            .normalize(upload.clone(), declared)
            .await
            .unwrap();
        assert_eq!(kept.media_type, "image/png", "{declared}");
        assert!(kept.bytes.starts_with(PNG_MAGIC), "{declared}");
    }
}

#[tokio::test]
async fn an_image_already_inside_the_models_limits_is_kept_byte_for_byte() {
    let normalizer = ModelImageNormalizer::new(Some(&claude())).unwrap();
    let png = normalizer
        .normalize(bitmap(40, 20, || [10, 20, 30]), "image/bmp")
        .await
        .unwrap();
    let again = normalizer
        .normalize(png.bytes.clone(), "image/png")
        .await
        .unwrap();
    assert_eq!(again, png);
}

#[tokio::test]
async fn the_long_edge_kept_is_the_smaller_of_what_the_model_sees_and_the_many_image_ceiling() {
    // Sees 2576 px, but a long conversation's images are held to 2000.
    let normalizer = ModelImageNormalizer::new(Some(&claude())).unwrap();
    let kept = normalizer
        .normalize(bitmap(2400, 10, || [1, 2, 3]), "image/bmp")
        .await
        .unwrap();
    assert_eq!(
        &kept.bytes[16..24],
        [2000_u32.to_be_bytes(), 8_u32.to_be_bytes()].concat()
    );

    // A model that sees less is sent less.
    let standard = model(vec![ImageMediaType::Png], 5_000_000, 1568);
    let normalizer = ModelImageNormalizer::new(Some(&standard)).unwrap();
    let kept = normalizer
        .normalize(bitmap(2400, 10, || [1, 2, 3]), "image/bmp")
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
    let normalizer = ModelImageNormalizer::new(Some(&tight)).unwrap();
    let kept = normalizer
        .normalize(bitmap(400, 400, noise()), "image/bmp")
        .await
        .unwrap();
    assert_eq!(kept.media_type, "image/jpeg");
    assert!(kept.bytes.len() <= 300_000, "{}", kept.bytes.len());
    assert!(kept.bytes.len().div_ceil(3) * 4 <= 400_000);
}

#[tokio::test]
async fn refusals_are_typed_by_whether_the_image_or_its_size_is_the_problem() {
    let normalizer = ModelImageNormalizer::new(Some(&claude())).unwrap();
    for unreadable in [&b"not an image"[..], b"", b"%PDF-1.7"] {
        assert_eq!(
            normalizer.normalize(unreadable.to_vec(), "image/png").await,
            Err(NormalizeError::Unsupported),
            "{unreadable:?}"
        );
    }
    let truncated = bitmap(400, 400, noise())[..1000].to_vec();
    assert_eq!(
        normalizer.normalize(truncated, "image/bmp").await,
        Err(NormalizeError::Unsupported)
    );

    // Nothing legible fits 1 500 bytes.
    let tiny = model(vec![ImageMediaType::Png, ImageMediaType::Jpeg], 2_000, 2576);
    let normalizer = ModelImageNormalizer::new(Some(&tiny)).unwrap();
    assert_eq!(
        normalizer
            .normalize(bitmap(400, 400, noise()), "image/bmp")
            .await,
        Err(NormalizeError::TooLarge)
    );
}

#[tokio::test]
async fn a_model_with_no_recorded_image_limits_has_no_image_prepared_for_it() {
    let normalizer = ModelImageNormalizer::new(None).unwrap();
    assert_eq!(
        normalizer
            .normalize(bitmap(4, 4, || [0, 0, 0]), "image/bmp")
            .await,
        // Not a bad image: a perfectly good one, for a model that is offered none.
        Err(NormalizeError::NotOffered)
    );
}

#[test]
fn a_model_that_takes_neither_png_nor_jpeg_cannot_be_prepared_for() {
    let only_gif = model(vec![ImageMediaType::Gif], 5_000_000, 2576);
    assert!(ModelImageNormalizer::new(Some(&only_gif)).is_err());
}
