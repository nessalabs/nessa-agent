//! Current Auth purpose with real signed publication and real pinned TLS proof.
use super::*;
use crate::application::{pairing::OwnerPhonePurposeSource, ports::IssuePath};
fn active_signed(f: &SignedFixture) -> PairingRecord {
    let approved = ready(f.store.authorize_owner_phone(
        &f.initial,
        &f.proof,
        &signed_admission(f, f.initial.intent()),
        &Time,
    ))
    .unwrap();
    let credential = CredentialId::new("owner-phone-device").unwrap();
    let request = AttemptId::new([77; 16]);
    f.store
        .stage_pairing(
            approved.id(),
            credential.clone(),
            request,
            &signed_admission(f, approved.intent()),
            &Time,
        )
        .unwrap();
    let receipt = ReceiverOutcome::new(
        credential,
        request,
        ResourceId::new("actual-receiver").unwrap(),
        7,
        approved.intent().generation(),
    )
    .unwrap();
    f.store
        .remember_receiver(approved.id(), &receipt, &Time)
        .unwrap();
    f.store
        .publish_pairing(
            approved.id(),
            &signed_admission(f, approved.intent()),
            &Time,
        )
        .unwrap()
}
fn tls_channel(seed: u8) -> NativeTransport<UnixStream> {
    let gateway =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let device =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap();
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
    let _client = NativeTransport::connect(b, &device, GatewayTrust::Pinned(pin)).unwrap();
    server.join().unwrap()
}
fn device_session(
    store: &LocalCredentialStore,
    channel: &NativeTransport<UnixStream>,
    id: &str,
) -> Result<AuthenticatedSession, AccessError> {
    let verifier = store.device_verifier(channel.device_proof());
    ready(
        AuthenticateSession {
            policy: &crate::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
            verifier: &verifier,
            access: store,
            clock: &Time,
        }
        .execute(
            &CredentialEvidence::new(id.as_bytes().to_vec()).unwrap(),
            &AudienceId::new("gateway-1").unwrap(),
        ),
    )
}
#[test]
fn purpose_requires_actual_signed_active_device_and_retains_original_receiver_scope() {
    let f = signed_fixture();
    active_signed(&f);
    assert!(ready(f.store.owner_phone_purpose(&f.session, &Time))
        .unwrap()
        .is_none());
    let channel = tls_channel(42);
    let session = device_session(&f.store, &channel, "owner-phone-device").unwrap();
    let purpose = ready(f.store.owner_phone_purpose(&session, &Time))
        .unwrap()
        .unwrap();
    assert_eq!(purpose.credential().as_str(), "owner-phone-device");
    assert_eq!(purpose.receiver().as_str(), "actual-receiver");
    assert_eq!(purpose.paired_epoch(), 7);
    assert_eq!(purpose.owner(), f.request.source().owner());
    assert_eq!(purpose.membership(), f.request.source().membership());
    assert_eq!(purpose.audience(), f.request.source().audience());
    let wrong = tls_channel(43);
    assert!(device_session(&f.store, &wrong, "owner-phone-device").is_err());
}
#[test]
fn active_purpose_survives_expired_and_revoked_bootstrap_but_not_device_revocation() {
    struct Later;
    impl Clock for Later {
        fn unix_milliseconds(&self) -> u64 {
            201_000
        }
    }
    let f = signed_fixture();
    active_signed(&f);
    let channel = tls_channel(42);
    let session = device_session(&f.store, &channel, "owner-phone-device").unwrap();
    assert!(ready(f.store.owner_phone_purpose(&session, &Later))
        .unwrap()
        .is_some());
    f.store
        .revoke_sync(RevokeCredentialRequest {
            request_id: "retire-bootstrap".into(),
            issuer_principal_id: "owner".into(),
            credential_id: f.session.context().credential_id().as_str().into(),
            revoked_at: 110,
        })
        .unwrap();
    assert!(ready(f.store.owner_phone_purpose(&session, &Time))
        .unwrap()
        .is_some());
    f.store
        .revoke_sync(RevokeCredentialRequest {
            request_id: "revoke-phone".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "owner-phone-device".into(),
            revoked_at: 110,
        })
        .unwrap();
    assert!(matches!(
        ready(f.store.owner_phone_purpose(&session, &Time)),
        Err(AccessError::CredentialRevoked)
    ));
}
#[test]
fn all_legacy_canonical_pairing_classes_remain_without_owner_phone_purpose() {
    for fixture in [enrolled_claim(), peer_claim(), environment_claim()] {
        publish(&fixture);
        let session = device_session(&fixture.store, &fixture.channel, "native-device").unwrap();
        assert!(ready(fixture.store.owner_phone_purpose(&session, &Time))
            .unwrap()
            .is_none());
    }
}
#[test]
fn all_coarse_grants_on_an_ordinary_issued_credential_never_make_owner_phone_purpose() {
    let f = signed_fixture();
    let mut request = ordinary_issue("all-coarse-reader");
    request.grants = ["conversation.read", "conversation.write", "server.read"]
        .iter()
        .map(|a| grant("org-1", a))
        .collect();
    let IssueCredentialOutcome::Issued { evidence, .. } = f.store.issue_sync(request).unwrap()
    else {
        panic!("fresh credential")
    };
    let session = ready(
        AuthenticateSession {
            policy: &crate::adapters::cedar::CedarPolicyEvaluator::new().unwrap(),
            verifier: f.store.as_ref(),
            access: f.store.as_ref(),
            clock: &Time,
        }
        .execute(&evidence, &AudienceId::new("gateway-1").unwrap()),
    )
    .unwrap();
    assert!(ready(f.store.owner_phone_purpose(&session, &Time))
        .unwrap()
        .is_none());
}
#[test]
fn delegated_membership_loss_is_an_error_never_missing_purpose() {
    let f = signed_fixture();
    active_signed(&f);
    let channel = tls_channel(42);
    let session = device_session(&f.store, &channel, "owner-phone-device").unwrap();
    // Retain another active administrator so disabling this membership is valid registry state.
    let mut administrator = ordinary_issue("separate-administrator");
    administrator.principal.id = "other-admin".into();
    administrator.principal.kind = PrincipalKindDto::Human;
    administrator.membership.id = "other-admin-membership".into();
    administrator.membership.principal_id = "other-admin".into();
    administrator.membership.role = MembershipRoleDto::Admin;
    f.store
        .issue_internal(administrator, IssuePath::SurfaceProvision)
        .unwrap();
    // Trusted registry fixture changes current membership in one actual published revision.
    let mut slot = f.store.slot().unwrap();
    let mut next = slot.as_ref().unwrap().clone();
    // The registry requires a designated active owner membership; transfer it before disabling the prior owner.
    next.owner_membership_id = "other-admin-membership".into();
    let membership = next
        .memberships
        .iter_mut()
        .find(|m| m.id == session.context().membership_id().as_str())
        .unwrap();
    membership.state = MembershipStateDto::Disabled;
    next.revision += 1;
    f.store.persist(&next).unwrap();
    let revision = next.revision;
    *slot = Some(next.clone());
    f.store.publish_snapshot(next).unwrap();
    f.store.publish_revision(revision);
    drop(slot);
    assert!(matches!(
        ready(f.store.owner_phone_purpose(&session, &Time)),
        Err(AccessError::InactiveMembership)
    ));
}

