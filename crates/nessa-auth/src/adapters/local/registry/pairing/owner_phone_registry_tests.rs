//! Real registry tests for atomic signed originals; no receiver/server effects.
use super::*;
use crate::{
    adapters::pairing::verify_owner_phone_request,
    application::{pairing::OwnerPhoneStore, ports::IssuePath},
    domain::pairing::{OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource},
};
struct SignedFixture {
    directory: TempDir,
    store: Arc<LocalCredentialStore>,
    session: AuthenticatedSession,
    request: OwnerPhoneRequest,
    proof: crate::application::pairing::VerifiedOwnerPhoneDeviceProof,
    initial: PairingRecord,
    identity: NativeIdentity,
}
fn signed_fixture() -> SignedFixture {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(open_store(directory.path().join("native/credentials.v1.json")).unwrap());
    let mut bootstrap_request = bootstrap();
    for action in ["conversation.read", "conversation.write", "server.read"] {
        bootstrap_request.grants.push(grant("org-1", action));
    }
    let bootstrap = store.bootstrap(bootstrap_request).unwrap();
    let session = ready(
        AuthenticateSession {
            policy: &crate::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
            verifier: store.as_ref(),
            access: store.as_ref(),
            clock: &Time,
        }
        .execute(&bootstrap.evidence, &AudienceId::new("gateway-1").unwrap()),
    )
    .unwrap();
    let context = session.context();
    let gateway = Resource::new(
        OrganizationId::new("org-1").unwrap(),
        ResourceId::new("gateway-1").unwrap(),
    );
    let source = OwnerPhoneSource::new(
        context.principal_id().clone(),
        context.membership_id().clone(),
        context.audience_id().clone(),
        context.credential_id().clone(),
        gateway.clone(),
    )
    .unwrap();
    let identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([42; 32]))).unwrap();
    let request = OwnerPhoneRequest::new(
        OwnerPhoneRequestId::new([7; 16]).unwrap(),
        source,
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32])))
            .unwrap()
            .public_spki(),
        "127.0.0.1:19744".parse().unwrap(),
        identity.device_key(),
    )
    .unwrap();
    let signature = identity.sign_owner_phone_request(&request).unwrap();
    let proof = verify_owner_phone_request(request.clone(), &signature).unwrap();
    let intent = ConsentIntent::new(
        ConsentIntentId::new([8; 16]),
        1,
        context.audience_id().clone(),
        context.principal_id().clone(),
        context.membership_id().clone(),
        gateway,
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let initial = PairingRecord::new_owner_phone(
        InvitationId::new([9; 16]),
        intent,
        Time.unix_milliseconds(),
        PairingPolicy::initial(),
    )
    .unwrap();
    SignedFixture {
        directory,
        store,
        session,
        request,
        proof,
        initial,
        identity,
    }
}
fn signed_admission(f: &SignedFixture, intent: &ConsentIntent) -> PairingAdmission {
    ready(
        AuthorizePairing {
            access: f.store.as_ref(),
            policy: &CedarPolicyEvaluator::new().unwrap(),
            clock: &Time,
        }
        .execute(&f.session, intent, intent.resource()),
    )
    .unwrap()
}
#[test]
fn signed_original_replay_requires_original_intent_and_keeps_one_record() {
    let f = signed_fixture();
    let admission = signed_admission(&f, f.initial.intent());
    let approved = ready(
        f.store
            .authorize_owner_phone(&f.initial, &f.proof, &admission, &Time),
    )
    .unwrap();
    assert_eq!(approved.phase(), PairingPhase::Approved);
    assert_eq!(approved.charged_attempts(), 0);
    let fresh = signed_admission(&f, approved.intent());
    let repeated = ready(
        f.store
            .authorize_owner_phone(&f.initial, &f.proof, &fresh, &Time),
    )
    .unwrap();
    assert_eq!(repeated, approved);
    let wrong_intent = ConsentIntent::new(
        ConsentIntentId::new([88; 16]),
        1,
        f.initial.intent().audience().clone(),
        f.initial.intent().owner().clone(),
        f.initial.intent().membership().clone(),
        f.initial.intent().resource().clone(),
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let wrong = signed_admission(&f, &wrong_intent);
    assert!(ready(
        f.store
            .authorize_owner_phone(&f.initial, &f.proof, &wrong, &Time)
    )
    .is_err());
    assert_eq!(f.store.pending_pairings().unwrap().len(), 1);
    assert_eq!(
        f.store.read_owner_phone(&f.request, &fresh, &Time).unwrap(),
        approved
    );
}
#[test]
fn signed_original_changed_endpoint_conflicts_even_with_valid_new_signature() {
    let f = signed_fixture();
    let admission = signed_admission(&f, f.initial.intent());
    let approved = ready(
        f.store
            .authorize_owner_phone(&f.initial, &f.proof, &admission, &Time),
    )
    .unwrap();
    let changed = OwnerPhoneRequest::new(
        f.request.id(),
        f.request.source().clone(),
        *f.request.gateway_pin(),
        "127.0.0.1:19745".parse().unwrap(),
        f.request.device_key(),
    )
    .unwrap();
    let proof = verify_owner_phone_request(
        changed.clone(),
        &f.identity.sign_owner_phone_request(&changed).unwrap(),
    )
    .unwrap();
    let fresh = signed_admission(&f, approved.intent());
    assert!(ready(
        f.store
            .authorize_owner_phone(&f.initial, &proof, &fresh, &Time)
    )
    .is_err());
    assert_eq!(
        f.store
            .find_owner_phone(f.request.source(), f.request.id())
            .unwrap(),
        approved
    );
}
#[test]
fn signed_original_cold_registry_preserves_approval_and_rejects_removed_history() {
    let f = signed_fixture();
    let approved = ready(f.store.authorize_owner_phone(
        &f.initial,
        &f.proof,
        &signed_admission(&f, f.initial.intent()),
        &Time,
    ))
    .unwrap();
    let path = f.directory.path().join("native/credentials.v1.json");
    let request = f.request.clone();
    drop(f.store);
    let cold = open_store(path.clone()).unwrap();
    assert_eq!(
        cold.find_owner_phone(request.source(), request.id())
            .unwrap(),
        approved
    );
    drop(cold);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    value["pairings"][0]["history"] = serde_json::json!([]);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(open_store(path).is_err());
}
#[test]
fn signed_original_namespace_duplicate_is_rejected_even_with_distinct_record_ids() {
    let f = signed_fixture();
    ready(f.store.authorize_owner_phone(
        &f.initial,
        &f.proof,
        &signed_admission(&f, f.initial.intent()),
        &Time,
    ))
    .unwrap();
    let path = f.directory.path().join("native/credentials.v1.json");
    drop(f.store);
    let mut value: serde_json::Value =
        serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    let mut duplicate = value["pairings"][0].clone();
    duplicate["invitation"] = serde_json::json!(vec![19; 16]);
    duplicate["consent"] = serde_json::json!(vec![18; 16]);
    value["pairings"].as_array_mut().unwrap().push(duplicate);
    std::fs::write(&path, serde_json::to_vec(&value).unwrap()).unwrap();
    assert!(open_store(path).is_err());
}
#[test]
fn genuine_tls_key_recovers_only_original_public_state_without_advancing_it() {
    let f = signed_fixture();
    let approved = ready(f.store.authorize_owner_phone(
        &f.initial,
        &f.proof,
        &signed_admission(&f, f.initial.intent()),
        &Time,
    ))
    .unwrap();
    for correct in [true, false] {
        let gateway =
            NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
        let peer = NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new(if correct {
            [42; 32]
        } else {
            [43; 32]
        })))
        .unwrap();
        let (a, b) = UnixStream::pair().unwrap();
        for stream in [&a, &b] {
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(5)))
                .unwrap();
        }
        let pin = gateway.public_spki();
        let server = thread::spawn(move || NativeTransport::accept(a, &gateway).unwrap());
        let _client = NativeTransport::connect(b, &peer, GatewayTrust::Pinned(pin)).unwrap();
        let channel = server.join().unwrap();
        let result =
            f.store
                .recover_owner_phone(f.request.id(), f.proof.digest(), channel.device_proof());
        if correct {
            assert_eq!(result.unwrap(), approved);
        } else {
            assert!(result.is_err());
        }
        assert_eq!(
            f.store
                .find_owner_phone(f.request.source(), f.request.id())
                .unwrap(),
            approved
        );
    }
}
#[test]
fn another_valid_owner_credential_cannot_stage_the_original_signed_consent() {
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
    assert!(f
        .store
        .stage_pairing(
            approved.id(),
            CredentialId::new("phone-credential").unwrap(),
            AttemptId::new([77; 16]),
            &admission,
            &Time
        )
        .is_err());
    assert_eq!(
        f.store
            .find_owner_phone(f.request.source(), f.request.id())
            .unwrap(),
        approved
    );
}

