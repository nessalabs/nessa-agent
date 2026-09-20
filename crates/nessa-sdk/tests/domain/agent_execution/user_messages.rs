use super::support::*;
use nessa_sdk::domain::common::value_objects::Sha256Digest;

fn image(seed: u8, size: u64) -> ImageReference {
    ImageReference::new(
        Sha256Digest::from_bytes([seed; 32]),
        ImageMediaType::Png,
        size,
    )
    .unwrap()
}

#[test]
fn image_media_types_are_a_closed_exact_set() {
    for (text, kind) in [
        ("image/png", ImageMediaType::Png),
        ("image/jpeg", ImageMediaType::Jpeg),
        ("image/gif", ImageMediaType::Gif),
        ("image/webp", ImageMediaType::Webp),
    ] {
        assert_eq!(ImageMediaType::parse(text), Ok(kind));
        assert_eq!(kind.as_str(), text);
    }
    for rejected in [
        "",
        "image/PNG",
        "image/png; charset=binary",
        " image/png",
        "image/svg+xml",
        "image/heic",
        "application/pdf",
        "text/plain",
    ] {
        assert_eq!(
            ImageMediaType::parse(rejected),
            Err(ExecutionError::UnsupportedImageMediaType),
            "{rejected:?}"
        );
    }
}

#[test]
fn an_image_reference_is_nonempty_and_within_the_single_image_budget() {
    let digest = Sha256Digest::from_bytes([7; 32]);
    assert_eq!(
        ImageReference::new(digest, ImageMediaType::Jpeg, 0),
        Err(ExecutionError::EmptyValue("image"))
    );
    assert_eq!(
        ImageReference::new(digest, ImageMediaType::Jpeg, ImageReference::MAX_BYTES + 1),
        Err(ExecutionError::ValueTooLong {
            field: "image",
            max_bytes: ImageReference::MAX_BYTES as usize,
        })
    );
    let largest = ImageReference::new(digest, ImageMediaType::Jpeg, ImageReference::MAX_BYTES);
    let largest = largest.unwrap();
    assert_eq!(largest.digest(), digest);
    assert_eq!(largest.media_type(), ImageMediaType::Jpeg);
    assert_eq!(largest.size(), ImageReference::MAX_BYTES);
}

#[test]
fn a_user_message_needs_text_or_an_image() {
    assert_eq!(
        UserMessage::new(None, Vec::new()),
        Err(ExecutionError::EmptyValue("user message"))
    );

    let text = PromptText::new(" look \n").unwrap();
    let only_text = UserMessage::text_only(text.clone());
    assert_eq!(
        only_text,
        UserMessage::new(Some(text.clone()), Vec::new()).unwrap()
    );
    assert_eq!(only_text.text_str(), " look \n");
    assert!(only_text.images().is_empty());

    let only_image = UserMessage::new(None, vec![image(1, 10)]).unwrap();
    assert_eq!(only_image.text(), None);
    assert_eq!(only_image.text_str(), "");
    assert_eq!(only_image.images(), [image(1, 10)]);
}

#[test]
fn a_user_message_keeps_attachment_order_and_counts_repeats() {
    let message = UserMessage::new(None, vec![image(2, 5), image(1, 5), image(2, 5)]).unwrap();
    assert_eq!(message.images(), [image(2, 5), image(1, 5), image(2, 5)]);
    // Order is part of what was said, so it is part of equality.
    assert_ne!(
        message,
        UserMessage::new(None, vec![image(1, 5), image(2, 5), image(2, 5)]).unwrap()
    );
}

#[test]
fn a_user_message_bounds_image_count_and_total_bytes() {
    let at_count = (0..UserMessage::MAX_IMAGES as u8).map(|seed| image(seed, 1));
    assert!(UserMessage::new(None, at_count.collect()).is_ok());
    let over_count = (0..=UserMessage::MAX_IMAGES as u8).map(|seed| image(seed, 1));
    assert_eq!(
        UserMessage::new(None, over_count.collect()),
        Err(ExecutionError::TooManyValues {
            field: "user message images",
            max: UserMessage::MAX_IMAGES,
        })
    );

    // Two of the largest image fill the total exactly; one more byte does not fit.
    let full = vec![image(9, ImageReference::MAX_BYTES); 2];
    assert!(UserMessage::new(None, full.clone()).is_ok());
    let mut over = full;
    over.push(image(9, 1));
    assert_eq!(
        UserMessage::new(None, over),
        Err(ExecutionError::ValueTooLong {
            field: "user message images",
            max_bytes: UserMessage::MAX_IMAGE_BYTES as usize,
        })
    );
}