#[test]
fn purpose_refuses_revision_changed_after_current_session_snapshot() {
    struct RevokeAfterSnapshot<'a> {
        store: &'a LocalCredentialStore,
        fired: std::sync::atomic::AtomicBool,
    }
    impl Clock for RevokeAfterSnapshot<'_> {
        fn unix_milliseconds(&self) -> u64 {
            assert!(!self.fired.swap(true, std::sync::atomic::Ordering::SeqCst));
            self.store
                .revoke_sync(RevokeCredentialRequest {
                    request_id: "revoke-between-snapshot-and-purpose".into(),
                    issuer_principal_id: "owner".into(),
                    credential_id: "owner-phone-device".into(),
                    revoked_at: 110,
                })
                .unwrap();
            Time.unix_milliseconds()
        }
    }
    let f = signed_fixture();
    active_signed(&f);
    let channel = tls_channel(42);
    let session = device_session(&f.store, &channel, "owner-phone-device").unwrap();
    let clock = RevokeAfterSnapshot {
        store: f.store.as_ref(),
        fired: std::sync::atomic::AtomicBool::new(false),
    };
    assert!(matches!(
        ready(f.store.owner_phone_purpose(&session, &clock)),
        Err(AccessError::StaleRevision)
    ));
    assert!(clock.fired.load(std::sync::atomic::Ordering::SeqCst));
    assert!(matches!(
        ready(f.store.owner_phone_purpose(&session, &Time)),
        Err(AccessError::CredentialRevoked)
    ));
}

#[path = "owner_phone_original_tests.rs"]
mod original_projection;
