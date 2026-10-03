use super::super::{
    tests::{bootstrap, grant, open_store, ready},
    BootstrapRequest, DeviceProofBinding, LocalCredentialStore, LocalStoreError, Registry,
    StoredProof,
};
#[cfg(unix)]
use crate::adapters::pairing::tests::io::{message_lengths, CryptoFixtureTransport};
use crate::{
    adapters::{
        cedar::CedarPolicyEvaluator,
        pairing::{
            tests::Entropy, ClientAttempt, FilePairingState, GatewayTrust, ManualCode,
            NativeIdentity, NativeTransport, ServerInvitation,
        },
    },
    application::{
        authorization::AuthorizeAction,
        credential_admin::{
            AuthRevisionSource, IssueCredentialOutcome, IssueCredentialRequest,
            RevokeCredentialRequest,
        },
        dto::{
            InitiatorDto, IssuanceCauseDto, MembershipInputDto, MembershipRoleDto,
            MembershipStateDto, PrincipalInputDto, PrincipalKindDto, TransitionCauseDto,
        },
        pairing::{
            resolve_terminal_stage, AttemptReservation, AuthorizePairing, GatewayKeyStore,
            OwnerDecision, PairingAdmission, PairingStore, PairingStoreError, PrivateKeyMaterial,
            PrivateStateError, ReceiverOutcome, RuntimeEnd, StageOwnership, StageReceiptLookup,
            StageResolution,
        },
        ports::{
            AccessError, AccessReader, AccessSnapshot, Clock, CredentialEvidence,
            CredentialVerifier, Decision, PolicyEvaluator, PortFuture, VerifiedCredential,
        },
        session::{AuthenticateSession, AuthenticatedSession, ReadCurrentSession},
    },
    domain::{
        pairing::{
            AttemptId, AttemptOutcome, ConsentIntent, ConsentIntentId, InvitationId, PairingError,
            PairingPhase, PairingPolicy, PairingRecord, PublicIntent, TerminalCause,
        },
        Action, AudienceId, AuthContext, CredentialId, MembershipId, MembershipRole,
        OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine};
use sha2::{Digest, Sha256};
use std::{
    future::Future,
    os::unix::{fs::PermissionsExt, net::UnixStream},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
    task::{Context, Poll, Waker},
    thread,
    time::Duration,
};
use tempfile::TempDir;
use zeroize::Zeroizing;
struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        110_001
    }
}
struct Fixture {
    directory: TempDir,
    store: Arc<LocalCredentialStore>,
    session: AuthenticatedSession,
    record: PairingRecord,
    gateway: Resource,
    channel: NativeTransport<UnixStream>,
    _client: NativeTransport<UnixStream>,
}
fn enrolled_claim() -> Fixture {
    claim_fixture(|public| public, None)
}
fn claim_fixture(
    public_for: impl FnOnce(PublicIntent) -> PublicIntent,
    refusal: Option<PairingError>,
) -> Fixture {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(open_store(directory.path().join("native/credentials.v1.json")).unwrap());
    let mut request = bootstrap();
    request.grants.push(grant("org-1", "conversation.read"));
    let bootstrap = store.bootstrap(request).unwrap();
    let audience = AudienceId::new("gateway-1").unwrap();
    let session = ready(
        AuthenticateSession {
            verifier: store.as_ref(),
            access: store.as_ref(),
            clock: &Time,
        }
        .execute(&bootstrap.evidence, &audience),
    )
    .unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org-1").unwrap(),
        ResourceId::new("gateway-1").unwrap(),
    );
    let intent = ConsentIntent::new(
        ConsentIntentId::new([3; 16]),
        1,
        audience,
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("owner-membership").unwrap(),
        gateway.clone(),
    )
    .unwrap();
    let admission = ready(
        AuthorizePairing {
            access: store.as_ref(),
            policy: &CedarPolicyEvaluator::new().unwrap(),
            clock: &Time,
        }
        .execute(&session, &intent, &gateway),
    )
    .unwrap();
    let record = PairingRecord::new(
        InvitationId::new([1; 16]),
        intent,
        110_001,
        PairingPolicy::initial(),
    )
    .unwrap();
    ready(store.create_pairing(&record, &admission, &Time)).unwrap();
    let public = public_for(
        PublicIntent::new(
            record.id(),
            AttemptId::new([2; 16]),
            record.intent().id(),
            1,
            record.expires_at_ms(),
        )
        .unwrap(),
    );
    let identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let client_identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([11; 32]))).unwrap();
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let invitation =
        ServerInvitation::register(&mut Entropy, &code, record.id(), identity.public_spki())
            .unwrap();
    let (a, b) = UnixStream::pair().unwrap();
    let (server_lengths, client_lengths) = message_lengths();
    for stream in [&a, &b] {
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
    }
    let server_store = store.clone();
    let server_path = directory.path().join("native/credentials.v1.json");
    let refused = refusal.is_some();
    let server = thread::spawn(move || {
        let mut channel = CryptoFixtureTransport::new(
            NativeTransport::accept(a, &identity).unwrap(),
            server_lengths,
        );
        let request = channel.read_message().unwrap();
        let input = Sha256::digest(&request).into();
        assert!(matches!(
            server_store
                .reserve_attempt(
                    public.invitation(),
                    public.attempt(),
                    channel.device_proof(),
                    input,
                    &Time
                )
                .unwrap(),
            AttemptReservation::Admitted(_)
        ));
        assert!(matches!(
            server_store
                .reserve_attempt(
                    public.invitation(),
                    public.attempt(),
                    channel.device_proof(),
                    input,
                    &Time
                )
                .unwrap(),
            AttemptReservation::Existing(_)
        ));
        let (attempt, response) = invitation
            .start(
                &mut Entropy,
                &request,
                channel.pairing_context(public).unwrap(),
            )
            .unwrap();
        channel.write_message(&response).unwrap();
        let confirmation = attempt.finish(&channel.read_message().unwrap()).unwrap();
        let before = std::fs::read(&server_path).unwrap();
        let result = server_store.confirm_claim(confirmation.proof(), &Time);
        if let Some(error) = refusal {
            assert_eq!(result, Err(PairingStoreError::Domain(error)));
            assert_eq!(std::fs::read(&server_path).unwrap(), before);
            let pending = server_store.read_pairing(public.invitation()).unwrap();
            assert_eq!(pending.phase(), PairingPhase::Available);
            assert_eq!(pending.charged_attempts(), 1);
            assert_eq!(
                pending.attempt_status(public.attempt(), channel.device_proof().key()),
                Ok(AttemptOutcome::Pending)
            );
            channel.write_message(b"refused").unwrap();
        } else {
            let claimed = result.unwrap();
            assert_eq!(claimed.phase(), PairingPhase::Claimed);
            assert_eq!(
                server_store
                    .confirm_claim(confirmation.proof(), &Time)
                    .unwrap(),
                claimed
            );
            channel.write_message(b"claimed").unwrap();
        }
        channel.into_transport()
    });
    let mut client = CryptoFixtureTransport::new(
        NativeTransport::connect(b, &client_identity, GatewayTrust::ManualBootstrap).unwrap(),
        client_lengths,
    );
    let (attempt, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    client.write_message(&request).unwrap();
    let finalization = attempt
        .finish(
            &mut Entropy,
            &code,
            &client.read_message().unwrap(),
            &client.pairing_context(public).unwrap(),
            |_| Ok(()),
        )
        .unwrap();
    client.write_message(&finalization).unwrap();
    assert_eq!(
        client.read_message().unwrap(),
        if refused { b"refused" } else { b"claimed" }
    );
    let channel = server.join().unwrap();
    Fixture {
        directory,
        store,
        session,
        record,
        gateway,
        channel,
        _client: client.into_transport(),
    }
}
fn confirmation_refuses_mismatched_public(public_for: impl FnOnce(PublicIntent) -> PublicIntent) {
    let fixture = claim_fixture(public_for, Some(PairingError::Conflict));
    let prior = fixture.store.read_pairing(fixture.record.id()).unwrap();
    assert_eq!(prior.phase(), PairingPhase::Available);
    assert_eq!(prior.charged_attempts(), 1);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let id = fixture.record.id();
    let Fixture {
        directory, store, ..
    } = fixture;
    drop(store);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(id).unwrap(), prior);
    drop(reopened);
    drop(directory);

    let canonical = enrolled_claim();
    let claimed = canonical.store.read_pairing(canonical.record.id()).unwrap();
    assert_eq!(claimed.phase(), PairingPhase::Claimed);
    assert_eq!(claimed.charged_attempts(), 1);
    assert_eq!(claimed.intent(), canonical.record.intent());
}
#[test]
fn confirmed_claim_requires_original_consent() {
    confirmation_refuses_mismatched_public(|public| {
        PublicIntent::new(
            public.invitation(),
            public.attempt(),
            ConsentIntentId::new([4; 16]),
            public.generation(),
            public.expiry_ms(),
        )
        .unwrap()
    });
}
#[test]
fn confirmed_claim_requires_original_generation() {
    confirmation_refuses_mismatched_public(|public| {
        PublicIntent::new(
            public.invitation(),
            public.attempt(),
            public.consent(),
            public.generation() + 1,
            public.expiry_ms(),
        )
        .unwrap()
    });
}
#[test]
fn confirmed_claim_requires_original_expiry() {
    confirmation_refuses_mismatched_public(|public| {
        PublicIntent::new(
            public.invitation(),
            public.attempt(),
            public.consent(),
            public.generation(),
            public.expiry_ms() + 1,
        )
        .unwrap()
    });
}
fn stage(fixture: &Fixture) {
    let policy = CedarPolicyEvaluator::new().unwrap();
    let authorize = AuthorizePairing {
        access: fixture.store.as_ref(),
        policy: &policy,
        clock: &Time,
    };
    let admission =
        ready(authorize.execute(&fixture.session, fixture.record.intent(), &fixture.gateway))
            .unwrap();
    let key = fixture.channel.device_proof().key();
    ready(fixture.store.decide_pairing(
        fixture.record.id(),
        OwnerDecision::Approve(key),
        &admission,
        &Time,
    ))
    .unwrap();
    let admission =
        ready(authorize.execute(&fixture.session, fixture.record.intent(), &fixture.gateway))
            .unwrap();
    let credential = CredentialId::new("native-device").unwrap();
    let request = AttemptId::new([9; 16]);
    fixture
        .store
        .stage_pairing(
            fixture.record.id(),
            credential.clone(),
            request,
            &admission,
            &Time,
        )
        .unwrap();
}
fn publish(fixture: &Fixture) -> PairingRecord {
    stage(fixture);
    complete_stage(fixture)
}
fn complete_stage(fixture: &Fixture) -> PairingRecord {
    let policy = CedarPolicyEvaluator::new().unwrap();
    let authorize = AuthorizePairing {
        access: fixture.store.as_ref(),
        policy: &policy,
        clock: &Time,
    };
    let credential = CredentialId::new("native-device").unwrap();
    let request = AttemptId::new([9; 16]);
    // A trusted receiver adapter is represented here by its physical outcome; server acceptance
    // tests exercise the actual canonical SQLite owner separately.
    let outcome = ReceiverOutcome::new(
        credential,
        request,
        ResourceId::new("receiver").unwrap(),
        1,
        1,
    )
    .unwrap();
    fixture
        .store
        .remember_receiver(fixture.record.id(), &outcome, &Time)
        .unwrap();
    let admission =
        ready(authorize.execute(&fixture.session, fixture.record.intent(), &fixture.gateway))
            .unwrap();
    fixture
        .store
        .publish_pairing(fixture.record.id(), &admission, &Time)
        .unwrap()
}
struct HeldPairingClock {
    entered: mpsc::Sender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    held: Arc<AtomicBool>,
    expiry_ms: u64,
}
impl Clock for HeldPairingClock {
    fn unix_milliseconds(&self) -> u64 {
        self.held.store(true, Ordering::Release);
        let _ = self.entered.send(());
        if let Ok(release) = self.release.lock() {
            let _ = release.recv_timeout(Duration::from_secs(10));
        }
        self.held.store(false, Ordering::Release);
        self.expiry_ms
    }
}
struct ReleasePairingMutation(Option<mpsc::Sender<()>>);
impl Drop for ReleasePairingMutation {
    fn drop(&mut self) {
        if let Some(release) = self.0.take() {
            let _ = release.send(());
        }
    }
}
#[test]
fn device_admission_reads_committed_snapshot_during_pairing_mutation() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let audience = AudienceId::new("gateway-1").unwrap();
    let evidence = CredentialEvidence::new(b"native-device".to_vec()).unwrap();
    let expected = ready(
        fixture
            .store
            .device_verifier(fixture.channel.device_proof())
            .verify(&evidence, &audience),
    )
    .unwrap();
    assert_eq!(
        expected.credential_id,
        CredentialId::new("native-device").unwrap()
    );

    let intent = ConsentIntent::new(
        ConsentIntentId::new([25; 16]),
        1,
        audience.clone(),
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("owner-membership").unwrap(),
        fixture.gateway.clone(),
    )
    .unwrap();
    let neighbor = PairingRecord::new(
        InvitationId::new([24; 16]),
        intent,
        Time.unix_milliseconds(),
        PairingPolicy::initial(),
    )
    .unwrap();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, neighbor.intent(), &fixture.gateway),
    )
    .unwrap();
    ready(fixture.store.create_pairing(&neighbor, &admission, &Time)).unwrap();
    let revision = fixture.store.revision().unwrap();
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let (verified_tx, verified_rx) = mpsc::channel();
    let held = Arc::new(AtomicBool::new(false));
    let clock = HeldPairingClock {
        entered: entered_tx,
        release: Mutex::new(release_rx),
        held: held.clone(),
        expiry_ms: neighbor.expires_at_ms(),
    };
    let proof = fixture.channel.device_proof();
    let (entered, early, mutation, verification) = thread::scope(|threads| {
        let release = ReleasePairingMutation(Some(release_tx));
        let mutation = threads.spawn(|| fixture.store.expire_pairing_if_due(neighbor.id(), &clock));
        let entered = entered_rx.recv_timeout(Duration::from_secs(2));
        let verification = threads.spawn(|| {
            let result = ready(
                fixture
                    .store
                    .device_verifier(proof)
                    .verify(&evidence, &audience),
            );
            let _ = verified_tx.send((result, held.load(Ordering::Acquire)));
        });
        let early = verified_rx.recv_timeout(Duration::from_secs(2));
        // No assertion occurs while the Clock holds the original mutation.
        // Even either observation timing out releases and joins both threads.
        drop(release);
        let mutation = mutation.join();
        let verification = verification.join();
        (entered, early, mutation, verification)
    });
    assert!(entered.is_ok(), "the public mutation must enter its Clock");
    assert!(
        early.is_ok(),
        "device verification must finish before pairing mutation release: {early:?}"
    );
    let (verified, was_held) = early.unwrap();
    assert!(was_held, "the original mutation must still be held");
    assert_eq!(verified, Ok(expected.clone()));
    verification.unwrap();
    assert_eq!(mutation.unwrap().unwrap().phase(), PairingPhase::Terminal);
    assert_eq!(fixture.store.revision().unwrap(), revision + 1);
    assert_eq!(
        ready(
            fixture
                .store
                .device_verifier(fixture.channel.device_proof())
                .verify(&evidence, &audience)
        ),
        Ok(expected)
    );
    fixture
        .store
        .revoke_sync(RevokeCredentialRequest {
            request_id: "revoke-after-held-pairing".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "native-device".into(),
            revoked_at: 110,
        })
        .unwrap();
    assert_eq!(
        ready(
            fixture
                .store
                .device_verifier(fixture.channel.device_proof())
                .verify(&evidence, &audience)
        ),
        Err(AccessError::InvalidCredential)
    );
}
#[test]
fn native_key_publication_and_canonical_explicit_revoke_agree() {
    let fixture = enrolled_claim();
    let active = publish(&fixture);
    assert_eq!(active.phase(), PairingPhase::Active);
    let verifier = fixture
        .store
        .device_verifier(fixture.channel.device_proof());
    let evidence = CredentialEvidence::new(b"native-device".to_vec()).unwrap();
    let audience = AudienceId::new("gateway-1").unwrap();
    let device = ready(
        AuthenticateSession {
            verifier: &verifier,
            access: fixture.store.as_ref(),
            clock: &Time,
        }
        .execute(&evidence, &audience),
    )
    .unwrap();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let authorize = AuthorizeAction {
        access: fixture.store.as_ref(),
        policy: &policy,
        clock: &Time,
    };
    assert_eq!(
        ready(authorize.execute(
            &device,
            &Action::new("conversation.read").unwrap(),
            &fixture.gateway
        ))
        .unwrap(),
        Decision::Allow
    );
    for action in ["conversation.write", "credential.manage"] {
        assert_eq!(
            ready(authorize.execute(&device, &Action::new(action).unwrap(), &fixture.gateway))
                .unwrap(),
            Decision::Deny
        );
    }
    assert_eq!(
        ready(fixture.store.verify(&evidence, &audience)).unwrap_err(),
        AccessError::InvalidCredential
    );
    let stale = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    let revoke = RevokeCredentialRequest {
        request_id: "revoke-native".into(),
        issuer_principal_id: "owner".into(),
        credential_id: "native-device".into(),
        revoked_at: 110,
    };
    let first = fixture.store.revoke_sync(revoke.clone()).unwrap();
    assert_eq!(fixture.store.revoke_sync(revoke).unwrap(), first);
    assert_eq!(
        fixture
            .store
            .publish_pairing(fixture.record.id(), &stale, &Time)
            .unwrap_err(),
        PairingStoreError::StaleRevision
    );
    let terminal = fixture.store.read_pairing(fixture.record.id()).unwrap();
    assert_eq!(terminal.phase(), PairingPhase::Terminal);
    assert_eq!(
        terminal.terminal().unwrap().0,
        TerminalCause::CredentialRevoked
    );
    assert!(terminal.cleanup_pending());
    assert_eq!(
        ready(verifier.verify(&evidence, &audience)).unwrap_err(),
        AccessError::InvalidCredential
    );
    assert_eq!(
        ready(
            ReadCurrentSession {
                access: fixture.store.as_ref(),
                clock: &Time
            }
            .execute(&fixture.session)
        )
        .unwrap()
        .membership
        .role(),
        MembershipRole::Admin
    );
    let id = fixture.record.id();
    let path = fixture.directory.path().join("native/credentials.v1.json");
    drop(fixture.store);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(id).unwrap(), terminal);
}
#[test]
fn owner_recovery_preserves_read_only_key_binding() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let outcome = fixture
        .store
        .recover_owner("recovered-owner".into(), 111, None)
        .unwrap();
    let active = fixture.store.read_pairing(fixture.record.id()).unwrap();
    assert_eq!(active.phase(), PairingPhase::Active);
    assert!(outcome
        .transitions
        .iter()
        .all(|event| event.credential_id != "native-device"));
    let verifier = fixture
        .store
        .device_verifier(fixture.channel.device_proof());
    let evidence = CredentialEvidence::new(b"native-device".to_vec()).unwrap();
    let device = ready(
        AuthenticateSession {
            verifier: &verifier,
            access: fixture.store.as_ref(),
            clock: &Time,
        }
        .execute(&evidence, &AudienceId::new("gateway-1").unwrap()),
    )
    .unwrap();
    assert_eq!(
        ready(
            ReadCurrentSession {
                access: fixture.store.as_ref(),
                clock: &Time
            }
            .execute(&device)
        )
        .unwrap()
        .membership
        .role(),
        MembershipRole::Admin
    );
    let id = fixture.record.id();
    let path = fixture.directory.path().join("native/credentials.v1.json");
    drop(fixture.store);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(id).unwrap(), active);
}

