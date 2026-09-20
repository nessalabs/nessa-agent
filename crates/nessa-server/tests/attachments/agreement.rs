//! Facts this context spells in one place and other places spell again: the
//! media type an image library names and the one a message may name, and the
//! numbers the published protocol schema states. Each is valid on its own;
//! these check that together they describe the same gateway.
use crate::attachments::domain::{Attachment, MediaType};
use nessa_images::Encoding;
use nessa_sdk::domain::{
    agent_execution::prompts::{ImageReference, UserMessage},
    common::value_objects::ImageMediaType,
};
use serde_json::Value;
use std::collections::BTreeSet;

/// The published contract, read as data rather than restated here.
fn schema() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    )))
    .expect("the product schema is valid JSON")
}
fn definition<'a>(schema: &'a Value, name: &str) -> &'a Value {
    &schema["$defs"][name]
}

#[test]
fn every_encoding_the_normalizer_can_answer_is_one_a_message_may_name() {
    // The normalizer hands back the image library's word for an encoding, and
    // the service turns that into a stored file. Anything the library can
    // answer must survive that trip unchanged, or an upload would be refused
    // for a normalization that in fact succeeded.
    for encoding in [Encoding::Png, Encoding::Jpeg, Encoding::Gif, Encoding::Webp] {
        let named = encoding.media_type();
        let media_type = MediaType::parse(named).expect(named);
        assert!(media_type.is_image(), "{named}");
        let image = ImageMediaType::parse(named).expect(named);
        assert_eq!(image.as_str(), named);
        // And the whole way round: a stored file of that type is an image
        // reference a message can carry.
        let stored = Attachment::new(
            nessa_sdk::domain::common::value_objects::Sha256Digest::from_bytes([3; 32]),
            media_type,
            1024,
        )
        .unwrap();
        assert_eq!(
            stored.as_image().map(|image| image.media_type()),
            Some(image)
        );
    }
}

#[test]
fn the_protocol_schema_states_the_same_numbers_the_sdk_enforces() {
    let schema = schema();

    // One image, and the encodings one may be.
    let image = definition(&schema, "ImageAttachment");
    assert_eq!(
        image["properties"]["size"]["maximum"].as_u64(),
        Some(ImageReference::MAX_BYTES)
    );
    let published: BTreeSet<&str> = image["properties"]["mimeType"]["enum"]
        .as_array()
        .expect("the published encodings are a list")
        .iter()
        .map(|value| value.as_str().expect("an encoding is a string"))
        .collect();
    let enforced: BTreeSet<&str> = ImageMediaType::ALL
        .iter()
        .map(|media_type| media_type.as_str())
        .collect();
    assert_eq!(published, enforced);

    // How many images one message may carry, everywhere a message is spelled.
    for named in [
        "ConversationMessage",
        "ConversationPending",
        "ConversationSendParams",
    ] {
        assert_eq!(
            definition(&schema, named)["properties"]["attachments"]["maxItems"].as_u64(),
            Some(UserMessage::MAX_IMAGES as u64),
            "{named}"
        );
    }

    // And how large a file this gateway will take at all, which is wider than
    // what a message may name: a camera's raw file is uploaded and normalized.
    assert_eq!(
        definition(&schema, "AttachmentBeginParams")["properties"]["size"]["maximum"].as_u64(),
        Some(Attachment::MAX_BYTES)
    );
    const { assert!(Attachment::MAX_BYTES > ImageReference::MAX_BYTES) }
}
