//! Public wire decoding retains complete consent; it never proves key possession.
use nessa_protocol::pairing::wire::{decode_reply, NativePairingReply};
use serde_json::{json, Value};
fn frame(class: &str, action: &str) -> Value {
    json!({"kind":"status","status":{"stage":"approved","consent":{"public":{"invitation":vec![1;16],"attempt":vec![2;16],"consent":vec![3;16],"generation":1,"expiryMs":600000,"class":class},"audience":"gateway","organization":"org","resource":"gateway","action":action}}})
}
fn consent(value: &Value) -> nessa_auth::domain::pairing::DisclosedConsent {
    match decode_reply(&serde_json::to_vec(value).unwrap()).unwrap() {
        NativePairingReply::Status(status) => status.consent().unwrap().clone(),
        _ => panic!("expected protected consent"),
    }
}
#[test]
fn owner_phone_wire_requires_all_canonical_actions_in_order() {
    let mut valid = frame("gateway-owner-conversation-read-write", "conversation.read");
    valid["status"]["consent"]["additionalActions"] = json!(["conversation.write", "server.read"]);
    let decoded = consent(&valid);
    assert_eq!(
        decoded
            .grants()
            .map(|g| g.action().as_str())
            .collect::<Vec<_>>(),
        ["conversation.read", "conversation.write", "server.read"]
    );
    for changed in [
        json!([]),
        json!(["conversation.write"]),
        json!(["server.read", "conversation.write"]),
        json!(["conversation.write", "server.read", "credential.manage"]),
    ] {
        let mut bad = valid.clone();
        bad["status"]["consent"]["additionalActions"] = changed;
        assert!(decode_reply(&serde_json::to_vec(&bad).unwrap()).is_err());
    }
    let mut omitted = valid.clone();
    omitted["status"]["consent"]
        .as_object_mut()
        .unwrap()
        .remove("additionalActions");
    assert!(decode_reply(&serde_json::to_vec(&omitted).unwrap()).is_err());
    let mut forged = valid.clone();
    forged["status"]["consent"]["additionalActions"] =
        json!([{"action":"conversation.write","resource":"foreign"},"server.read"]);
    assert!(decode_reply(&serde_json::to_vec(&forged).unwrap()).is_err());
}
#[test]
fn legacy_wire_stays_single_action_and_never_accepts_widening() {
    for (class, action) in [
        ("gateway-conversation-read", "conversation.read"),
        ("peer-gateway-conversation-read", "conversation.read"),
        ("peer-gateway-environment-serve", "environment.serve"),
    ] {
        let valid = frame(class, action);
        let decoded = consent(&valid);
        assert_eq!(decoded.grants().count(), 1);
        assert!(decoded.additional_grants().is_empty());
        let bytes = serde_json::to_vec(&valid).unwrap();
        assert!(!String::from_utf8(bytes)
            .unwrap()
            .contains("additionalActions"));
        let mut widened = valid;
        widened["status"]["consent"]["additionalActions"] = json!(["conversation.write"]);
        assert!(decode_reply(&serde_json::to_vec(&widened).unwrap()).is_err());
    }
}
#[test]
fn foreign_public_scope_never_correlates_as_the_original_full_consent() {
    use nessa_auth::domain::{
        pairing::{ConsentClass, ConsentIntent, ConsentIntentId},
        AudienceId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    };
    let expected = ConsentIntent::new(
        ConsentIntentId::new([3; 16]),
        1,
        AudienceId::new("gateway").unwrap(),
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("member").unwrap(),
        Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let mut valid = frame("gateway-owner-conversation-read-write", "conversation.read");
    valid["status"]["consent"]["additionalActions"] = json!(["conversation.write", "server.read"]);
    assert!(consent(&valid).verify_intent(&expected).is_ok());
    for field in ["resource", "organization", "audience"] {
        let mut foreign = valid.clone();
        foreign["status"]["consent"][field] = json!("foreign");
        assert!(
            consent(&foreign).verify_intent(&expected).is_err(),
            "foreign {field}"
        );
    }
}
#[test]
fn legacy_protected_encoder_bytes_are_unchanged_for_all_three_classes() {
    use nessa_auth::domain::{
        pairing::{
            AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, DeviceKey, InvitationId,
            PairingEvent, PairingInitiator, PairingPolicy, PairingRecord,
        },
        AudienceId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    };
    use nessa_protocol::pairing::{wire::encode_status, DevicePairingStatus};
    for class in [
        ConsentClass::DeviceRead,
        ConsentClass::PeerRead,
        ConsentClass::PeerEnvironment,
    ] {
        let intent = ConsentIntent::new(
            ConsentIntentId::new([3; 16]),
            1,
            AudienceId::new("gateway").unwrap(),
            PrincipalId::new("owner").unwrap(),
            MembershipId::new("member").unwrap(),
            Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
            class,
        )
        .unwrap();
        let record = PairingRecord::new(
            InvitationId::new([1; 16]),
            intent,
            0,
            PairingPolicy::initial(),
        )
        .unwrap();
        let key = DeviceKey::new([4; 32]);
        let attempt = AttemptId::new([2; 16]);
        let reserved = record
            .transition(
                PairingEvent::reserve(attempt, key, [5; 32]),
                PairingInitiator::Device(key),
                1,
            )
            .unwrap()
            .after()
            .clone();
        let claimed = reserved
            .transition(
                PairingEvent::claim(attempt, key),
                PairingInitiator::Device(key),
                2,
            )
            .unwrap()
            .after()
            .clone();
        let public = nessa_auth::domain::pairing::PublicIntent::new(
            InvitationId::new([1; 16]),
            attempt,
            ConsentIntentId::new([3; 16]),
            1,
            claimed.expires_at_ms(),
            class,
        )
        .unwrap();
        let bytes =
            encode_status(public, &DevicePairingStatus::Claimed(Box::new(claimed))).unwrap();
        let a = serde_json::to_string(&vec![1; 16]).unwrap();
        let b = serde_json::to_string(&vec![2; 16]).unwrap();
        let c = serde_json::to_string(&vec![3; 16]).unwrap();
        let expected=format!("{{\"kind\":\"status\",\"status\":{{\"stage\":\"claimed\",\"consent\":{{\"public\":{{\"invitation\":{a},\"attempt\":{b},\"consent\":{c},\"generation\":1,\"expiryMs\":{},\"class\":\"{}\"}},\"audience\":\"gateway\",\"organization\":\"org\",\"resource\":\"gateway\",\"action\":\"{}\"}}}}}}",public.expiry_ms(),class.as_str(),class.grant_action());
        assert_eq!(bytes, expected.into_bytes());
    }
}