#[test]
fn runtime_restart_and_expiry_preserve_first_cause_and_active_credentials() {
    struct Expiry;
    impl Clock for Expiry {
        fn unix_milliseconds(&self) -> u64 {
            710_001
        }
    }
    let fixture = enrolled_claim();
    assert!(matches!(
        fixture
            .store
            .end_pairing(fixture.record.id(), RuntimeEnd::Restarted, &Time),
        Err(PairingStoreError::Domain(_))
    ));
    let active = publish(&fixture);
    assert!(matches!(
        fixture
            .store
            .end_pairing(active.id(), RuntimeEnd::Expired, &Expiry),
        Err(PairingStoreError::Domain(_))
    ));
    assert_eq!(fixture.store.read_pairing(active.id()).unwrap(), active);
    let intent = ConsentIntent::new(
        ConsentIntentId::new([80; 16]),
        1,
        fixture.record.intent().audience().clone(),
        fixture.record.intent().owner().clone(),
        fixture.record.intent().membership().clone(),
        fixture.gateway.clone(),
    )
    .unwrap();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let admission = ready(
        (AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        })
        .execute(&fixture.session, &intent, &fixture.gateway),
    )
    .unwrap();
    let available = PairingRecord::new(
        InvitationId::new([81; 16]),
        intent,
        110_001,
        PairingPolicy::initial(),
    )
    .unwrap();
    ready(fixture.store.create_pairing(&available, &admission, &Time)).unwrap();
    fixture
        .store
        .reserve_attempt(
            available.id(),
            AttemptId::new([82; 16]),
            fixture.channel.device_proof(),
            [83; 32],
            &Time,
        )
        .unwrap();
    assert_eq!(fixture.store.pending_pairings().unwrap().len(), 1);
    let ended = fixture
        .store
        .end_pairing(available.id(), RuntimeEnd::Restarted, &Time)
        .unwrap();
    assert_eq!(ended.terminal().unwrap().0, TerminalCause::Restarted);
    assert_eq!(
        ended
            .attempt_status(
                AttemptId::new([82; 16]),
                fixture.channel.device_proof().key()
            )
            .unwrap(),
        AttemptOutcome::Superseded
    );
    assert_eq!(
        fixture
            .store
            .end_pairing(available.id(), RuntimeEnd::Expired, &Expiry)
            .unwrap(),
        ended
    );
    assert!(fixture.store.pending_pairings().unwrap().is_empty());
    let expiry_intent = ConsentIntent::new(
        ConsentIntentId::new([84; 16]),
        1,
        fixture.record.intent().audience().clone(),
        fixture.record.intent().owner().clone(),
        fixture.record.intent().membership().clone(),
        fixture.gateway.clone(),
    )
    .unwrap();
    let admission = ready(
        (AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        })
        .execute(&fixture.session, &expiry_intent, &fixture.gateway),
    )
    .unwrap();
    let expiring = PairingRecord::new(
        InvitationId::new([85; 16]),
        expiry_intent,
        110_001,
        PairingPolicy::initial(),
    )
    .unwrap();
    ready(fixture.store.create_pairing(&expiring, &admission, &Time)).unwrap();
    assert!(matches!(
        fixture
            .store
            .end_pairing(expiring.id(), RuntimeEnd::Expired, &Time),
        Err(PairingStoreError::Domain(_))
    ));
    let expired = fixture
        .store
        .end_pairing(expiring.id(), RuntimeEnd::Expired, &Expiry)
        .unwrap();
    assert_eq!(expired.terminal().unwrap().0, TerminalCause::Expired);
    assert_eq!(
        fixture
            .store
            .end_pairing(expired.id(), RuntimeEnd::Restarted, &Expiry)
            .unwrap(),
        expired
    );
    let path = fixture.directory.path().join("native/credentials.v1.json");
    // Retain the private directory while dropping the serving registry owner.
    let Fixture {
        directory: _directory,
        store,
        ..
    } = fixture;
    drop(store);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(ended.id()).unwrap(), ended);
    assert_eq!(reopened.read_pairing(active.id()).unwrap(), active);
    assert_eq!(reopened.read_pairing(expired.id()).unwrap(), expired);
    assert!(reopened.pending_pairings().unwrap().is_empty());
}

#[test]
fn admission_requires_exact_gateway_resource_before_any_enrollment_effect() {
    let fixture = enrolled_claim();
    let wrong = ConsentIntent::new(
        ConsentIntentId::new([90; 16]),
        1,
        fixture.record.intent().audience().clone(),
        fixture.record.intent().owner().clone(),
        fixture.record.intent().membership().clone(),
        Resource::new(
            fixture.gateway.organization_id().clone(),
            ResourceId::new("other-resource").unwrap(),
        ),
    )
    .unwrap();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let authorize = AuthorizePairing {
        access: fixture.store.as_ref(),
        policy: &policy,
        clock: &Time,
    };
    assert!(matches!(
        ready(authorize.execute(&fixture.session, &wrong, &fixture.gateway)),
        Err(AccessError::IdentityMismatch)
    ));
    assert!(
        ready(authorize.execute(&fixture.session, fixture.record.intent(), &fixture.gateway))
            .is_ok()
    );
    assert_eq!(
        fixture
            .store
            .read_pairing(fixture.record.id())
            .unwrap()
            .phase(),
        PairingPhase::Claimed
    );
    assert_eq!(fixture.store.pending_pairings().unwrap().len(), 1);
}

