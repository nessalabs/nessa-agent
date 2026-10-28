//! Same-entry original facts through real registry and permanent-key TLS fixtures.
use super::*;
use crate::application::credential_registry::{CredentialRegistryFault, RegistryInvariant};
fn assert_original(f: &SignedFixture, record: &PairingRecord) {
    let receipt = f
        .store
        .find_owner_phone_original(f.request.source(), f.request.id())
        .unwrap();
    assert_eq!(receipt.original(), &f.request);
    assert_eq!(receipt.digest(), f.proof.digest());
    assert_eq!(receipt.record(), record);
    assert_eq!(receipt.record().intent(), f.initial.intent());
}
#[test]
fn original_projection_retains_one_original_through_every_published_stage() {
    let f = signed_fixture();
    let approved = ready(f.store.authorize_owner_phone(
        &f.initial,
        &f.proof,
        &signed_admission(&f, f.initial.intent()),
        &Time,
    ))
    .unwrap();
    assert_original(&f, &approved);
    let credential = CredentialId::new("owner-phone-device").unwrap();
    let request = AttemptId::new([77; 16]);
    let staged = f
        .store
        .stage_pairing(
            approved.id(),
            credential.clone(),
            request,
            &signed_admission(&f, approved.intent()),
            &Time,
        )
        .unwrap();
    assert_original(&f, &staged);
    let outcome = ReceiverOutcome::new(
        credential,
        request,
        ResourceId::new("actual-receiver").unwrap(),
        7,
        approved.intent().generation(),
    )
    .unwrap();
    let remembered = f
        .store
        .remember_receiver(approved.id(), &outcome, &Time)
        .unwrap();
    assert_original(&f, &remembered);
    let active = f
        .store
        .publish_pairing(
            approved.id(),
            &signed_admission(&f, approved.intent()),
            &Time,
        )
        .unwrap();
    assert_original(&f, &active);
    assert_eq!(
        active.receiver_binding(),
        Some((outcome.receiver(), outcome.epoch()))
    );
}
#[test]
fn original_projection_cold_reopen_preserves_historical_endpoint_and_pin() {
    let f = signed_fixture();
    let active = active_signed(&f);
    let path = f.directory.path().join("native/credentials.v1.json");
    let original = f.request.clone();
    let digest = *f.proof.digest();
    // A later discovery is a different factual input and must never rewrite history.
    let changed = OwnerPhoneRequest::new(
        original.id(),
        original.source().clone(),
        identity_pin(),
        "127.0.0.1:29744".parse().unwrap(),
        original.device_key(),
    )
    .unwrap();
    assert_ne!(changed.gateway_pin(), original.gateway_pin());
    assert_ne!(changed.endpoint(), original.endpoint());
    drop(f.store);
    let cold = open_store(path).unwrap();
    let receipt = cold
        .find_owner_phone_original(original.source(), original.id())
        .unwrap();
    assert_eq!(receipt.original(), &original);
    assert_eq!(receipt.digest(), &digest);
    assert_eq!(receipt.record(), &active);
}
fn identity_pin() -> [u8; 44] {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32])))
        .unwrap()
        .public_spki()
}
#[test]
fn original_projection_stale_admission_cannot_disclose_a_newer_record() {
    let f = signed_fixture();
    let admission = signed_admission(&f, f.initial.intent());
    ready(
        f.store
            .authorize_owner_phone(&f.initial, &f.proof, &admission, &Time),
    )
    .unwrap();
    assert!(matches!(
        f.store
            .read_owner_phone_original(&f.request, &admission, &Time),
        Err(PairingStoreError::StaleRevision)
    ));
    let fresh = signed_admission(&f, f.initial.intent());
    let receipt = f
        .store
        .read_owner_phone_original(&f.request, &fresh, &Time)
        .unwrap();
    assert_eq!(receipt.original(), &f.request);
    assert_eq!(receipt.digest(), f.proof.digest());
}
#[test]
fn original_projection_tls_recovery_after_bootstrap_retirement_has_no_write_effect() {
    let f = signed_fixture();
    let active = active_signed(&f);
    f.store
        .revoke_sync(RevokeCredentialRequest {
            request_id: "retire-original-projection-bootstrap".into(),
            issuer_principal_id: "owner".into(),
            credential_id: f.session.context().credential_id().as_str().into(),
            revoked_at: 110,
        })
        .unwrap();
    let path = f.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    let channel = tls_channel(42);
    let receipt = f
        .store
        .recover_owner_phone_original(f.request.id(), f.proof.digest(), channel.device_proof())
        .unwrap();
    assert_eq!(receipt.original(), &f.request);
    assert_eq!(receipt.record(), &active);
    assert_eq!(receipt.digest(), f.proof.digest());
    let wrong = tls_channel(43);
    assert!(matches!(
        f.store.recover_owner_phone_original(
            f.request.id(),
            f.proof.digest(),
            wrong.device_proof()
        ),
        Err(PairingStoreError::NotFound)
    ));
    assert!(matches!(
        f.store.recover_owner_phone_original(
            OwnerPhoneRequestId::new([17; 16]).unwrap(),
            f.proof.digest(),
            channel.device_proof()
        ),
        Err(PairingStoreError::NotFound)
    ));
    let mut digest = *f.proof.digest();
    digest[0] ^= 1;
    assert!(matches!(
        f.store
            .recover_owner_phone_original(f.request.id(), &digest, channel.device_proof()),
        Err(PairingStoreError::NotFound)
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
    assert_eq!(f.store.read_pairing(active.id()).unwrap(), active);
}
#[test]
fn original_projection_corrupt_stored_endpoint_fails_cold_restoration() {
    let f = signed_fixture();
    active_signed(&f);
    let path = f.directory.path().join("native/credentials.v1.json");
    drop(f.store);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let original = value["pairings"][0]
        .as_object_mut()
        .unwrap()
        .get_mut("ownerPhoneOriginal")
        .expect("stored canonical original exists");
    // Mutation must alter an EXISTING field, not add an ignored alias.
    assert!(original.get("endpoint").is_some());
    original["endpoint"] = serde_json::json!("127.0.0.1:29744");
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(matches!(
        open_store(path),
        Err(LocalStoreError::InvalidRegistry {
            fault: CredentialRegistryFault::InvalidState(RegistryInvariant::CredentialBinding),
            ..
        })
    ));
}

#[test]
fn original_projection_rejects_other_valid_owner_bootstrap_credential() {
    let f = signed_fixture();
    let approved = ready(f.store.authorize_owner_phone(
        &f.initial,
        &f.proof,
        &signed_admission(&f, f.initial.intent()),
        &Time,
    ))
    .unwrap();
    let mut request = ordinary_issue("another-owner-credential");
    request.principal.id = "owner".into();
    request.principal.kind = PrincipalKindDto::Human;
    request.membership.id = "owner-membership".into();
    request.membership.principal_id = "owner".into();
    request.membership.role = MembershipRoleDto::Admin;
    request.grants = [
        "credential.manage",
        "conversation.read",
        "conversation.write",
        "server.read",
    ]
    .iter()
    .map(|a| grant("org-1", a))
    .collect();
    let outcome = f
        .store
        .issue_internal(request, IssuePath::SurfaceProvision)
        .unwrap();
    let evidence = match outcome {
        IssueCredentialOutcome::Issued { evidence, .. } => evidence,
        _ => panic!("expected fresh owner credential"),
    };
    let other = ready(
        AuthenticateSession {
            policy: &crate::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
            verifier: f.store.as_ref(),
            access: f.store.as_ref(),
            clock: &Time,
        }
        .execute(&evidence, &AudienceId::new("gateway-1").unwrap()),
    )
    .unwrap();
    let admission = ready(
        AuthorizePairing {
            access: f.store.as_ref(),
            policy: &CedarPolicyEvaluator::new().unwrap(),
            clock: &Time,
        }
        .execute(&other, approved.intent(), approved.intent().resource()),
    )
    .unwrap();
    assert!(matches!(
        f.store
            .read_owner_phone_original(&f.request, &admission, &Time),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    ));
    assert_eq!(
        f.store
            .find_owner_phone_original(f.request.source(), f.request.id())
            .unwrap()
            .record(),
        &approved
    );
}
