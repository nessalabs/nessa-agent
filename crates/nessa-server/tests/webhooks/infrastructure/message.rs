use super::*;
use crate::webhooks::application::{InvalidMessage, MacKey, MAX_BODY_BYTES};
use crate::webhooks::domain::{Delivery, Observation};
use nessa_protocol::product::generated::RecordWireRecord;
use uuid::Uuid;

#[test]
fn wire_bytes_and_mac_match_an_independent_golden_vector() {
    let key = MacKey::new(std::array::from_fn(|i| i as u8));
    let message = record_message(
        Uuid::from_u128(1),
        0,
        "00000000-0000-0000-0000-00000000000a",
        "i",
        1,
        r#"{"text":"hello"}"#,
    )
    .unwrap();
    assert_eq!(
        message.body(),
        br#"{"conversationId":"00000000-0000-0000-0000-00000000000a","incarnation":"i","position":1,"record":{"text":"hello"}}"#
    );
    // Python stdlib hmac/sha256, three length-prefixed fields, key 00..1f.
    assert_eq!(
        signature(&key, &message),
        "v1=3ba781a84e593504463f3df9cba8d39cb6993768321265a4048b710e60b5ff04"
    );
    assert_eq!(message.id(), Uuid::from_u128(1));
    assert_eq!(message.timestamp_ms(), 0);
}

#[test]
fn retry_contract_preserves_id_timestamp_body_and_signature() {
    let key = MacKey::new([42; 32]);
    let first = Delivery::queued(Uuid::from_u128(2), 7)
        .admit(7, 50)
        .unwrap();
    let retry = first
        .observed(Observation::Retryable, 75)
        .unwrap()
        .admit(7, 1075)
        .unwrap();
    let encode = |d: &Delivery| {
        record_message(
            d.id(),
            d.timestamp_ms().unwrap(),
            "00000000-0000-0000-0000-00000000000a",
            "i",
            5,
            r#"{"b":2,"a":"\u0061"}"#,
        )
        .unwrap()
    };
    let a = encode(&first);
    let b = encode(&retry);
    assert_eq!(a.body(), b.body());
    assert!(std::str::from_utf8(a.body())
        .unwrap()
        .ends_with(r#""record":{"b":2,"a":"\u0061"}}"#));
    assert_eq!(a.timestamp_ms(), b.timestamp_ms());
    assert_eq!(signature(&key, &a), signature(&key, &b));
    let changed = record_message(
        a.id(),
        a.timestamp_ms() + 1,
        "00000000-0000-0000-0000-00000000000a",
        "i",
        5,
        "{}",
    )
    .unwrap();
    assert_ne!(signature(&key, &a), signature(&key, &changed));
    let changed = record_message(
        Uuid::from_u128(3),
        a.timestamp_ms(),
        "00000000-0000-0000-0000-00000000000a",
        "i",
        5,
        "{}",
    )
    .unwrap();
    assert_ne!(signature(&key, &a), signature(&key, &changed));
}

#[test]
fn malformed_and_oversized_records_fail_before_a_body_is_retained() {
    for raw in ["", "{", "{}{}", "not-json", " {}", "{} ", "{\"a\": 1}"] {
        assert!(matches!(
            record_message(
                Uuid::nil(),
                1,
                "00000000-0000-0000-0000-00000000000a",
                "i",
                1,
                raw
            ),
            Err(InvalidMessage)
        ));
    }
    let too_large = format!("\"{}\"", "x".repeat(MAX_BODY_BYTES));
    assert!(record_message(
        Uuid::nil(),
        1,
        "00000000-0000-0000-0000-00000000000a",
        "i",
        1,
        &too_large
    )
    .is_err());
    let envelope_overflow = format!("\"{}\"", "x".repeat(MAX_BODY_BYTES - 2));
    assert!(record_message(
        Uuid::nil(),
        1,
        "00000000-0000-0000-0000-00000000000a",
        "i",
        1,
        &envelope_overflow
    )
    .is_err());
}

#[test]
fn compact_encoding_preserves_quoted_whitespace_and_rejects_invalid_identity() {
    let chat = "00000000-0000-0000-0000-00000000000a";
    let raw = r#"{"text":"a b\t\" c\\ d","next":1}"#;
    let message = record_message(Uuid::nil(), 0, chat, "i", 0, raw).unwrap();
    assert!(std::str::from_utf8(message.body())
        .unwrap()
        .ends_with(&format!("{raw}}}")));
    for (conversation, incarnation) in [("chat", "i"), (chat, ""), (chat, " ")] {
        assert!(record_message(Uuid::nil(), 0, conversation, incarnation, 0, "{}").is_err());
    }
}

#[test]
fn record_envelope_preserves_the_records_page_owner_representation() {
    // Use the actual owner DTO, including decimal position and base64 payload.
    // The other golden's {text:...} is only a framing fixture, not a wire schema.
    let record = RecordWireRecord {
        position: "1".into(),
        id: "test-record".into(),
        payload: "dGVzdC1vbmx5".into(),
    };
    let raw = serde_json::to_string(&record).unwrap();
    let message = record_message(
        Uuid::nil(),
        0,
        "00000000-0000-0000-0000-00000000000a",
        "i",
        1,
        &raw,
    )
    .unwrap();
    assert!(std::str::from_utf8(message.body())
        .unwrap()
        .ends_with(r#""record":{"position":"1","id":"test-record","payload":"dGVzdC1vbmx5"}}"#));
}