fn private_keys(directory: &TempDir) -> FilePairingState {
    let root = directory.path().canonicalize().unwrap();
    std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700)).unwrap();
    nessa_local_storage::create_directory_beneath(&root, Path::new("keys")).unwrap();
    FilePairingState::open(&root, Path::new("keys")).unwrap()
}
#[test]
fn missing_key_with_terminal_history_refuses() {
    let fixture = enrolled_claim();
    let keys_directory = tempfile::tempdir().unwrap();
    let keys = private_keys(&keys_directory);
    let admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &CedarPolicyEvaluator::new().unwrap(),
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    let terminal = ready(fixture.store.decide_pairing(
        fixture.record.id(),
        OwnerDecision::Cancel,
        &admission,
        &Time,
    ))
    .unwrap();
    assert_eq!(terminal.phase(), PairingPhase::Terminal);
    assert!(fixture.store.pending_pairings().unwrap().is_empty());
    let key = PrivateKeyMaterial::new(Zeroizing::new([10; 32]));
    assert_eq!(
        fixture.store.publish_first_gateway_key(&keys, &key, &Time),
        Err(PairingStoreError::GatewayKeyHistoryExists)
    );
    assert!(keys
        .restore_gateway_key(&AudienceId::new("gateway-1").unwrap(), &Time)
        .unwrap()
        .is_none());
    let path = fixture.directory.path().join("native/credentials.v1.json");
    drop(fixture.store);
    let reopened = open_store(path).unwrap();
    assert_eq!(
        reopened.publish_first_gateway_key(&keys, &key, &Time),
        Err(PairingStoreError::GatewayKeyHistoryExists)
    );
    assert_eq!(reopened.read_pairing(terminal.id()).unwrap(), terminal);
}
#[test]
fn first_key_publication_holds_current_history_admission() {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(open_store(directory.path().join("native/credentials.v1.json")).unwrap());
    let mut request = bootstrap();
    request.grants.push(grant("org-1", "conversation.read"));
    let bootstrapped = store.bootstrap(request).unwrap();
    let audience = AudienceId::new("gateway-1").unwrap();
    let session = ready(
        AuthenticateSession {
            verifier: store.as_ref(),
            access: store.as_ref(),
            clock: &Time,
        }
        .execute(&bootstrapped.evidence, &audience),
    )
    .unwrap();
    let gateway = Resource::new(
        OrganizationId::new("org-1").unwrap(),
        ResourceId::new("gateway-1").unwrap(),
    );
    let intent = ConsentIntent::new(
        ConsentIntentId::new([3; 16]),
        1,
        audience.clone(),
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("owner-membership").unwrap(),
        gateway.clone(),
    )
    .unwrap();
    let admission = ready(
        AuthorizePairing {
            access: store.as_ref(),
            policy: &CedarPolicyEvaluator::new().unwrap(),
            clock: &Time,
        }
        .execute(&session, &intent, &gateway),
    )
    .unwrap();
    let record = PairingRecord::new(
        InvitationId::new([1; 16]),
        intent,
        110_001,
        PairingPolicy::initial(),
    )
    .unwrap();
    let keys_directory = tempfile::tempdir().unwrap();
    let keys = Arc::new(private_keys(&keys_directory));
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    struct HeldKeys {
        keys: Arc<FilePairingState>,
        entered: mpsc::Sender<()>,
        release: Mutex<mpsc::Receiver<()>>,
    }
    impl GatewayKeyStore for HeldKeys {
        fn restore_gateway_key(
            &self,
            gateway: &AudienceId,
            clock: &dyn Clock,
        ) -> Result<Option<PrivateKeyMaterial>, PrivateStateError> {
            self.keys.restore_gateway_key(gateway, clock)
        }
        fn save_gateway_key(
            &self,
            key: &PrivateKeyMaterial,
            gateway: &AudienceId,
            clock: &dyn Clock,
        ) -> Result<(), PrivateStateError> {
            self.entered.send(()).unwrap();
            self.release.lock().unwrap().recv().unwrap();
            self.keys.save_gateway_key(key, gateway, clock)
        }
    }
    let held = HeldKeys {
        keys: keys.clone(),
        entered: entered_tx,
        release: Mutex::new(release_rx),
    };
    let publisher_store = store.clone();
    let publisher = thread::spawn(move || {
        publisher_store.publish_first_gateway_key(
            &held,
            &PrivateKeyMaterial::new(Zeroizing::new([10; 32])),
            &Time,
        )
    });
    entered_rx.recv_timeout(Duration::from_secs(5)).unwrap();
    // The canonical owner, not a separate publication flag, excludes create.
    let (second_started_tx, second_started_rx) = mpsc::channel();
    let second_store = store.clone();
    let second_keys = keys.clone();
    let second = thread::spawn(move || {
        second_started_tx.send(()).unwrap();
        second_store.publish_first_gateway_key(
            second_keys.as_ref(),
            &PrivateKeyMaterial::new(Zeroizing::new([11; 32])),
            &Time,
        )
    });
    second_started_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    let (create_started_tx, create_started_rx) = mpsc::channel();
    let creator_store = store.clone();
    let creator = thread::spawn(move || {
        create_started_tx.send(()).unwrap();
        ready(creator_store.create_pairing(&record, &admission, &Time))
    });
    create_started_rx
        .recv_timeout(Duration::from_secs(5))
        .unwrap();
    assert!(keys
        .restore_gateway_key(&audience, &Time)
        .unwrap()
        .is_none());
    release_tx.send(()).unwrap();
    assert_eq!(publisher.join().unwrap(), Ok(()));
    assert!(matches!(
        second.join().unwrap(),
        Err(PairingStoreError::PrivateState(PrivateStateError::Conflict)
            | PairingStoreError::GatewayKeyHistoryExists)
    ));
    creator.join().unwrap().unwrap();
    let saved = keys.restore_gateway_key(&audience, &Time).unwrap().unwrap();
    assert_eq!(saved.expose_bytes(), &[10; 32]);
    assert_eq!(
        store.publish_first_gateway_key(keys.as_ref(), &saved, &Time),
        Err(PairingStoreError::GatewayKeyHistoryExists)
    );
    // Restore of an existing key is allowed with history; it issues no new key.
    assert_eq!(
        keys.restore_gateway_key(&audience, &Time)
            .unwrap()
            .unwrap()
            .expose_bytes(),
        &[10; 32]
    );
}

fn ordinary_issue(identity: &str) -> IssueCredentialRequest {
    IssueCredentialRequest {
        request_id: format!("issue-{identity}"),
        issuer_principal_id: "owner".into(),
        credential_id: identity.into(),
        principal: PrincipalInputDto {
            id: "reader".into(),
            kind: PrincipalKindDto::Agent,
        },
        membership: MembershipInputDto {
            id: "reader-membership".into(),
            principal_id: "reader".into(),
            organization_id: "org-1".into(),
            role: MembershipRoleDto::Member,
            state: MembershipStateDto::Active,
        },
        audience_id: "gateway-1".into(),
        issued_at: 110,
        expires_at: None,
        grants: vec![grant("org-1", "conversation.read")],
    }
}
#[test]
fn staged_identity_refuses_all_unrelated_issuance_and_exact_pairing_completes() {
    let fixture = enrolled_claim();
    stage(&fixture);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        fixture.store.issue_sync(ordinary_issue("native-device")),
        Err(LocalStoreError::Conflict)
    ));
    assert!(matches!(
        fixture.store.provision_surface(
            "reader",
            "surface-retry".into(),
            "native-device".into(),
            vec!["conversation.read".into()],
            110,
            None
        ),
        Err(LocalStoreError::Conflict)
    ));
    assert!(matches!(
        fixture
            .store
            .recover_owner("native-device".into(), 110, None),
        Err(LocalStoreError::Conflict)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    fixture
        .store
        .issue_sync(ordinary_issue("distinct-reader"))
        .unwrap();
    assert_eq!(complete_stage(&fixture).phase(), PairingPhase::Active);
    drop(fixture.store);
    assert!(open_store(path).is_ok());
}
#[test]
fn cancelled_stage_keeps_identity_reserved_after_restart() {
    let fixture = enrolled_claim();
    stage(&fixture);
    let policy = CedarPolicyEvaluator::new().unwrap();
    let admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    ready(fixture.store.decide_pairing(
        fixture.record.id(),
        OwnerDecision::Cancel,
        &admission,
        &Time,
    ))
    .unwrap();
    let path = fixture.directory.path().join("native/credentials.v1.json");
    drop(fixture.store);
    let store = open_store(&path).unwrap();
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        store.issue_sync(ordinary_issue("native-device")),
        Err(LocalStoreError::Conflict)
    ));
    assert_eq!(std::fs::read(path).unwrap(), before);
    store.issue_sync(ordinary_issue("distinct-reader")).unwrap();
}
#[test]
fn restored_activation_requires_original_credential_proof_cause_actor_and_time() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let original: Registry = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    drop(fixture.store);
    for tamper in 0..7 {
        let mut changed = original.clone();
        let issued = changed
            .transitions
            .iter_mut()
            .find(|entry| entry.credential_id == "native-device")
            .unwrap();
        match tamper {
            0 => {
                issued.cause = TransitionCauseDto::Issued {
                    cause: IssuanceCauseDto::SurfaceProvision,
                }
            }
            1 => {
                changed
                    .credentials
                    .iter_mut()
                    .find(|entry| entry.metadata.id == "native-device")
                    .unwrap()
                    .verifier = StoredProof::Bearer(URL_SAFE_NO_PAD.encode([42; 32]))
            }
            2 => {
                changed
                    .credentials
                    .retain(|entry| entry.metadata.id != "native-device");
                changed
                    .transitions
                    .retain(|entry| entry.credential_id != "native-device");
                for (index, entry) in changed.transitions.iter_mut().enumerate() {
                    entry.sequence = index as u64 + 1;
                }
            }
            3 => issued.initiator = InitiatorDto::LocalOperator,
            4 => {
                issued.at += 1;
                issued.after.issued_at += 1;
                changed
                    .credentials
                    .iter_mut()
                    .find(|entry| entry.metadata.id == "native-device")
                    .unwrap()
                    .metadata
                    .issued_at += 1;
            }
            5 | 6 => {
                let StoredProof::Device(DeviceProofBinding::DevicePairing {
                    invitation,
                    generation,
                }) = &mut changed
                    .credentials
                    .iter_mut()
                    .find(|entry| entry.metadata.id == "native-device")
                    .unwrap()
                    .verifier
                else {
                    panic!("actual device proof")
                };
                if tamper == 5 {
                    invitation[0] = 99;
                } else {
                    *generation += 1;
                }
            }
            _ => unreachable!(),
        }
        let bytes = serde_json::to_vec(&changed).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            matches!(
                open_store(&path),
                Err(LocalStoreError::InvalidRegistry { .. })
            ),
            "tamper {tamper}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    assert!(open_store(path).is_ok());
}