#[test]
fn a_replacement_owner_can_end_the_enrollment_and_cannot_stage_or_approve_it() {
    use crate::domain::pairing::DeviceKey;

    let f = signed_fixture();
    let approved = ready(f.store.authorize_owner_phone(
        &f.initial,
        &f.proof,
        &signed_admission(&f, f.initial.intent()),
        &Time,
    ))
    .unwrap();
    let second_request = OwnerPhoneRequest::new(
        OwnerPhoneRequestId::new([8; 16]).unwrap(),
        f.request.source().clone(),
        *f.request.gateway_pin(),
        f.request.endpoint(),
        f.request.device_key(),
    )
    .unwrap();
    let second_proof = verify_owner_phone_request(
        second_request.clone(),
        &f.identity
            .sign_owner_phone_request(&second_request)
            .unwrap(),
    )
    .unwrap();
    let second_intent = ConsentIntent::new(
        ConsentIntentId::new([18; 16]),
        1,
        f.initial.intent().audience().clone(),
        f.initial.intent().owner().clone(),
        f.initial.intent().membership().clone(),
        f.initial.intent().resource().clone(),
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let second_initial = PairingRecord::new_owner_phone(
        InvitationId::new([19; 16]),
        second_intent,
        Time.unix_milliseconds(),
        PairingPolicy::initial(),
    )
    .unwrap();
    let denied_target = ready(f.store.authorize_owner_phone(
        &second_initial,
        &second_proof,
        &signed_admission(&f, second_initial.intent()),
        &Time,
    ))
    .unwrap();
    let mut request = ordinary_issue("replacement-owner");
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
    .map(|action| grant("org-1", action))
    .collect();
    let outcome = f
        .store
        .issue_internal(request, IssuePath::SurfaceProvision)
        .unwrap();
    let evidence = match outcome {
        IssueCredentialOutcome::Issued { evidence, .. } => evidence,
        _ => panic!("expected a replacement owner credential"),
    };
    let replacement = ready(
        AuthenticateSession {
            policy: &crate::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
            verifier: f.store.as_ref(),
            access: f.store.as_ref(),
            clock: &Time,
        }
        .execute(&evidence, &AudienceId::new("gateway-1").unwrap()),
    )
    .unwrap();
    let admit = |intent: &ConsentIntent| {
        ready(
            AuthorizePairing {
                access: f.store.as_ref(),
                policy: &CedarPolicyEvaluator::new().unwrap(),
                clock: &Time,
            }
            .execute(&replacement, intent, intent.resource()),
        )
        .unwrap()
    };
    assert!(matches!(
        f.store.stage_pairing(
            approved.id(),
            CredentialId::new("phone-credential").unwrap(),
            AttemptId::new([77; 16]),
            &admit(approved.intent()),
            &Time,
        ),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    ));
    assert!(matches!(
        ready(f.store.decide_pairing(
            approved.id(),
            OwnerDecision::Approve(DeviceKey::new([2; 32])),
            &admit(approved.intent()),
            &Time,
        )),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    ));
    let cancelled = ready(f.store.decide_pairing(
        approved.id(),
        OwnerDecision::Cancel,
        &admit(approved.intent()),
        &Time,
    ))
    .unwrap();
    assert_eq!(cancelled.phase(), PairingPhase::Terminal);
    assert_eq!(
        cancelled.terminal().map(|(cause, _)| cause),
        Some(TerminalCause::Cancelled)
    );
    let denied = ready(f.store.decide_pairing(
        denied_target.id(),
        OwnerDecision::Deny,
        &admit(denied_target.intent()),
        &Time,
    ))
    .unwrap();
    assert_eq!(denied.phase(), PairingPhase::Terminal);
    assert_eq!(
        denied.terminal().map(|(cause, _)| cause),
        Some(TerminalCause::Denied)
    );
}

#[path = "owner_phone_purpose_tests.rs"]
mod owner_phone_purpose_tests;
