//! Public generated-object boundary over actual private preparation and restoration.
use super::*;
use crate::{
    test_support::PrivateFixture, NativeCallError, NativeDeviceSession, NativePrivateStateKind,
    NativeSessionFailureKind,
};
use nessa_auth::{
    adapters::pairing::{verify_owner_phone_request, FilePairingState, NativeIdentity},
    application::pairing::{ClientPendingStore, OwnerPhoneOriginalStore, PrivateKeyMaterial},
    domain::{
        pairing::{OwnerPhoneContext, OwnerPhoneSource},
        AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use nessa_local_storage::create_directory_beneath;
use nessa_protocol::product::generated::{PhoneDiscovery, PhoneOriginal};
use serde_json::{json, Value};
use std::{fs, path::Path};
use zeroize::Zeroizing;

fn root() -> PrivateFixture {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    root
}
fn discovery() -> PhoneDiscovery {
    let source = OwnerPhoneSource::new(
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("member").unwrap(),
        AudienceId::new("gateway").unwrap(),
        CredentialId::new("bootstrap-public-id").unwrap(),
        Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
    )
    .unwrap();
    let pin = NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([8; 32])))
        .unwrap()
        .public_spki();
    owner_phone::discovery(&source, "127.0.0.1:9".parse().unwrap(), pin).unwrap()
}
fn text() -> String {
    serde_json::to_string(&discovery()).unwrap()
}
fn open(root: &PrivateFixture) -> NativeDeviceSession {
    NativeDeviceSession::open(
        root.path().to_str().unwrap().into(),
        "private".into(),
        "127.0.0.1:9".into(),
        None,
    )
    .unwrap()
}
fn argument(
    result: Result<Arc<NativeOwnerPhonePreparation>, NativeOwnerPreparationCallError>,
) -> NativeOwnerArgumentFailure {
    match result.err().expect("argument must refuse") {
        NativeOwnerPreparationCallError::Argument { failure } => failure,
        other => panic!("wrong argument failure: {other:?}"),
    }
}
fn preparation(
    result: Result<Arc<NativeOwnerPhonePreparation>, NativeOwnerPreparationCallError>,
) -> Arc<NativeOwnerPreparationFailure> {
    match result.err().expect("preparation must refuse") {
        NativeOwnerPreparationCallError::Preparation { failure } => failure,
        other => panic!("wrong preparation failure: {other:?}"),
    }
}