struct At(u64);
impl Clock for At {
    fn unix_milliseconds(&self) -> u64 {
        self.0
    }
}
struct ReceiptLookup {
    returned: Mutex<Option<StageOwnership>>,
    held: Mutex<Option<StageOwnership>>,
    receipt: Option<ReceiverOutcome>,
    unavailable: bool,
    pending: bool,
    calls: AtomicUsize,
}
impl ReceiptLookup {
    fn absent() -> Self {
        Self {
            returned: Mutex::new(None),
            held: Mutex::new(None),
            receipt: None,
            unavailable: false,
            pending: false,
            calls: AtomicUsize::new(0),
        }
    }
}
impl StageReceiptLookup for ReceiptLookup {
    fn lookup<'a>(
        &'a self,
        stage: StageOwnership,
    ) -> PortFuture<'a, (StageOwnership, Option<ReceiverOutcome>), PairingStoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.pending {
            *self.held.lock().unwrap() = Some(stage);
            return Box::pin(std::future::pending());
        }
        Box::pin(async move {
            if self.unavailable {
                return Err(PairingStoreError::Unavailable);
            }
            let returned = self.returned.lock().unwrap().take();
            let ownership = if let Some(returned) = returned {
                *self.held.lock().unwrap() = Some(stage);
                returned
            } else {
                stage
            };
            Ok((ownership, self.receipt.clone()))
        })
    }
}
fn cancel_stage(fixture: &Fixture) {
    let policy = CedarPolicyEvaluator::new().unwrap();
    let admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    ready(fixture.store.decide_pairing(
        fixture.record.id(),
        OwnerDecision::Cancel,
        &admission,
        &Time,
    ))
    .unwrap();
}
#[test]
fn expired_stages_keep_reserved_identity_and_published_activation_is_not_expired() {
    for published in [false, true] {
        let fixture = enrolled_claim();
        if published {
            publish(&fixture);
        } else {
            stage(&fixture);
        }
        let original = fixture.store.read_pairing(fixture.record.id()).unwrap();
        let expired = fixture
            .store
            .expire_pairing_if_due(fixture.record.id(), &At(710_001))
            .unwrap();
        if published {
            assert_eq!(expired, original);
            assert_eq!(expired.phase(), PairingPhase::Active);
        } else {
            assert_eq!(expired.terminal().unwrap().0, TerminalCause::Expired);
            let lease = fixture
                .store
                .clone()
                .acquire_stage(fixture.record.id())
                .unwrap();
            let lookup = ReceiptLookup::absent();
            let StageResolution::NoReceiver(proof) =
                ready(resolve_terminal_stage(lease, &lookup)).unwrap()
            else {
                panic!("absent lookup cannot invent receiver evidence")
            };
            let cleaned = fixture
                .store
                .finish_no_receiver(&proof, &At(710_001))
                .unwrap();
            assert!(!cleaned.cleanup_pending());
            assert_eq!(cleaned.stage_binding(), original.stage_binding());
            assert_eq!(cleaned.terminal().unwrap().0, TerminalCause::Expired);
            drop(proof);
        }
        let path = fixture.directory.path().join("native/credentials.v1.json");
        let final_record = fixture.store.read_pairing(fixture.record.id()).unwrap();
        drop(fixture.store);
        let reopened = open_store(&path).unwrap();
        assert_eq!(
            reopened.read_pairing(final_record.id()).unwrap(),
            final_record
        );
        let before = std::fs::read(&path).unwrap();
        assert!(matches!(
            reopened.issue_sync(ordinary_issue("native-device")),
            Err(LocalStoreError::Conflict)
        ));
        assert!(matches!(
            reopened.provision_surface(
                "reader",
                "surface-retry".into(),
                "native-device".into(),
                vec!["conversation.read".into()],
                110,
                None
            ),
            Err(LocalStoreError::Conflict)
        ));
        assert!(matches!(
            reopened.recover_owner("native-device".into(), 110, None),
            Err(LocalStoreError::Conflict)
        ));
        assert_eq!(std::fs::read(&path).unwrap(), before);
        reopened
            .issue_sync(ordinary_issue("distinct-reader"))
            .unwrap();
    }
}
#[test]
fn returned_stage_ownership_and_receipts_must_match_original_registry() {
    let fixture = enrolled_claim();
    stage(&fixture);
    let original = fixture
        .store
        .clone()
        .acquire_stage(fixture.record.id())
        .unwrap();
    cancel_stage(&fixture);
    let foreign = enrolled_claim();
    stage(&foreign);
    cancel_stage(&foreign);
    let returned = foreign
        .store
        .clone()
        .acquire_stage(foreign.record.id())
        .unwrap();
    assert_eq!(original.record().id(), returned.record().id());
    assert_eq!(original.record().intent(), returned.record().intent());
    assert_eq!(
        original.record().claim_binding(),
        returned.record().claim_binding()
    );
    assert_eq!(
        original.record().stage_binding(),
        returned.record().stage_binding()
    );
    let lookup = ReceiptLookup::absent();
    *lookup.returned.lock().unwrap() = Some(returned);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    assert!(matches!(
        ready(resolve_terminal_stage(original, &lookup)),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(matches!(
        fixture.store.clone().acquire_stage(fixture.record.id()),
        Err(PairingStoreError::StageOccupied)
    ));
    drop(lookup.held.lock().unwrap().take());
    for mismatch in 0..4 {
        let mut lookup = ReceiptLookup::absent();
        lookup.receipt = Some(
            ReceiverOutcome::new(
                CredentialId::new(if mismatch == 0 {
                    "foreign-credential"
                } else {
                    "native-device"
                })
                .unwrap(),
                AttemptId::new(if mismatch == 1 { [10; 16] } else { [9; 16] }),
                ResourceId::new("receiver").unwrap(),
                1,
                if mismatch == 2 { 2 } else { 1 },
            )
            .unwrap(),
        );
        let lease = fixture
            .store
            .clone()
            .acquire_stage(fixture.record.id())
            .unwrap();
        let resolved = ready(resolve_terminal_stage(lease, &lookup));
        if mismatch < 3 {
            assert!(matches!(
                resolved,
                Err(PairingStoreError::Domain(PairingError::Conflict))
            ));
        } else {
            let StageResolution::Receiver { ownership, outcome } = resolved.unwrap() else {
                panic!("exact receipt expected")
            };
            assert_eq!(outcome, lookup.receipt.unwrap());
            assert!(matches!(
                fixture.store.clone().acquire_stage(fixture.record.id()),
                Err(PairingStoreError::StageOccupied)
            ));
            drop(ownership);
        }
        assert_eq!(
            std::fs::read(&path).unwrap(),
            before,
            "resolution alone commits no receiver/cleanup"
        );
    }
}
#[test]
fn terminal_stage_refuses_recorded_receiver_before_lookup() {
    let fixture = enrolled_claim();
    stage(&fixture);
    let lease = fixture
        .store
        .clone()
        .acquire_stage(fixture.record.id())
        .unwrap();
    cancel_stage(&fixture);
    let outcome = ReceiverOutcome::new(
        CredentialId::new("native-device").unwrap(),
        AttemptId::new([9; 16]),
        ResourceId::new("receiver").unwrap(),
        1,
        1,
    )
    .unwrap();
    let terminal = fixture
        .store
        .remember_receiver(fixture.record.id(), &outcome, &Time)
        .unwrap();
    assert_eq!(terminal.phase(), PairingPhase::Terminal);
    assert!(terminal.cleanup_pending());
    assert_eq!(terminal.receiver_binding(), Some((outcome.receiver(), 1)));
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    let lookup = ReceiptLookup::absent();
    assert!(matches!(
        ready(resolve_terminal_stage(lease, &lookup)),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    ));
    assert_eq!(lookup.calls.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let absent = enrolled_claim();
    stage(&absent);
    cancel_stage(&absent);
    let lease = absent
        .store
        .clone()
        .acquire_stage(absent.record.id())
        .unwrap();
    let lookup = ReceiptLookup::absent();
    assert!(matches!(
        ready(resolve_terminal_stage(lease, &lookup)),
        Ok(StageResolution::NoReceiver(_))
    ));
    assert_eq!(lookup.calls.load(Ordering::SeqCst), 1);
    assert!(absent
        .store
        .read_pairing(absent.record.id())
        .unwrap()
        .cleanup_pending());
}

#[test]
fn no_receiver_proof_refuses_other_registry_and_retains_original_completion() {
    let original = enrolled_claim();
    stage(&original);
    cancel_stage(&original);
    let lease = original
        .store
        .clone()
        .acquire_stage(original.record.id())
        .unwrap();
    let foreign = enrolled_claim();
    stage(&foreign);
    cancel_stage(&foreign);
    let foreign_lease = foreign
        .store
        .clone()
        .acquire_stage(foreign.record.id())
        .unwrap();
    assert_eq!(lease.record().id(), foreign_lease.record().id());
    assert_eq!(lease.record().intent(), foreign_lease.record().intent());
    assert_eq!(
        lease.record().claim_binding(),
        foreign_lease.record().claim_binding()
    );
    assert_eq!(
        lease.record().stage_binding(),
        foreign_lease.record().stage_binding()
    );
    let lookup = ReceiptLookup::absent();
    let StageResolution::NoReceiver(foreign_proof) =
        ready(resolve_terminal_stage(foreign_lease, &lookup)).unwrap()
    else {
        panic!("original absent proof expected")
    };
    let original_path = original.directory.path().join("native/credentials.v1.json");
    let foreign_path = foreign.directory.path().join("native/credentials.v1.json");
    let original_before = std::fs::read(&original_path).unwrap();
    let foreign_before = std::fs::read(&foreign_path).unwrap();
    assert!(matches!(
        original.store.finish_no_receiver(&foreign_proof, &Time),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    ));
    assert_eq!(std::fs::read(&original_path).unwrap(), original_before);
    assert_eq!(std::fs::read(&foreign_path).unwrap(), foreign_before);
    assert!(matches!(
        foreign.store.clone().acquire_stage(foreign.record.id()),
        Err(PairingStoreError::StageOccupied)
    ));
    drop(lease);
    let lease = original
        .store
        .clone()
        .acquire_stage(original.record.id())
        .unwrap();
    let StageResolution::NoReceiver(proof) = ready(resolve_terminal_stage(lease, &lookup)).unwrap()
    else {
        panic!("original absent proof expected")
    };
    let cleaned = original.store.finish_no_receiver(&proof, &Time).unwrap();
    assert!(!cleaned.cleanup_pending());
    assert_eq!(cleaned.terminal().unwrap().0, TerminalCause::Cancelled);
    let completed_bytes = std::fs::read(&original_path).unwrap();
    assert_eq!(
        original.store.finish_no_receiver(&proof, &Time).unwrap(),
        cleaned
    );
    assert_eq!(std::fs::read(&original_path).unwrap(), completed_bytes);
    drop(original.store);
    assert!(matches!(
        open_store(&original_path),
        Err(LocalStoreError::Locked)
    ));
    drop(proof);
    let reopened = open_store(&original_path).unwrap();
    assert_eq!(
        reopened.read_pairing(original.record.id()).unwrap(),
        cleaned
    );
    assert_eq!(std::fs::read(&foreign_path).unwrap(), foreign_before);
    drop(foreign_proof);
}

#[test]
fn no_receiver_completion_refuses_receiver_arriving_after_absent_proof() {
    let fixture = enrolled_claim();
    stage(&fixture);
    cancel_stage(&fixture);
    let prior = fixture.store.read_pairing(fixture.record.id()).unwrap();
    let lease = fixture
        .store
        .clone()
        .acquire_stage(fixture.record.id())
        .unwrap();
    let lookup = ReceiptLookup::absent();
    let StageResolution::NoReceiver(proof) = ready(resolve_terminal_stage(lease, &lookup)).unwrap()
    else {
        panic!("original absent proof expected")
    };
    assert_eq!(lookup.calls.load(Ordering::SeqCst), 1);
    let outcome = ReceiverOutcome::new(
        CredentialId::new("native-device").unwrap(),
        AttemptId::new([9; 16]),
        ResourceId::new("late-receiver").unwrap(),
        1,
        1,
    )
    .unwrap();
    let remembered = fixture
        .store
        .remember_receiver(fixture.record.id(), &outcome, &Time)
        .unwrap();
    assert_eq!(remembered.terminal(), prior.terminal());
    assert!(remembered.cleanup_pending());
    assert_eq!(remembered.receiver_binding(), Some((outcome.receiver(), 1)));
    assert!(matches!(
        fixture.store.clone().acquire_stage(fixture.record.id()),
        Err(PairingStoreError::StageOccupied)
    ));
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        fixture.store.finish_no_receiver(&proof, &Time),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);

    let fenced = ReceiverOutcome::new(
        outcome.credential().clone(),
        outcome.request(),
        outcome.receiver().clone(),
        outcome.epoch() + 1,
        outcome.generation(),
    )
    .unwrap();
    let cleaned = fixture
        .store
        .finish_cleanup(fixture.record.id(), &fenced, &Time)
        .unwrap();
    assert_eq!(cleaned.terminal(), prior.terminal());
    assert!(!cleaned.cleanup_pending());
    assert_eq!(cleaned.receiver_binding(), Some((fenced.receiver(), 2)));
    let cleaned_bytes = std::fs::read(&path).unwrap();
    assert_eq!(
        fixture.store.finish_no_receiver(&proof, &Time),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    );
    assert_eq!(std::fs::read(&path).unwrap(), cleaned_bytes);
    let id = fixture.record.id();
    let Fixture {
        directory, store, ..
    } = fixture;
    drop(store);
    assert!(matches!(
        open_store(path.clone()),
        Err(LocalStoreError::Locked)
    ));
    drop(proof);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(id).unwrap(), cleaned);
    drop(reopened);
    drop(directory);

    let absent = enrolled_claim();
    stage(&absent);
    cancel_stage(&absent);
    let original = absent.store.read_pairing(absent.record.id()).unwrap();
    let lease = absent
        .store
        .clone()
        .acquire_stage(absent.record.id())
        .unwrap();
    let StageResolution::NoReceiver(proof) =
        ready(resolve_terminal_stage(lease, &ReceiptLookup::absent())).unwrap()
    else {
        panic!("original absent proof expected")
    };
    let completed = absent.store.finish_no_receiver(&proof, &Time).unwrap();
    assert!(!completed.cleanup_pending());
    assert!(completed.receiver_binding().is_none());
    assert_eq!(completed.terminal(), original.terminal());
}

#[test]
fn absent_lookup_and_caller_loss_retain_original_stage_without_invented_effect() {
    let fixture = enrolled_claim();
    stage(&fixture);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let staged_bytes = std::fs::read(&path).unwrap();
    let premature = ReceiptLookup::absent();
    let lease = fixture
        .store
        .clone()
        .acquire_stage(fixture.record.id())
        .unwrap();
    assert!(matches!(
        ready(resolve_terminal_stage(lease, &premature)),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    ));
    assert_eq!(premature.calls.load(Ordering::SeqCst), 0);
    assert_eq!(std::fs::read(&path).unwrap(), staged_bytes);
    cancel_stage(&fixture);
    let before = std::fs::read(&path).unwrap();
    let mut unavailable = ReceiptLookup::absent();
    unavailable.unavailable = true;
    let lease = fixture
        .store
        .clone()
        .acquire_stage(fixture.record.id())
        .unwrap();
    assert!(matches!(
        ready(resolve_terminal_stage(lease, &unavailable)),
        Err(PairingStoreError::Unavailable)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let mut pending = ReceiptLookup::absent();
    pending.pending = true;
    let lease = fixture
        .store
        .clone()
        .acquire_stage(fixture.record.id())
        .unwrap();
    let mut caller = Box::pin(resolve_terminal_stage(lease, &pending));
    let waker = Waker::noop();
    assert!(matches!(
        caller.as_mut().poll(&mut Context::from_waker(waker)),
        Poll::Pending
    ));
    drop(caller);
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert!(matches!(
        fixture.store.clone().acquire_stage(fixture.record.id()),
        Err(PairingStoreError::StageOccupied)
    ));
    assert!(matches!(open_store(&path), Err(LocalStoreError::Locked)));
    let id = fixture.record.id();
    drop(fixture.store);
    assert!(
        matches!(open_store(&path), Err(LocalStoreError::Locked)),
        "lookup's original Arc retains actual registry lock"
    );
    drop(pending.held.lock().unwrap().take());
    let reopened = Arc::new(open_store(&path).unwrap());
    assert!(reopened.read_pairing(id).unwrap().cleanup_pending());
    let absent = ReceiptLookup::absent();
    let lease = reopened.clone().acquire_stage(id).unwrap();
    let StageResolution::NoReceiver(proof) = ready(resolve_terminal_stage(lease, &absent)).unwrap()
    else {
        panic!("exact absence expected")
    };
    let cleaned = reopened.finish_no_receiver(&proof, &Time).unwrap();
    assert!(!cleaned.cleanup_pending());
    assert_eq!(cleaned.terminal().unwrap().0, TerminalCause::Cancelled);
    assert_eq!(cleaned.stage_binding().unwrap().0.as_str(), "native-device");
    assert!(matches!(
        reopened.clone().acquire_stage(id),
        Err(PairingStoreError::Domain(_))
    ));
}

struct CurrentAccess {
    store: Arc<LocalCredentialStore>,
    stale: bool,
    unavailable: bool,
}
impl AccessReader for CurrentAccess {
    fn read<'a>(&'a self, credential: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        Box::pin(async move {
            if self.unavailable {
                return Err(AccessError::Unavailable);
            }
            let mut snapshot = AccessReader::read(self.store.as_ref(), credential).await?;
            if self.stale {
                snapshot.revision = 0;
            }
            Ok(snapshot)
        })
    }
}
struct CurrentPolicy {
    cedar: CedarPolicyEvaluator,
    blocked: Option<&'static str>,
    unavailable: bool,
    actions: Mutex<Vec<String>>,
}
impl PolicyEvaluator for CurrentPolicy {
    fn evaluate(
        &self,
        context: &AuthContext,
        action: &Action,
        resource: &Resource,
        snapshot: &AccessSnapshot,
    ) -> Result<Decision, AccessError> {
        self.actions
            .lock()
            .unwrap()
            .push(action.as_str().to_owned());
        let actual = PolicyEvaluator::evaluate(&self.cedar, context, action, resource, snapshot)?;
        if self.blocked == Some(action.as_str()) {
            if self.unavailable {
                Err(AccessError::Unavailable)
            } else {
                Ok(Decision::Deny)
            }
        } else {
            Ok(actual)
        }
    }
}
struct ExpiringProof(CredentialId);
impl CredentialVerifier for ExpiringProof {
    fn verify<'a>(
        &'a self,
        _evidence: &'a CredentialEvidence,
        _audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            Ok(VerifiedCredential {
                credential_id: self.0.clone(),
                expires_at: Some(111),
            })
        })
    }
}
#[test]
fn pairing_admission_current_access_policy_revision_and_deadline_neighbors() {
    let fixture = enrolled_claim();
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    let policy = CedarPolicyEvaluator::new().unwrap();
    for unavailable in [false, true] {
        let access = CurrentAccess {
            store: fixture.store.clone(),
            stale: !unavailable,
            unavailable,
        };
        assert!(matches!(
            ready(
                AuthorizePairing {
                    access: &access,
                    policy: &policy,
                    clock: &Time
                }
                .execute(
                    &fixture.session,
                    fixture.record.intent(),
                    &fixture.gateway
                )
            ),
            Err(AccessError::StaleRevision | AccessError::Unavailable)
        ));
    }
    for action in ["credential.manage", "conversation.read"] {
        for unavailable in [false, true] {
            let policy = CurrentPolicy {
                cedar: CedarPolicyEvaluator::new().unwrap(),
                blocked: Some(action),
                unavailable,
                actions: Mutex::new(Vec::new()),
            };
            assert!(matches!(
                ready(
                    AuthorizePairing {
                        access: fixture.store.as_ref(),
                        policy: &policy,
                        clock: &Time
                    }
                    .execute(
                        &fixture.session,
                        fixture.record.intent(),
                        &fixture.gateway
                    )
                ),
                Err(AccessError::InvalidCredential | AccessError::Unavailable)
            ));
            let expected: &[&str] = if action == "credential.manage" {
                &["credential.manage"]
            } else {
                &["credential.manage", "conversation.read"]
            };
            let actions = policy.actions.lock().unwrap();
            assert_eq!(
                actions.iter().map(String::as_str).collect::<Vec<_>>(),
                expected
            );
        }
    }
    for selector in 0..5 {
        let intent = ConsentIntent::new(
            ConsentIntentId::new([91; 16]),
            1,
            AudienceId::new(if selector == 0 {
                "foreign-gateway"
            } else {
                "gateway-1"
            })
            .unwrap(),
            PrincipalId::new(if selector == 1 {
                "foreign-owner"
            } else {
                "owner"
            })
            .unwrap(),
            MembershipId::new(if selector == 2 {
                "foreign-member"
            } else {
                "owner-membership"
            })
            .unwrap(),
            Resource::new(
                OrganizationId::new(if selector == 3 {
                    "foreign-org"
                } else {
                    "org-1"
                })
                .unwrap(),
                ResourceId::new(if selector == 4 {
                    "foreign-resource"
                } else {
                    "gateway-1"
                })
                .unwrap(),
            ),
        )
        .unwrap();
        assert!(
            matches!(
                ready(
                    AuthorizePairing {
                        access: fixture.store.as_ref(),
                        policy: &policy,
                        clock: &Time
                    }
                    .execute(&fixture.session, &intent, &fixture.gateway)
                ),
                Err(AccessError::IdentityMismatch)
            ),
            "selector {selector}"
        );
    }
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let current_access = CurrentAccess {
        store: fixture.store.clone(),
        stale: false,
        unavailable: false,
    };
    let current_policy = CurrentPolicy {
        cedar: CedarPolicyEvaluator::new().unwrap(),
        blocked: None,
        unavailable: false,
        actions: Mutex::new(Vec::new()),
    };
    ready(
        AuthorizePairing {
            access: &current_access,
            policy: &current_policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    assert_eq!(
        &*current_policy.actions.lock().unwrap(),
        &["credential.manage", "conversation.read"]
    );
    let new_intent = ConsentIntent::new(
        ConsentIntentId::new([92; 16]),
        1,
        fixture.record.intent().audience().clone(),
        fixture.record.intent().owner().clone(),
        fixture.record.intent().membership().clone(),
        fixture.gateway.clone(),
    )
    .unwrap();
    let record = PairingRecord::new(
        InvitationId::new([93; 16]),
        new_intent,
        110_001,
        PairingPolicy::initial(),
    )
    .unwrap();
    let stale = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, record.intent(), &fixture.gateway),
    )
    .unwrap();
    fixture
        .store
        .issue_sync(ordinary_issue("revision-advance"))
        .unwrap();
    let changed = std::fs::read(&path).unwrap();
    assert!(matches!(
        ready(fixture.store.create_pairing(&record, &stale, &Time)),
        Err(PairingStoreError::StaleRevision)
    ));
    assert_eq!(std::fs::read(&path).unwrap(), changed);
    let proof = ExpiringProof(fixture.session.context().credential_id().clone());
    let evidence = CredentialEvidence::new(b"injected-verified-proof".to_vec()).unwrap();
    let short_session = ready(
        AuthenticateSession {
            verifier: &proof,
            access: fixture.store.as_ref(),
            clock: &Time,
        }
        .execute(&evidence, record.intent().audience()),
    )
    .unwrap();
    let admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&short_session, record.intent(), &fixture.gateway),
    )
    .unwrap();
    assert!(matches!(
        ready(
            fixture
                .store
                .create_pairing(&record, &admission, &At(111_000))
        ),
        Err(PairingStoreError::Domain(PairingError::Expired))
    ));
    assert_eq!(std::fs::read(&path).unwrap(), changed);
    assert_eq!(
        ready(fixture.store.create_pairing(&record, &admission, &Time)).unwrap(),
        record
    );
}

