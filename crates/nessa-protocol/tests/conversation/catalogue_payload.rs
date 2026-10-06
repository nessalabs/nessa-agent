use super::*;
use crate::conversation::domain::LATEST_TIME_MS;
use serde_json::{json, Value};

const ENTRY: &str = "00000000-0000-0000-0000-000000000001";
const OTHER: &str = "00000000-0000-0000-0000-000000000002";

fn metadata() -> CatalogueMetadata {
    CatalogueMetadata::new(
        ConversationId::new(ENTRY).unwrap(),
        20,
        Some(AgentId::Codex),
        ConversationModelId::new("model").unwrap(),
        ConversationApprovalMode::Ask,
        Some(
            ConversationSummary::new(
                Some(ConversationTitle::new("A \"quote\" \\ path").unwrap()),
                Some(ConversationPreview::new("snow 雪 and \"quote\"").unwrap()),
                10,
                true,
            )
            .unwrap(),
        ),
    )
    .unwrap()
}

fn raw() -> Value {
    serde_json::from_slice(&encode(ENTRY, &metadata()).unwrap()).unwrap()
}
fn decoded(raw: &Value) -> Result<CatalogueMetadata, CataloguePayloadError> {
    decode(ENTRY, &serde_json::to_vec(raw).unwrap())
}

#[test]
fn catalogue_metadata_codec_preserves_current_field_representation() {
    let value = metadata();
    let summary = value.summary().unwrap();
    let expected = json!({
        "id": ENTRY, "createdAtMs": 20, "agent": "codex", "model": "model", "approvalMode": "ask",
        "summary": { "title": summary.title().unwrap().as_str(), "preview": summary.preview().unwrap().as_str(), "updatedAtMs": 10, "archived": true }
    });
    let bytes = encode(ENTRY, &value).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&expected).unwrap());
    assert_eq!(decode(ENTRY, &bytes).unwrap(), value);
}

#[test]
fn catalogue_metadata_codec_correlates_identity_and_uses_product_owners() {
    assert_eq!(
        decode(OTHER, &encode(ENTRY, &metadata()).unwrap()),
        Err(CataloguePayloadError::WrongEntry)
    );
    assert_eq!(
        encode(OTHER, &metadata()),
        Err(CataloguePayloadError::WrongEntry)
    );
    let mut cases = Vec::new();
    for (field, invalid) in [
        ("id", json!("not-a-conversation")),
        ("agent", json!("unsupported")),
        ("model", json!("model\n")),
        ("approvalMode", json!("unknown")),
        ("createdAtMs", json!(LATEST_TIME_MS + 1)),
    ] {
        let mut value = raw();
        value[field] = invalid;
        if field == "id" {
            assert_eq!(
                decode("not-a-conversation", &serde_json::to_vec(&value).unwrap()),
                Err(CataloguePayloadError::Metadata)
            );
        } else {
            cases.push(value);
        }
    }
    for (field, invalid) in [
        ("title", json!("two  spaces")),
        (
            "preview",
            json!("x".repeat(ConversationPreview::MAX_BYTES + 1)),
        ),
        ("updatedAtMs", json!(LATEST_TIME_MS + 1)),
    ] {
        let mut value = raw();
        value["summary"][field] = invalid;
        cases.push(value);
    }
    for value in cases {
        assert_eq!(decoded(&value), Err(CataloguePayloadError::Metadata));
    }
    let mut boundary = raw();
    boundary["createdAtMs"] = json!(LATEST_TIME_MS);
    boundary["summary"]["updatedAtMs"] = json!(LATEST_TIME_MS);
    assert!(decoded(&boundary).is_ok());
}

#[test]
fn catalogue_metadata_codec_requires_one_complete_current_shape() {
    for field in [
        "id",
        "createdAtMs",
        "agent",
        "model",
        "approvalMode",
        "summary",
    ] {
        let mut value = raw();
        value.as_object_mut().unwrap().remove(field);
        assert_eq!(decoded(&value), Err(CataloguePayloadError::Malformed));
    }
    for field in ["title", "preview", "updatedAtMs", "archived"] {
        let mut value = raw();
        value["summary"].as_object_mut().unwrap().remove(field);
        assert_eq!(decoded(&value), Err(CataloguePayloadError::Malformed));
    }
    let mut extra = raw();
    extra["unrecognized"] = json!(true);
    assert_eq!(decoded(&extra), Err(CataloguePayloadError::Malformed));
    let mut extra = raw();
    extra["summary"]["unrecognized"] = json!(true);
    assert_eq!(decoded(&extra), Err(CataloguePayloadError::Malformed));
    let bytes = encode(ENTRY, &metadata()).unwrap();
    let duplicate = format!(
        "{{\"id\":\"{ENTRY}\",{}",
        std::str::from_utf8(&bytes).unwrap().trim_start_matches('{')
    );
    assert_eq!(
        decode(ENTRY, duplicate.as_bytes()),
        Err(CataloguePayloadError::Malformed)
    );
    let mut nullable = raw();
    nullable["agent"] = Value::Null;
    nullable["summary"] = Value::Null;
    let decoded = decoded(&nullable).unwrap();
    assert_eq!(decoded.agent(), None);
    assert_eq!(decoded.summary(), None);
}

fn object_keys(value: &Value) -> Vec<String> {
    value.as_object().unwrap().keys().cloned().collect()
}

fn published_keys(value: &Value) -> Vec<String> {
    value
        .as_array()
        .unwrap()
        .iter()
        .map(|key| key.as_str().unwrap().to_owned())
        .collect()
}

#[test]
fn published_catalogue_payload_keys_are_the_encoded_object_keys() {
    let published: Value = serde_json::from_str(include_str!(
        "../../src/conversation/catalogue-payload-keys.json"
    ))
    .unwrap();
    let value = raw();
    assert_eq!(object_keys(&value), published_keys(&published["metadata"]));
    assert_eq!(
        object_keys(&value["summary"]),
        published_keys(&published["summary"])
    );
    let mut absent = value;
    absent["summary"] = Value::Null;
    assert_eq!(object_keys(&absent), published_keys(&published["metadata"]));
}

#[test]
fn catalogue_metadata_codec_bounds_input_before_parser() {
    let bytes = encode(ENTRY, &metadata()).unwrap();
    let mut boundary = bytes;
    boundary.resize(MAX_CATALOGUE_PAYLOAD_BYTES, b' ');
    assert!(decode(ENTRY, &boundary).is_ok());
    boundary.push(b' ');
    assert_eq!(
        decode(ENTRY, &boundary),
        Err(CataloguePayloadError::Oversized)
    );
    assert_eq!(
        decode(ENTRY, &vec![b'!'; MAX_CATALOGUE_PAYLOAD_BYTES + 1]),
        Err(CataloguePayloadError::Oversized)
    );
}