#[test]
fn public_original_proof_cold_restore_and_raw_parent_drop_preserve_exact_facts_without_issuance() {
    let root = root();
    let parent = open(&root);
    let proof = parent.prepare_owner_original(vec![1; 16], text()).unwrap();
    let public_json = proof.original_json().unwrap();
    let original: PhoneOriginal = serde_json::from_str(&public_json).unwrap();
    let original = owner_phone::request(original).unwrap();
    assert_eq!(proof.request_id(), vec![1; 16]);
    assert_eq!(proof.device_key(), original.device_key().bytes().to_vec());
    let signature: [u8; 64] = proof.signature().try_into().unwrap();
    assert_eq!(
        verify_owner_phone_request(original.clone(), &signature)
            .unwrap()
            .digest()
            .as_slice(),
        proof.digest()
    );
    let replay = parent.prepare_owner_original(vec![1; 16], text()).unwrap();
    assert_eq!(replay.signature(), proof.signature());
    assert_eq!(replay.digest(), proof.digest());
    assert!(parent.local_enrollment().unwrap().is_none());
    assert!(matches!(
        parent.bind_saved_receiver(),
        Err(NativeCallError::Session { failure }) if failure.kind() == NativeSessionFailureKind::NoCredential
    ));
    let no_credential = parent.open_product("instance".into()).err().unwrap();
    assert!(
        matches!(no_credential, NativeCallError::Session { failure } if failure.kind() == NativeSessionFailureKind::NoCredential)
    );
    drop(parent);
    // The escaped proof owns no parent: raw last drop released the actual FS lease.
    let private = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    assert!(private.load_credential().unwrap().is_none());
    assert!(private.load_pending().unwrap().is_none());
    assert!(private.load_original().unwrap().unwrap().key().is_some());
    drop(private);
    assert_eq!(proof.original_json().unwrap(), public_json);
    let restored_parent = open(&root);
    let restored = restored_parent
        .restore_owner_original(vec![1; 16], text())
        .unwrap();
    assert_eq!(restored.original_json().unwrap(), public_json);
    assert_eq!(restored.signature(), proof.signature());
    assert_eq!(restored.digest(), proof.digest());
    let close = restored_parent.close_observed();
    assert!(close.released() && close.failure().is_none());
    assert!(restored_parent.close_observed().released());
    drop(restored_parent);
    assert!(close.released());
}
#[test]
fn public_restore_missing_and_reserved_do_not_fall_back_to_initial_preparation() {
    let root = root();
    let parent = open(&root);
    let missing = preparation(parent.restore_owner_original(vec![1; 16], text()));
    assert_eq!(missing.kind(), NativeOwnerPreparationFailureKind::Missing);
    assert!(!root.path().join("private/owner-phone-original").exists());
    parent.close().unwrap();
    let store = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    let context: OwnerPhoneContext = owner_phone::context(discovery()).unwrap();
    let reservation =
        OwnerPhoneReservation::new(OwnerPhoneRequestId::new([1; 16]).unwrap(), context);
    drop(store.reserve_original(reservation).unwrap());
    let before = fs::read(root.path().join("private/owner-phone-original")).unwrap();
    drop(store);
    let parent = open(&root);
    for initial in [false, true] {
        let result = if initial {
            parent.prepare_owner_original(vec![1; 16], text())
        } else {
            parent.restore_owner_original(vec![1; 16], text())
        };
        assert_eq!(
            preparation(result).kind(),
            NativeOwnerPreparationFailureKind::Incomplete
        );
        assert_eq!(
            fs::read(root.path().join("private/owner-phone-original")).unwrap(),
            before
        );
    }
}
#[test]
fn foreign_unique_bounded_discovery_rejects_contradictions_before_private_effects() {
    let root = root();
    let parent = open(&root);
    let encoded = text();
    let mut cases = vec![
        (
            vec![0; 16],
            encoded.clone(),
            NativeOwnerArgumentFailure::RequestId,
        ),
        (
            vec![1; 15],
            encoded.clone(),
            NativeOwnerArgumentFailure::RequestId,
        ),
    ];
    cases.push((
        vec![1; 16],
        encoded.replace(
            "\"ownerId\":\"owner\"",
            "\"ownerId\":\"owner\",\"ownerId\":\"other\"",
        ),
        NativeOwnerArgumentFailure::Json,
    ));
    for (field, value, expected) in [
        (
            "nativeEndpoint",
            json!("127.0.0.1:0"),
            NativeOwnerArgumentFailure::Endpoint,
        ),
        ("gatewayKey", json!([0]), NativeOwnerArgumentFailure::Length),
        ("unknown", json!(true), NativeOwnerArgumentFailure::Json),
    ] {
        let mut changed: Value = serde_json::from_str(&encoded).unwrap();
        changed[field] = value;
        cases.push((vec![1; 16], changed.to_string(), expected));
    }
    let mut changed: Value = serde_json::from_str(&encoded).unwrap();
    changed["gatewayKey"][0] = json!(256);
    cases.push((
        vec![1; 16],
        changed.to_string(),
        NativeOwnerArgumentFailure::Octet,
    ));
    let mut changed: Value = serde_json::from_str(&encoded).unwrap();
    changed["source"]["resourceId"] = json!("other-gateway");
    cases.push((
        vec![1; 16],
        changed.to_string(),
        NativeOwnerArgumentFailure::Source,
    ));
    let mut changed: Value = serde_json::from_str(&encoded).unwrap();
    changed["consent"]["grants"].as_array_mut().unwrap().pop();
    cases.push((
        vec![1; 16],
        changed.to_string(),
        NativeOwnerArgumentFailure::Consent,
    ));
    let exact = format!(
        "{}{}",
        encoded,
        " ".repeat(MAX_PROTECTED_RESPONSE_BYTES - encoded.len())
    );
    assert_eq!(
        preparation(parent.restore_owner_original(vec![1; 16], exact.clone())).kind(),
        NativeOwnerPreparationFailureKind::Missing
    );
    cases.push((
        vec![1; 16],
        exact + " ",
        NativeOwnerArgumentFailure::TooLarge,
    ));
    for (id, input, expected) in cases {
        assert_eq!(argument(parent.prepare_owner_original(id, input)), expected);
        assert!(!root.path().join("private/owner-phone-original").exists());
    }
}
#[test]
fn foreign_changed_original_and_independent_seed_public_key_corruption_refuse_without_repair() {
    let root = root();
    let parent = open(&root);
    let proof = parent.prepare_owner_original(vec![1; 16], text()).unwrap();
    let original_path = root.path().join("private/owner-phone-original");
    let before = fs::read(&original_path).unwrap();
    let mismatch = preparation(parent.restore_owner_original(vec![2; 16], text()));
    assert_eq!(
        mismatch.private_state().unwrap().kind(),
        NativePrivateStateKind::Conflict
    );
    assert_eq!(fs::read(&original_path).unwrap(), before);
    parent.close().unwrap();
    for offset in [1, 33] {
        let mut corrupt = before.clone();
        let position = corrupt.len() - offset;
        corrupt[position] ^= 1;
        fs::write(&original_path, &corrupt).unwrap();
        let parent = open(&root);
        let failure = preparation(parent.restore_owner_original(vec![1; 16], text()));
        assert_eq!(
            failure.private_state().unwrap().kind(),
            NativePrivateStateKind::Corrupt
        );
        assert_eq!(fs::read(&original_path).unwrap(), corrupt);
        assert_eq!(proof.request_id(), vec![1; 16]);
        parent.close().unwrap();
    }
    fs::write(&original_path, before).unwrap();
    let parent = open(&root);
    assert_eq!(
        parent
            .restore_owner_original(vec![1; 16], text())
            .unwrap()
            .digest(),
        proof.digest()
    );
}