#[test]
fn reopen_refuses_duplicate_staged_identity_and_unactivated_ordinary_issuance() {
    let fixture = enrolled_claim();
    stage(&fixture);
    fixture
        .store
        .issue_sync(ordinary_issue("ordinary-reader"))
        .unwrap();
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let original: Registry = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
    drop(fixture.store);
    for duplicate in [false, true] {
        let mut changed = original.clone();
        if duplicate {
            // Both histories replay individually, and neither owns Available.
            let mut other = serde_json::to_value(&changed.pairings[0]).unwrap();
            other["invitation"] = serde_json::json!(vec![2; 16]);
            other["consent"] = serde_json::json!(vec![4; 16]);
            let other: super::StoredPairing = serde_json::from_value(other).unwrap();
            changed.pairings.push(other);
        } else {
            // The ordinary bearer and its canonical transition/receipt remain
            // internally correlated; the retained Stage reserves this same ID.
            changed
                .credentials
                .iter_mut()
                .find(|entry| entry.metadata.id == "ordinary-reader")
                .unwrap()
                .metadata
                .id = "native-device".into();
            for transition in &mut changed.transitions {
                if transition.credential_id == "ordinary-reader" {
                    transition.credential_id = "native-device".into();
                }
            }
            for receipt in &mut changed.issue_receipts {
                if receipt.credential_id == "ordinary-reader" {
                    receipt.credential_id = "native-device".into();
                }
            }
        }
        let bytes = serde_json::to_vec(&changed).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(
            matches!(
                open_store(&path),
                Err(LocalStoreError::InvalidRegistry { .. })
            ),
            "duplicate {duplicate}"
        );
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(&path, serde_json::to_vec(&original).unwrap()).unwrap();
    let reopened = open_store(&path).unwrap();
    assert_eq!(
        reopened.read_pairing(fixture.record.id()).unwrap().phase(),
        PairingPhase::Staging
    );
}

#[test]
fn public_reopen_refuses_conflicting_credential_metadata() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let original_bytes = std::fs::read(&path).unwrap();
    let original: Registry = serde_json::from_slice(&original_bytes).unwrap();
    drop(fixture.store);
    for field in 0..11 {
        let mut changed = original.clone();
        let entry = changed
            .credentials
            .iter_mut()
            .find(|entry| entry.metadata.id == "native-device")
            .unwrap();
        match field {
            0 => entry.metadata.id = "another-credential".into(),
            1 => entry.metadata.principal_id = "another-principal".into(),
            2 => {
                entry.metadata.organization_id = "another-organization".into();
                entry.metadata.grants[0].resource.organization_id = "another-organization".into();
            }
            3 => entry.metadata.audience_id = "another-audience".into(),
            4 => entry.metadata.expires_at = Some(entry.metadata.issued_at + 100),
            5 => entry.metadata.grants[0].action = "conversation.write".into(),
            6 => entry.metadata.grants[0].resource.id = "another-resource".into(),
            7 => entry.metadata.grants.clear(),
            8 => entry.metadata.grants.push(entry.metadata.grants[0].clone()),
            9 => entry.metadata.issued_at += 1,
            10 => {
                let StoredProof::Device(DeviceProofBinding::DevicePairing { generation, .. }) =
                    &mut entry.verifier
                else {
                    panic!("device issuance")
                };
                *generation = 2;
            }
            _ => unreachable!(),
        }
        assert_public_registry_refusal(&path, &changed);
    }
    std::fs::write(&path, &original_bytes).unwrap();
    let reopened = open_store(&path).unwrap();
    assert_eq!(
        reopened.read_pairing(fixture.record.id()).unwrap().phase(),
        PairingPhase::Active
    );
}

