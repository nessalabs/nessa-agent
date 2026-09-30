use super::*;
use crate::conversation::{
    application::{CatalogueDescriptor, CatalogueKey},
    domain::{Conversation, LATEST_TIME_MS},
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use serde_json::{json, Value};

const ENTRY: &str = "00000000-0000-0000-0000-000000000001";
const OTHER: &str = "00000000-0000-0000-0000-000000000002";

fn value() -> CatalogueValue {
    let id = ConversationId::new(ENTRY).unwrap();
    let conversation = Conversation::restore(
        id.clone(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("owner").unwrap(),
        "panel".into(),
        "create".into(),
        20,
        Some(AgentId::Codex),
        ConversationModelId::new("model").unwrap(),
        ConversationApprovalMode::Ask,
    )
    .unwrap();
    CatalogueValue {
        descriptor: CatalogueDescriptor {
            key: CatalogueKey { creation: 1, id },
            revision: 2,
            deleted: false,
        },
        conversation,
        summary: Some(
            ConversationSummary::new(
                Some(ConversationTitle::new("A \"quote\" \\ path").unwrap()),
                Some(ConversationPreview::new("snow 雪 and \"quote\"").unwrap()),
                10,
                true,
            )
            .unwrap(),
        ),
    }
}

fn raw() -> Value {
    serde_json::from_slice(&encode(&value()).unwrap()).unwrap()
}
fn decoded(raw: &Value) -> Result<CatalogueMetadata, CataloguePayloadError> {
    decode(ENTRY, &serde_json::to_vec(raw).unwrap())
}

#[test]
fn catalogue_metadata_codec_preserves_current_field_representation() {
    let value = value();
    let conversation = &value.conversation;
    let summary = value.summary.as_ref().unwrap();
    let expected = json!({
        "id": ENTRY, "createdAtMs": 20, "agent": "codex", "model": "model", "approvalMode": "ask",
        "summary": { "title": summary.title().unwrap().as_str(), "preview": summary.preview().unwrap().as_str(), "updatedAtMs": 10, "archived": true }
    });
    let bytes = encode(&value).unwrap();
    assert_eq!(bytes, serde_json::to_vec(&expected).unwrap());
    let metadata = decode(ENTRY, &bytes).unwrap();
    assert_eq!(metadata.id(), conversation.id());
    assert_eq!(
        metadata.created_at_ms(),
        conversation.creation_requested_at_ms()
    );
    assert_eq!(metadata.agent(), conversation.agent());
    assert_eq!(metadata.model(), conversation.model());
    assert_eq!(metadata.approval_mode(), conversation.approval_mode());
    assert_eq!(metadata.summary(), value.summary.as_ref());
}

#[test]
fn catalogue_metadata_codec_correlates_identity_and_uses_product_owners() {
    assert_eq!(
        decode(OTHER, &encode(&value()).unwrap()),
        Err(CataloguePayloadError::WrongEntry)
    );
    let mut value = value();
    value.descriptor.key.id = ConversationId::new(OTHER).unwrap();
    assert_eq!(encode(&value), Err(CataloguePayloadError::WrongEntry));
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
    let bytes = encode(&value()).unwrap();
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

#[test]
fn catalogue_metadata_codec_bounds_input_before_parser() {
    let bytes = encode(&value()).unwrap();
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
