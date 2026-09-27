//! Facts this context spells in one place and other places spell again: the
//! media type an image library names and the one a message may name, and the
//! numbers the published protocol schema states. Each is valid on its own;
//! these check that together they describe the same gateway.
use crate::attachments::domain::{Attachment, MediaType};
use nessa_images::Encoding;
use nessa_sdk::domain::{
    agent_execution::prompts::{ImageReference, LinkedFile, UserMessage},
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

    // A linked file has no size at all — nothing is uploaded for it — so the
    // only numbers to agree on are how long a path may be and how many of them
    // one message may name.
    //
    // The length is in bytes, and `x-utf8MaxBytes` is the keyword that says so.
    // `maxLength` counts code points, so it is a coarse upper bound and not the
    // rule: equating the two is how a path of 4,095 CJK characters — 12,286
    // bytes — passed every check here and was refused on arrival.
    let path = &definition(&schema, "LinkedFile")["properties"]["path"];
    assert_eq!(
        path["x-utf8MaxBytes"].as_u64(),
        Some(LinkedFile::MAX_PATH_BYTES as u64)
    );
    // Never tighter than the byte rule, or a path the gateway accepts would be
    // refused by a validator that reads only the standard keyword.
    assert!(
        path["maxLength"]
            .as_u64()
            .expect("a coarse bound is published")
            >= LinkedFile::MAX_PATH_BYTES as u64
    );
    // And the domain really does count bytes, which is the half a corpus of
    // ASCII paths can never show: these two have the same number of characters
    // and only one of them fits.
    let ascii = format!("/{}", "a".repeat(LinkedFile::MAX_PATH_BYTES - 1));
    let multibyte = format!("/{}", "あ".repeat(LinkedFile::MAX_PATH_BYTES - 1));
    assert_eq!(ascii.chars().count(), multibyte.chars().count());
    assert!(LinkedFile::new(ascii).is_ok());
    assert_eq!(
        LinkedFile::new(multibyte),
        Err(
            nessa_sdk::domain::agent_execution::ExecutionError::ValueTooLong {
                field: "linked file path",
                max_bytes: LinkedFile::MAX_PATH_BYTES,
            }
        )
    );
    for named in [
        "ConversationMessage",
        "ConversationPending",
        "ConversationSendParams",
    ] {
        assert_eq!(
            definition(&schema, named)["properties"]["files"]["maxItems"].as_u64(),
            Some(UserMessage::MAX_FILES as u64),
            "{named}"
        );
    }
}

/// The schema's path pattern is a coarse gate in front of the domain's rule,
/// so what it accepts must be a superset of what the domain accepts, and every
/// character rule it states must be one the domain states too. Anything the
/// schema let through that the domain refuses is answered `invalid_request`;
/// anything the schema refuses that the domain would have taken is a file a
/// person could never attach, which is the failure worth catching here.
#[test]
fn the_published_path_rule_and_the_domain_rule_describe_the_same_paths() {
    let schema = schema();
    let pattern = definition(&schema, "LinkedFile")["properties"]["path"]["pattern"]
        .as_str()
        .expect("the published path rule is a string");
    // Written for the JSON Schema dialect's regular expressions, which is why
    // it is checked by hand rather than compiled here: each class it names is
    // one the domain names, in the same direction.
    assert!(pattern.starts_with("^(?:/"), "{pattern}");
    for refused in [
        // Control characters, both ranges. C1 was the gap: the published rule
        // named only C0 and DEL while the domain uses `char::is_control`, so a
        // path holding U+0085 passed every client and was refused on arrival.
        "\\u0000-\\u001f",
        "\\u007f",
        "\\u0080-\\u009f",
        // A separator inside a component, which is what makes every component
        // nonempty and forbids a trailing one.
        "[^/",
        // And a component that is `.` or `..`.
        "(?!\\.{1,2}(?:/|$))",
    ] {
        assert!(pattern.contains(refused), "{pattern} is missing {refused}");
    }

    for path in [
        "/Users/ada/report.pdf",
        "/Users/ada/report (final) 100% \"good\".pdf",
        "/Users/ada/отчёт.pdf",
        "/Users/ada/.zshrc",
        "/Users/ada/...",
        "/Users/ada/2026-09-20 10:30.txt",
        // Brackets and backslashes were refused by both, because the agent is
        // handed the path inside a markdown link. The ACP adapter encodes both
        // halves of that link now, so neither rule has an opinion here.
        "/Users/ada/[draft] notes.pdf",
        "/Users/ada/a]b.pdf",
        "/Users/ada/back\\slash",
        "/a",
    ] {
        assert!(
            LinkedFile::new(path.into()).is_ok(),
            "the schema admits {path:?} but the domain refuses it"
        );
    }

    // And the other direction, which is the one that was wrong: every shape the
    // domain refuses is one the published rule refuses too, so a third-party
    // SDK reading the schema turns the same paths away that the gateway does.
    // The pattern is checked against these in `scripts/check-product-protocol.mjs`,
    // where there is a regular-expression engine for the published dialect;
    // what is asserted here is that the domain and that list agree on them.
    for refused in [
        "/tmp/a\u{85}b.pdf",
        "/tmp/a\u{9f}b.pdf",
        "/tmp/a\u{7f}b.pdf",
        "/tmp/",
        "/",
        "//tmp/a.pdf",
        "/tmp//a.pdf",
        "/tmp/.",
        "/tmp/..",
        "/tmp/../etc/passwd",
        "/tmp/./a.pdf",
        "report.pdf",
    ] {
        assert!(
            LinkedFile::new(refused.into()).is_err(),
            "the published rule refuses {refused:?} and the domain does not"
        );
    }
}