#[test]
fn real_device_verifier_refuses_other_tls_key_audience_and_ordinary_bearer() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let IssueCredentialOutcome::Issued {
        evidence: ordinary_secret,
        ..
    } = fixture
        .store
        .issue_sync(ordinary_issue("ordinary-reader"))
        .unwrap()
    else {
        panic!("fresh issuance")
    };
    let evidence = CredentialEvidence::new(b"native-device".to_vec()).unwrap();
    let audience = AudienceId::new("gateway-1").unwrap();
    let verifier = fixture
        .store
        .device_verifier(fixture.channel.device_proof());
    assert!(ready(verifier.verify(&evidence, &audience)).is_ok());
    assert_eq!(
        ready(verifier.verify(&evidence, &AudienceId::new("foreign-gateway").unwrap()))
            .unwrap_err(),
        AccessError::InvalidCredential
    );
    let ordinary_id = CredentialEvidence::new(b"ordinary-reader".to_vec()).unwrap();
    assert_eq!(
        ready(verifier.verify(&ordinary_id, &audience)).unwrap_err(),
        AccessError::InvalidCredential
    );
    assert!(ready(fixture.store.verify(&ordinary_secret, &audience)).is_ok());
    let gateway =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([10; 32]))).unwrap();
    let other_device =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([42; 32]))).unwrap();
    let (a, b) = UnixStream::pair().unwrap();
    for stream in [&a, &b] {
        stream
            .set_read_timeout(Some(Duration::from_secs(30)))
            .unwrap();
        stream
            .set_write_timeout(Some(Duration::from_secs(30)))
            .unwrap();
    }
    let server = thread::spawn(move || NativeTransport::accept(a, &gateway).unwrap());
    let _peer = NativeTransport::connect(b, &other_device, GatewayTrust::ManualBootstrap).unwrap();
    let channel = server.join().unwrap();
    assert_ne!(
        channel.device_proof().key(),
        fixture.channel.device_proof().key()
    );
    let other_verifier = fixture.store.device_verifier(channel.device_proof());
    assert_eq!(
        ready(other_verifier.verify(&evidence, &audience)).unwrap_err(),
        AccessError::InvalidCredential
    );
    assert!(ready(verifier.verify(&evidence, &audience)).is_ok());
}

#[test]
fn public_reopen_refuses_malformed_original_issuance() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let original_bytes = std::fs::read(&path).unwrap();
    let original: Registry = serde_json::from_slice(&original_bytes).unwrap();
    drop(fixture.store);
    for shape in 0..3 {
        let mut changed = original.clone();
        let issued = changed
            .transitions
            .iter_mut()
            .find(|entry| entry.credential_id == "native-device")
            .unwrap();
        match shape {
            0 => issued.before = Some(issued.after),
            1 => issued.after.revoked_at = Some(issued.at),
            2 => issued.after.issued_at += 1,
            _ => unreachable!(),
        }
        assert_public_registry_refusal(&path, &changed);
    }
    std::fs::write(&path, &original_bytes).unwrap();
    let reopened = open_store(&path).unwrap();
    assert_eq!(
        reopened.read_pairing(fixture.record.id()).unwrap().phase(),
        PairingPhase::Active
    );
}

#[test]
fn receiver_outcome_refuses_each_zero_bound_and_accepts_positive_neighbor() {
    let credential = CredentialId::new("native-device").unwrap();
    let request = AttemptId::new([9; 16]);
    let receiver = ResourceId::new("receiver").unwrap();
    for (epoch, generation) in [(0, 1), (1, 0)] {
        assert_eq!(
            ReceiverOutcome::new(
                credential.clone(),
                request,
                receiver.clone(),
                epoch,
                generation
            ),
            Err(PairingError::Invalid)
        );
    }
    let accepted =
        ReceiverOutcome::new(credential.clone(), request, receiver.clone(), 1, 1).unwrap();
    assert_eq!(accepted.credential(), &credential);
    assert_eq!(accepted.request(), request);
    assert_eq!(accepted.receiver(), &receiver);
    assert_eq!(accepted.epoch(), 1);
    assert_eq!(accepted.generation(), 1);
}