#[cfg(unix)]
#[test]
fn foreign_actual_readable_prepared_bytes_do_not_replace_failed_durability_acknowledgement() {
    use std::os::unix::fs::PermissionsExt;
    let root = root();
    let parent = open(&root);
    let proof = parent.prepare_owner_original(vec![1; 16], text()).unwrap();
    let path = root.path().join("private/owner-phone-original");
    let before = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    let refused = preparation(parent.restore_owner_original(vec![1; 16], text()));
    assert_eq!(refused.kind(), NativeOwnerPreparationFailureKind::Private);
    assert_eq!(
        refused.private_state().unwrap().kind(),
        NativePrivateStateKind::Uncertain
    );
    assert!(refused.crypto().is_none());
    assert_eq!(fs::read(&path).unwrap(), before);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        parent
            .restore_owner_original(vec![1; 16], text())
            .unwrap()
            .digest(),
        proof.digest()
    );
}
#[test]
fn foreign_preparation_crypto_and_parent_admission_remain_typed_separate_from_private_state() {
    let root = root();
    let parent = open(&root);
    let mut bad: Value = serde_json::from_str(&text()).unwrap();
    bad["gatewayKey"] = json!(vec![0; 44]);
    let crypto = preparation(parent.prepare_owner_original(vec![1; 16], bad.to_string()));
    assert_eq!(crypto.kind(), NativeOwnerPreparationFailureKind::Crypto);
    assert_eq!(crypto.crypto(), Some(NativeCryptoFailure::InvalidContext));
    assert!(crypto.private_state().is_none());
    assert!(!root.path().join("private/owner-phone-original").exists());
    parent.request_close().unwrap();
    match parent
        .prepare_owner_original(vec![1; 16], text())
        .err()
        .unwrap()
    {
        NativeOwnerPreparationCallError::Session { failure } => {
            assert_eq!(failure.kind(), NativeSessionFailureKind::Closed)
        }
        other => panic!("parent closure became preparation permission: {other:?}"),
    }
}

#[test]
fn foreign_close_observation_preserves_actual_tls_child_failure_after_release_and_parent_drop() {
    use crate::session::product_tests::fixtures;
    use crate::{NativeProductCompletionKind, NativeProductFailureKind};
    use nessa_protocol::protocol::{OutgoingMessage, ResponseFrame};
    let root = fixtures::root(true);
    let (address, peer) = fixtures::peer(|wire| {
        wire.request().unwrap();
        wire.send(OutgoingMessage::Response(
            ResponseFrame::success("wrong-id", &json!({})).unwrap(),
        ));
        assert!(wire.request().is_none());
    });
    let parent = NativeDeviceSession::open(
        root.path().to_str().unwrap().into(),
        "private".into(),
        address.to_string(),
        None,
    )
    .unwrap();
    let child = parent.open_product("phone-instance".into()).unwrap();
    child.wait_ready().unwrap();
    let ticket = child
        .request("conversation.list".into(), "{}".into())
        .unwrap();
    assert_eq!(ticket.wait().kind(), NativeProductCompletionKind::Uncertain);
    let observation = parent.close_observed();
    assert!(observation.released());
    let failure = observation.failure().unwrap();
    assert_eq!(failure.kind(), NativeSessionFailureKind::Product);
    assert_eq!(
        failure.product().unwrap().kind(),
        NativeProductFailureKind::Correlation
    );
    drop(parent);
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
    assert!(observation.released());
    assert_eq!(
        observation.failure().unwrap().product().unwrap().kind(),
        NativeProductFailureKind::Correlation
    );
    peer.join();
}