#[test]
fn authorized_pairing_commands_refuse_otherwise_valid_foreign_intent() {
    let fixture = enrolled_claim();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let authorize = AuthorizePairing {
        access: fixture.store.as_ref(),
        policy: &policy,
        clock: &Time,
    };
    let original = fixture.record.intent();
    let foreign = ConsentIntent::new(
        ConsentIntentId::new([91; 16]),
        original.generation(),
        original.audience().clone(),
        original.owner().clone(),
        original.membership().clone(),
        original.resource().clone(),
    )
    .unwrap();
    let admission_for = |intent: &ConsentIntent| {
        ready(authorize.execute(&fixture.session, intent, &fixture.gateway)).unwrap()
    };
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let key = fixture.channel.device_proof().key();
    let before = std::fs::read(&path).unwrap();
    let admitted = admission_for(&foreign);
    assert_eq!(admitted.intent(), &foreign);
    assert_eq!(
        ready(fixture.store.decide_pairing(
            fixture.record.id(),
            OwnerDecision::Approve(key),
            &admitted,
            &Time,
        )),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let approved = ready(fixture.store.decide_pairing(
        fixture.record.id(),
        OwnerDecision::Approve(key),
        &admission_for(original),
        &Time,
    ))
    .unwrap();
    assert_eq!(approved.phase(), PairingPhase::Approved);
    assert_eq!(approved.intent(), original);

    let credential = CredentialId::new("native-device").unwrap();
    let request = AttemptId::new([9; 16]);
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        fixture.store.stage_pairing(
            fixture.record.id(),
            credential.clone(),
            request,
            &admission_for(&foreign),
            &Time,
        ),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let staged = fixture
        .store
        .stage_pairing(
            fixture.record.id(),
            credential.clone(),
            request,
            &admission_for(original),
            &Time,
        )
        .unwrap();
    assert_eq!(staged.phase(), PairingPhase::Staging);
    assert_eq!(staged.stage_binding(), Some((&credential, request)));
    let outcome = ReceiverOutcome::new(
        credential.clone(),
        request,
        ResourceId::new("receiver").unwrap(),
        1,
        original.generation(),
    )
    .unwrap();
    fixture
        .store
        .remember_receiver(fixture.record.id(), &outcome, &Time)
        .unwrap();
    let before = std::fs::read(&path).unwrap();
    assert_eq!(
        fixture
            .store
            .publish_pairing(fixture.record.id(), &admission_for(&foreign), &Time),
        Err(PairingStoreError::Domain(PairingError::Conflict))
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    let active = fixture
        .store
        .publish_pairing(fixture.record.id(), &admission_for(original), &Time)
        .unwrap();
    assert_eq!(active.phase(), PairingPhase::Active);
    assert_eq!(active.intent(), original);
    assert_eq!(active.credential(), Some(&credential));
    assert_eq!(
        active.receiver_binding(),
        Some((outcome.receiver(), outcome.epoch()))
    );
    let id = fixture.record.id();
    let Fixture {
        directory, store, ..
    } = fixture;
    drop(store);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(id).unwrap(), active);
    drop(reopened);
    drop(directory);
}

#[test]
fn stage_retry_checks_current_issuer_time_before_existing_result() {
    let fixture = enrolled_claim();
    stage(&fixture);
    let policy = CedarPolicyEvaluator::new().unwrap();
    let admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    let prior = fixture.store.read_pairing(fixture.record.id()).unwrap();
    let (credential, request) = prior.stage_binding().unwrap();
    assert_eq!(
        fixture.store.stage_pairing(
            prior.id(),
            credential.clone(),
            request,
            &admission,
            &At(99_999),
        ),
        Err(PairingStoreError::Domain(PairingError::Invalid))
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(
        fixture
            .store
            .stage_pairing(prior.id(), credential.clone(), request, &admission, &Time)
            .unwrap(),
        prior
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
}

#[test]
fn equal_revision_alternate_store_admission_requires_original_issuer() {
    alternate_store_requires_original_issuer(true);
}
#[test]
fn equal_revision_alternate_store_admission_requires_missing_original_issuer() {
    alternate_store_requires_original_issuer(false);
}
fn alternate_store_requires_original_issuer(original_has_issuer: bool) {
    let fixture = enrolled_claim();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let original_admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    let approved = ready(fixture.store.decide_pairing(
        fixture.record.id(),
        OwnerDecision::Approve(fixture.channel.device_proof().key()),
        &original_admission,
        &Time,
    ))
    .unwrap();
    assert_eq!(approved.phase(), PairingPhase::Approved);
    let issuer_id = CredentialId::new("alternate-authorizer").unwrap();
    let target_identity = if original_has_issuer {
        issuer_id.as_str()
    } else {
        "target-revision"
    };
    fixture
        .store
        .issue_sync(ordinary_issue(target_identity))
        .unwrap();
    let original_snapshot = ready(AccessReader::read(
        fixture.store.as_ref(),
        fixture.session.context().credential_id(),
    ))
    .unwrap();
    if original_has_issuer {
        let resolved = ready(AccessReader::read(fixture.store.as_ref(), &issuer_id)).unwrap();
        assert_eq!(resolved.credential.principal_id().as_str(), "reader");
        assert_eq!(
            resolved.credential.organization_id(),
            fixture.record.intent().resource().organization_id()
        );
        assert_eq!(
            resolved.credential.audience_id(),
            fixture.record.intent().audience()
        );
        assert!(resolved.credential.is_valid_at(110));
    } else {
        assert!(matches!(
            ready(AccessReader::read(fixture.store.as_ref(), &issuer_id)),
            Err(AccessError::InvalidCredential)
        ));
    }
    let alternate_directory = tempfile::tempdir().unwrap();
    let alternate = open_store(
        alternate_directory
            .path()
            .join("native/credentials.v1.json"),
    )
    .unwrap();
    let mut request = bootstrap();
    request.credential_id = issuer_id.as_str().to_owned();
    request.grants.push(grant("org-1", "conversation.read"));
    let issued = alternate.bootstrap(request).unwrap();
    let mut alternate_snapshot = ready(AccessReader::read(&alternate, &issuer_id)).unwrap();
    while alternate_snapshot.revision < original_snapshot.revision {
        let prior_revision = alternate_snapshot.revision;
        alternate
            .issue_sync(ordinary_issue(&format!(
                "alternate-revision-{prior_revision}"
            )))
            .unwrap();
        alternate_snapshot = ready(AccessReader::read(&alternate, &issuer_id)).unwrap();
        assert_eq!(alternate_snapshot.revision, prior_revision + 1);
    }
    assert_eq!(alternate_snapshot.revision, original_snapshot.revision);
    assert_eq!(
        alternate_snapshot.credential.principal_id(),
        fixture.record.intent().owner()
    );
    let session = ready(
        AuthenticateSession {
            verifier: &alternate,
            access: &alternate,
            clock: &Time,
        }
        .execute(&issued.evidence, fixture.record.intent().audience()),
    )
    .unwrap();
    let admission = ready(
        AuthorizePairing {
            access: &alternate,
            policy: &policy,
            clock: &Time,
        }
        .execute(&session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    assert_eq!(admission.intent(), fixture.record.intent());
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let original_bytes = std::fs::read(&path).unwrap();
    let alternate_path = alternate_directory
        .path()
        .join("native/credentials.v1.json");
    let alternate_bytes = std::fs::read(&alternate_path).unwrap();
    let credential = CredentialId::new("native-device").unwrap();
    let correlation = AttemptId::new([9; 16]);
    assert_eq!(
        fixture.store.stage_pairing(
            fixture.record.id(),
            credential.clone(),
            correlation,
            &admission,
            &Time
        ),
        Err(if original_has_issuer {
            PairingStoreError::Domain(PairingError::Invalid)
        } else {
            PairingStoreError::Unavailable
        })
    );
    assert_eq!(std::fs::read(&path).unwrap(), original_bytes);
    assert_eq!(std::fs::read(&alternate_path).unwrap(), alternate_bytes);
    let original_admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    let staged = fixture
        .store
        .stage_pairing(
            fixture.record.id(),
            credential.clone(),
            correlation,
            &original_admission,
            &Time,
        )
        .unwrap();
    assert_eq!(staged.phase(), PairingPhase::Staging);
    assert_eq!(staged.stage_binding(), Some((&credential, correlation)));
    assert_eq!(staged.intent(), fixture.record.intent());
    assert_eq!(std::fs::read(&alternate_path).unwrap(), alternate_bytes);
    let id = fixture.record.id();
    let Fixture {
        directory, store, ..
    } = fixture;
    drop(store);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(id).unwrap(), staged);
    drop(reopened);
    drop(directory);
}

#[test]
fn equal_revision_alternate_gateway_admission_requires_current_issuer_audience() {
    let fixture = enrolled_claim();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let original_snapshot = ready(AccessReader::read(
        fixture.store.as_ref(),
        fixture.session.context().credential_id(),
    ))
    .unwrap();
    let alternate_directory = tempfile::tempdir().unwrap();
    let alternate = open_store(
        alternate_directory
            .path()
            .join("native/credentials.v1.json"),
    )
    .unwrap();
    let mut request = bootstrap();
    request.gateway_id = "gateway-2".into();
    request.grants[0].resource.id = "gateway-2".into();
    let mut read = grant("org-1", "conversation.read");
    read.resource.id = "gateway-2".into();
    request.grants.push(read);
    let issued = alternate.bootstrap(request).unwrap();
    let issuer = fixture.session.context().credential_id();
    let mut alternate_snapshot = ready(AccessReader::read(&alternate, issuer)).unwrap();
    while alternate_snapshot.revision < original_snapshot.revision {
        let prior_revision = alternate_snapshot.revision;
        let mut issue = ordinary_issue(&format!("alternate-gateway-revision-{prior_revision}"));
        issue.audience_id = "gateway-2".into();
        issue.grants[0].resource.id = "gateway-2".into();
        alternate.issue_sync(issue).unwrap();
        alternate_snapshot = ready(AccessReader::read(&alternate, issuer)).unwrap();
        assert_eq!(alternate_snapshot.revision, prior_revision + 1);
    }
    assert_eq!(alternate_snapshot.revision, original_snapshot.revision);
    assert_eq!(
        alternate_snapshot.credential.id(),
        original_snapshot.credential.id()
    );
    assert_eq!(
        alternate_snapshot.credential.principal_id(),
        original_snapshot.credential.principal_id()
    );
    assert_eq!(
        alternate_snapshot.credential.organization_id(),
        original_snapshot.credential.organization_id()
    );
    let gateway = Resource::new(
        OrganizationId::new("org-1").unwrap(),
        ResourceId::new("gateway-2").unwrap(),
    );
    let intent = ConsentIntent::new(
        ConsentIntentId::new([94; 16]),
        1,
        AudienceId::new("gateway-2").unwrap(),
        fixture.record.intent().owner().clone(),
        fixture.record.intent().membership().clone(),
        gateway.clone(),
    )
    .unwrap();
    let session = ready(
        AuthenticateSession {
            verifier: &alternate,
            access: &alternate,
            clock: &Time,
        }
        .execute(&issued.evidence, intent.audience()),
    )
    .unwrap();
    let admission = ready(
        AuthorizePairing {
            access: &alternate,
            policy: &policy,
            clock: &Time,
        }
        .execute(&session, &intent, &gateway),
    )
    .unwrap();
    let record = PairingRecord::new(
        InvitationId::new([95; 16]),
        intent,
        110_001,
        PairingPolicy::initial(),
    )
    .unwrap();
    assert_eq!(record.intent(), admission.intent());
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let before = std::fs::read(&path).unwrap();
    let alternate_path = alternate_directory
        .path()
        .join("native/credentials.v1.json");
    let alternate_bytes = std::fs::read(&alternate_path).unwrap();
    assert_eq!(
        ready(fixture.store.create_pairing(&record, &admission, &Time)),
        Err(PairingStoreError::Domain(PairingError::Invalid))
    );
    assert_eq!(std::fs::read(&path).unwrap(), before);
    assert_eq!(std::fs::read(&alternate_path).unwrap(), alternate_bytes);
    let original_intent = ConsentIntent::new(
        record.intent().id(),
        record.intent().generation(),
        fixture.record.intent().audience().clone(),
        fixture.record.intent().owner().clone(),
        fixture.record.intent().membership().clone(),
        fixture.gateway.clone(),
    )
    .unwrap();
    let original_record = PairingRecord::new(
        record.id(),
        original_intent,
        110_001,
        PairingPolicy::initial(),
    )
    .unwrap();
    let original_admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, original_record.intent(), &fixture.gateway),
    )
    .unwrap();
    assert_eq!(
        ready(
            fixture
                .store
                .create_pairing(&original_record, &original_admission, &Time)
        )
        .unwrap(),
        original_record
    );
    assert_eq!(std::fs::read(&alternate_path).unwrap(), alternate_bytes);
    let Fixture {
        directory, store, ..
    } = fixture;
    drop(store);
    let reopened = open_store(path).unwrap();
    assert_eq!(
        reopened.read_pairing(original_record.id()).unwrap(),
        original_record
    );
    drop(reopened);
    drop(directory);
}

struct AdmissionAuthority {
    directory: TempDir,
    store: Arc<LocalCredentialStore>,
    evidence: CredentialEvidence,
}
impl AdmissionAuthority {
    fn path(&self) -> PathBuf {
        self.directory.path().join("native/credentials.v1.json")
    }
}
fn admission_authority(request: BootstrapRequest) -> AdmissionAuthority {
    let directory = tempfile::tempdir().unwrap();
    let store = Arc::new(open_store(directory.path().join("native/credentials.v1.json")).unwrap());
    let evidence = store.bootstrap(request).unwrap().evidence;
    AdmissionAuthority {
        directory,
        store,
        evidence,
    }
}
fn owner_admission_authority(member: &str, organization: &str, issuer: &str) -> AdmissionAuthority {
    let mut request = bootstrap();
    request.organization.id = organization.into();
    request.membership.id = member.into();
    request.membership.organization_id = organization.into();
    request.credential_id = issuer.into();
    request.expires_at = None;
    request.grants = vec![
        grant(organization, "credential.manage"),
        grant(organization, "conversation.read"),
    ];
    admission_authority(request)
}
fn restore_admission_authority(
    authority: AdmissionAuthority,
    change: impl FnOnce(&mut serde_json::Value),
) -> AdmissionAuthority {
    let path = authority.path();
    let AdmissionAuthority {
        directory,
        store,
        evidence,
    } = authority;
    let original = std::fs::read(&path).unwrap();
    drop(store);
    let mut bytes: serde_json::Value = serde_json::from_slice(&original).unwrap();
    change(&mut bytes);
    // Exercise the original serialized boundary and full open-time validators,
    // without manufacturing or publishing a private in-memory Registry.
    std::fs::write(&path, serde_json::to_vec(&bytes).unwrap()).unwrap();
    let store = Arc::new(open_store(&path).unwrap());
    AdmissionAuthority {
        directory,
        store,
        evidence,
    }
}
fn with_second_admission_organization(
    authority: AdmissionAuthority,
    member: &str,
    issuer: &str,
) -> AdmissionAuthority {
    let source = owner_admission_authority(member, "org-2", issuer);
    let source_bytes = std::fs::read(source.path()).unwrap();
    let source_value: serde_json::Value = serde_json::from_slice(&source_bytes).unwrap();
    let restored = restore_admission_authority(authority, |target| {
        assert_eq!(target["gatewayId"], source_value["gatewayId"]);
        assert_eq!(target["principals"], source_value["principals"]);
        let revision = target["revision"].as_u64().unwrap() + 1;
        for name in ["organizations", "memberships", "credentials"] {
            target[name]
                .as_array_mut()
                .unwrap()
                .extend(source_value[name].as_array().unwrap().iter().cloned());
        }
        for original in source_value["transitions"].as_array().unwrap() {
            let mut transition = original.clone();
            let sequence = target["transitions"].as_array().unwrap().len() as u64 + 1;
            transition["sequence"] = sequence.into();
            transition["revision"] = revision.into();
            target["transitions"]
                .as_array_mut()
                .unwrap()
                .push(transition);
        }
        target["revision"] = revision.into();
    });
    assert_eq!(std::fs::read(source.path()).unwrap(), source_bytes);
    assert_eq!(restored.store.identity().unwrap().organization_ids.len(), 2);
    restored
}
fn align_admission_revision(authority: &AdmissionAuthority, target: &AdmissionAuthority) {
    let expected = target.store.identity().unwrap().revision;
    let session = ready(
        AuthenticateSession {
            verifier: authority.store.as_ref(),
            access: authority.store.as_ref(),
            clock: &Time,
        }
        .execute(
            &authority.evidence,
            &AudienceId::new(authority.store.gateway_id().unwrap()).unwrap(),
        ),
    )
    .unwrap();
    while authority.store.identity().unwrap().revision < expected {
        let previous = authority.store.identity().unwrap().revision;
        let mut request = ordinary_issue(&format!("align-{previous}"));
        request.issuer_principal_id = session.context().principal_id().as_str().into();
        request.principal.id = format!("aligned-reader-{previous}");
        request.membership.id = format!("aligned-member-{previous}");
        request.membership.principal_id = request.principal.id.clone();
        let organization = authority.store.identity().unwrap().organization_ids[0].clone();
        request.membership.organization_id = organization.clone();
        request.grants = vec![grant(&organization, "conversation.read")];
        authority.store.issue_sync(request).unwrap();
        assert_eq!(authority.store.identity().unwrap().revision, previous + 1);
    }
    assert_eq!(authority.store.identity().unwrap().revision, expected);
}
fn admission_record(owner: &str, member: &str, organization: &str, identity: u8) -> PairingRecord {
    let intent = ConsentIntent::new(
        ConsentIntentId::new([identity; 16]),
        1,
        AudienceId::new("gateway-1").unwrap(),
        PrincipalId::new(owner).unwrap(),
        MembershipId::new(member).unwrap(),
        Resource::new(
            OrganizationId::new(organization).unwrap(),
            ResourceId::new("gateway-1").unwrap(),
        ),
    )
    .unwrap();
    PairingRecord::new(
        InvitationId::new([identity + 1; 16]),
        intent,
        Time.unix_milliseconds(),
        PairingPolicy::initial(),
    )
    .unwrap()
}
fn admit_record(authority: &AdmissionAuthority, record: &PairingRecord) -> PairingAdmission {
    let session = ready(
        AuthenticateSession {
            verifier: authority.store.as_ref(),
            access: authority.store.as_ref(),
            clock: &Time,
        }
        .execute(&authority.evidence, record.intent().audience()),
    )
    .unwrap();
    ready(
        AuthorizePairing {
            access: authority.store.as_ref(),
            policy: &CedarPolicyEvaluator::new().unwrap(),
            clock: &Time,
        }
        .execute(&session, record.intent(), record.intent().resource()),
    )
    .unwrap()
}
fn create_and_reopen_admission(
    authority: AdmissionAuthority,
    record: &PairingRecord,
    admission: &PairingAdmission,
) {
    assert_eq!(
        ready(authority.store.create_pairing(record, admission, &Time)).unwrap(),
        *record
    );
    let path = authority.path();
    let AdmissionAuthority {
        directory, store, ..
    } = authority;
    drop(store);
    let reopened = open_store(path).unwrap();
    assert_eq!(reopened.read_pairing(record.id()).unwrap(), *record);
    drop(reopened);
    drop(directory);
}

#[derive(Clone, Copy)]
enum MembershipSelector {
    Identity,
    Principal,
    Organization,
}
fn check_current_membership(selector: MembershipSelector) {
    let mut target = owner_admission_authority("current-member", "org-1", "current-issuer");
    match selector {
        MembershipSelector::Identity => {}
        MembershipSelector::Principal => {
            let mut request = ordinary_issue("reader-issuer");
            request.membership.id = "alternate-member".into();
            target.store.issue_sync(request).unwrap();
        }
        MembershipSelector::Organization => {
            target = with_second_admission_organization(target, "alternate-member", "other-issuer");
        }
    }
    let authorizer = owner_admission_authority("alternate-member", "org-1", "current-issuer");
    align_admission_revision(&authorizer, &target);
    let record = admission_record("owner", "alternate-member", "org-1", 100);
    let admission = admit_record(&authorizer, &record);
    let current = ready(AccessReader::read(
        target.store.as_ref(),
        &CredentialId::new("current-issuer").unwrap(),
    ))
    .unwrap();
    assert!(current.credential.is_valid_at(Time.unix_seconds()));
    assert_eq!(current.credential.principal_id(), record.intent().owner());
    assert_eq!(
        current.credential.organization_id(),
        record.intent().resource().organization_id()
    );
    assert_eq!(current.credential.audience_id(), record.intent().audience());
    let member = match selector {
        MembershipSelector::Identity => current.membership,
        MembershipSelector::Principal | MembershipSelector::Organization => {
            let credential = match selector {
                MembershipSelector::Principal => "reader-issuer",
                MembershipSelector::Organization => "other-issuer",
                MembershipSelector::Identity => unreachable!(),
            };
            ready(AccessReader::read(
                target.store.as_ref(),
                &CredentialId::new(credential).unwrap(),
            ))
            .unwrap()
            .membership
        }
    };
    assert!(member.is_active());
    assert_eq!(
        member.id() == record.intent().membership(),
        !matches!(selector, MembershipSelector::Identity)
    );
    assert_eq!(
        member.principal_id() == record.intent().owner(),
        !matches!(selector, MembershipSelector::Principal)
    );
    assert_eq!(
        member.organization_id() == record.intent().resource().organization_id(),
        !matches!(selector, MembershipSelector::Organization)
    );
    let target_bytes = std::fs::read(target.path()).unwrap();
    let authorizer_bytes = std::fs::read(authorizer.path()).unwrap();
    assert_eq!(
        ready(target.store.create_pairing(&record, &admission, &Time)),
        Err(PairingStoreError::Domain(PairingError::Invalid))
    );
    assert_eq!(std::fs::read(target.path()).unwrap(), target_bytes);
    assert_eq!(std::fs::read(authorizer.path()).unwrap(), authorizer_bytes);
    let original = admission_record("owner", "current-member", "org-1", 102);
    let original_admission = admit_record(&target, &original);
    create_and_reopen_admission(target, &original, &original_admission);
    assert_eq!(std::fs::read(authorizer.path()).unwrap(), authorizer_bytes);
}
#[test]
fn equal_revision_alternate_admission_requires_current_membership_id() {
    check_current_membership(MembershipSelector::Identity);
}
#[test]
fn equal_revision_alternate_admission_requires_current_membership_principal() {
    check_current_membership(MembershipSelector::Principal);
}
#[test]
fn equal_revision_alternate_admission_requires_current_membership_organization() {
    check_current_membership(MembershipSelector::Organization);
}
#[test]
fn equal_revision_alternate_admission_requires_active_membership() {
    let target = owner_admission_authority("current-member", "org-1", "owner-issuer");
    let mut reader = ordinary_issue("current-issuer");
    reader.membership.id = "alternate-member".into();
    target.store.issue_sync(reader).unwrap();
    let target = restore_admission_authority(target, |bytes| {
        let member = bytes["memberships"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|member| member["id"] == "alternate-member")
            .unwrap();
        member["state"] = serde_json::to_value(MembershipStateDto::Disabled).unwrap();
    });
    let mut request = bootstrap();
    request.principal.id = "reader".into();
    request.principal.kind = PrincipalKindDto::Agent;
    request.membership.id = "alternate-member".into();
    request.membership.principal_id = "reader".into();
    request.credential_id = "current-issuer".into();
    request.expires_at = None;
    request.grants.push(grant("org-1", "conversation.read"));
    let authorizer = admission_authority(request);
    align_admission_revision(&authorizer, &target);
    let record = admission_record("reader", "alternate-member", "org-1", 104);
    let admission = admit_record(&authorizer, &record);
    let current = ready(AccessReader::read(
        target.store.as_ref(),
        &CredentialId::new("current-issuer").unwrap(),
    ))
    .unwrap();
    assert!(current.credential.is_valid_at(Time.unix_seconds()));
    assert_eq!(current.credential.principal_id(), record.intent().owner());
    assert_eq!(
        current.credential.organization_id(),
        record.intent().resource().organization_id()
    );
    assert_eq!(current.credential.audience_id(), record.intent().audience());
    assert_eq!(current.membership.id(), record.intent().membership());
    assert_eq!(current.membership.principal_id(), record.intent().owner());
    assert_eq!(
        current.membership.organization_id(),
        record.intent().resource().organization_id()
    );
    assert!(!current.membership.is_active());
    let target_bytes = std::fs::read(target.path()).unwrap();
    let authorizer_bytes = std::fs::read(authorizer.path()).unwrap();
    assert_eq!(
        ready(target.store.create_pairing(&record, &admission, &Time)),
        Err(PairingStoreError::Domain(PairingError::Invalid))
    );
    assert_eq!(std::fs::read(target.path()).unwrap(), target_bytes);
    assert_eq!(std::fs::read(authorizer.path()).unwrap(), authorizer_bytes);
    let target = restore_admission_authority(target, |bytes| {
        let member = bytes["memberships"]
            .as_array_mut()
            .unwrap()
            .iter_mut()
            .find(|member| member["id"] == "alternate-member")
            .unwrap();
        member["state"] = serde_json::to_value(MembershipStateDto::Active).unwrap();
    });
    // The exact original trusted authorizer/policy decision is retained; only
    // the current commit membership state changes at the restore boundary.
    create_and_reopen_admission(target, &record, &admission);
    assert_eq!(std::fs::read(authorizer.path()).unwrap(), authorizer_bytes);
}
#[test]
fn equal_revision_alternate_admission_requires_current_issuer_organization() {
    let target = with_second_admission_organization(
        owner_admission_authority("current-member", "org-1", "owner-issuer"),
        "other-member",
        "current-issuer",
    );
    let authorizer = owner_admission_authority("current-member", "org-1", "current-issuer");
    align_admission_revision(&authorizer, &target);
    let record = admission_record("owner", "current-member", "org-1", 106);
    let admission = admit_record(&authorizer, &record);
    let resolved = ready(AccessReader::read(
        target.store.as_ref(),
        &CredentialId::new("current-issuer").unwrap(),
    ))
    .unwrap();
    assert!(resolved.credential.is_valid_at(Time.unix_seconds()));
    assert_eq!(resolved.credential.principal_id(), record.intent().owner());
    assert_eq!(
        resolved.credential.audience_id(),
        record.intent().audience()
    );
    assert_ne!(
        resolved.credential.organization_id(),
        record.intent().resource().organization_id()
    );
    let current_member = ready(AccessReader::read(
        target.store.as_ref(),
        &CredentialId::new("owner-issuer").unwrap(),
    ))
    .unwrap()
    .membership;
    assert_eq!(current_member.id(), record.intent().membership());
    assert_eq!(current_member.principal_id(), record.intent().owner());
    assert_eq!(
        current_member.organization_id(),
        record.intent().resource().organization_id()
    );
    assert!(current_member.is_active());
    let target_bytes = std::fs::read(target.path()).unwrap();
    let authorizer_bytes = std::fs::read(authorizer.path()).unwrap();
    assert_eq!(
        ready(target.store.create_pairing(&record, &admission, &Time)),
        Err(PairingStoreError::Domain(PairingError::Invalid))
    );
    assert_eq!(std::fs::read(target.path()).unwrap(), target_bytes);
    assert_eq!(std::fs::read(authorizer.path()).unwrap(), authorizer_bytes);
    let original = admission_record("owner", "current-member", "org-1", 108);
    let original_admission = admit_record(&target, &original);
    create_and_reopen_admission(target, &original, &original_admission);
    assert_eq!(std::fs::read(authorizer.path()).unwrap(), authorizer_bytes);
}

#[test]
fn public_reopen_refuses_invalid_and_foreign_organization() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let original_bytes = std::fs::read(&path).unwrap();
    let original: Registry = serde_json::from_slice(&original_bytes).unwrap();
    drop(fixture.store);
    for foreign in [false, true] {
        let mut changed = original.clone();
        let metadata = &mut changed
            .credentials
            .iter_mut()
            .find(|entry| entry.metadata.id == "native-device")
            .unwrap()
            .metadata;
        metadata.grants[0].resource.organization_id = "foreign-organization".into();
        if foreign {
            metadata.organization_id = "foreign-organization".into();
        }
        assert_public_registry_refusal(&path, &changed);
    }
    std::fs::write(&path, &original_bytes).unwrap();
    let reopened = open_store(&path).unwrap();
    assert_eq!(
        reopened.read_pairing(fixture.record.id()).unwrap().phase(),
        PairingPhase::Active
    );
}

#[test]
fn pairing_binding_requires_original_identity_even_with_another_real_issuance() {
    let original = enrolled_claim();
    publish(&original);
    let other = enrolled_claim();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let authorize = AuthorizePairing {
        access: other.store.as_ref(),
        policy: &policy,
        clock: &Time,
    };
    let admission =
        ready(authorize.execute(&other.session, other.record.intent(), &other.gateway)).unwrap();
    ready(other.store.decide_pairing(
        other.record.id(),
        OwnerDecision::Approve(other.channel.device_proof().key()),
        &admission,
        &Time,
    ))
    .unwrap();
    let admission =
        ready(authorize.execute(&other.session, other.record.intent(), &other.gateway)).unwrap();
    other
        .store
        .stage_pairing(
            other.record.id(),
            CredentialId::new("other-native-device").unwrap(),
            AttemptId::new([9; 16]),
            &admission,
            &Time,
        )
        .unwrap();
    let outcome = ReceiverOutcome::new(
        CredentialId::new("other-native-device").unwrap(),
        AttemptId::new([9; 16]),
        ResourceId::new("receiver").unwrap(),
        1,
        1,
    )
    .unwrap();
    other
        .store
        .remember_receiver(other.record.id(), &outcome, &Time)
        .unwrap();
    let admission =
        ready(authorize.execute(&other.session, other.record.intent(), &other.gateway)).unwrap();
    other
        .store
        .publish_pairing(other.record.id(), &admission, &Time)
        .unwrap();

    let original_path = original.directory.path().join("native/credentials.v1.json");
    let other_path = other.directory.path().join("native/credentials.v1.json");
    let original_bytes = std::fs::read(&original_path).unwrap();
    let other_bytes = std::fs::read(&other_path).unwrap();
    let mut first: Registry = serde_json::from_slice(&original_bytes).unwrap();
    let second: Registry = serde_json::from_slice(&other_bytes).unwrap();
    let other_credential = second
        .credentials
        .iter()
        .find(|entry| entry.metadata.id == "other-native-device")
        .unwrap()
        .clone();
    let original_credential = first
        .credentials
        .iter_mut()
        .find(|entry| entry.metadata.id == "native-device")
        .unwrap();
    *original_credential = other_credential;
    drop(original.store);
    assert_public_registry_refusal(&original_path, &first);
    std::fs::write(&original_path, &original_bytes).unwrap();
    let reopened = open_store(&original_path).unwrap();
    assert_eq!(
        reopened.read_pairing(original.record.id()).unwrap().phase(),
        PairingPhase::Active
    );
    assert_eq!(std::fs::read(&other_path).unwrap(), other_bytes);
}

#[test]
fn public_reopen_refuses_revocation_before_original_issuance() {
    let fixture = enrolled_claim();
    publish(&fixture);
    let path = fixture.directory.path().join("native/credentials.v1.json");
    fixture
        .store
        .revoke_sync(RevokeCredentialRequest {
            request_id: "raw-binding-revoke".into(),
            issuer_principal_id: "owner".into(),
            credential_id: "native-device".into(),
            revoked_at: 110,
        })
        .unwrap();
    let retired_bytes = std::fs::read(&path).unwrap();
    let retired: Registry = serde_json::from_slice(&retired_bytes).unwrap();
    drop(fixture.store);
    let mut malformed = retired.clone();
    malformed
        .credentials
        .iter_mut()
        .find(|entry| entry.metadata.id == "native-device")
        .unwrap()
        .metadata
        .revoked_at = Some(0);
    assert_public_registry_refusal(&path, &malformed);
    std::fs::write(&path, &retired_bytes).unwrap();
    let reopened = open_store(&path).unwrap();
    assert!(matches!(
        reopened.read_pairing(fixture.record.id()).unwrap().phase(),
        PairingPhase::Terminal
    ));
    assert_eq!(std::fs::read(&path).unwrap(), retired_bytes);
}

fn assert_public_registry_refusal(path: &Path, changed: &Registry) {
    let bytes = serde_json::to_vec(changed).unwrap();
    std::fs::write(path, &bytes).unwrap();
    assert!(matches!(
        open_store(path),
        Err(LocalStoreError::InvalidRegistry { .. })
    ));
    assert_eq!(std::fs::read(path).unwrap(), bytes);
}

#[test]
fn public_reopen_validates_original_consent_and_history_bound() {
    let fixture = enrolled_claim();
    let policy = CedarPolicyEvaluator::new().unwrap();
    let admission = ready(
        AuthorizePairing {
            access: fixture.store.as_ref(),
            policy: &policy,
            clock: &Time,
        }
        .execute(&fixture.session, fixture.record.intent(), &fixture.gateway),
    )
    .unwrap();
    ready(fixture.store.decide_pairing(
        fixture.record.id(),
        OwnerDecision::Approve(fixture.channel.device_proof().key()),
        &admission,
        &Time,
    ))
    .unwrap();
    let path = fixture.directory.path().join("native/credentials.v1.json");
    let original_bytes = std::fs::read(&path).unwrap();
    let original: serde_json::Value = serde_json::from_slice(&original_bytes).unwrap();
    drop(fixture.store);
    for field in 0..3 {
        let mut changed = original.clone();
        match field {
            0 => {
                changed["pairings"][0]["history"][2]["actor"]["id"] =
                    serde_json::json!("other-owner")
            }
            1 => changed["pairings"][0]["generation"] = serde_json::json!(2),
            2 => {
                let history = changed["pairings"][0]["history"].as_array_mut().unwrap();
                let first = history[0].clone();
                while history.len() <= 32 {
                    history.push(first.clone());
                }
            }
            _ => unreachable!(),
        }
        let bytes = serde_json::to_vec(&changed).unwrap();
        std::fs::write(&path, &bytes).unwrap();
        assert!(matches!(
            open_store(&path),
            Err(LocalStoreError::InvalidRegistry { .. })
        ));
        assert_eq!(std::fs::read(&path).unwrap(), bytes);
    }
    std::fs::write(&path, &original_bytes).unwrap();
    let reopened = open_store(&path).unwrap();
    assert_eq!(
        reopened.read_pairing(fixture.record.id()).unwrap().phase(),
        PairingPhase::Approved
    );
}
