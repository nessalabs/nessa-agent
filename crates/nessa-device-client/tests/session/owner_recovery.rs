//! Actual original private state, pinned TLS and typed consumer/foreign outcomes.
use super::*;
#[cfg(unix)]
use crate::native_owner_recovery::NativeOwnerAdoptionPhase;
use crate::{
    native_owner_recovery::{NativeOwnerSettlementFailureKind, NativeOwnerStatus},
    test_support::PrivateFixture,
};
use nessa_auth::{
    adapters::pairing::NativeTransport,
    application::pairing::{
        DeviceCredential, OwnerPhoneAdoption, OwnerPhoneAdoptionFailure, OwnerPhoneEnrollmentKind,
        OwnerPhoneEnrollmentStore, OwnerPhoneOriginalCreation, OwnerPhonePrivateOriginal,
        PendingEnrollment, PrivateKeyMaterial,
    },
    domain::{
        pairing::{
            AttemptId, ConsentClass, ConsentIntent, ConsentIntentId, InvitationId,
            OwnerPhoneContext, OwnerPhoneRequest, OwnerPhoneRequestId, OwnerPhoneSource,
            PairingEvent, PairingInitiator, PairingPolicy, PairingRecord,
        },
        AudienceId, MembershipId, OrganizationId, PrincipalId, Resource,
    },
};
use nessa_client_core::pairing::{OwnerPhoneSettlement, OwnerPhoneSettlementFailure};
use nessa_local_storage::create_directory_beneath;
use nessa_protocol::pairing::{
    wire::{self, NativePairingRequest},
    DevicePairingStatus, EnrollmentChannel,
};
use std::{
    fs,
    net::TcpListener,
    sync::atomic::{AtomicUsize, Ordering},
    sync::mpsc,
    thread::{self, JoinHandle},
};
use zeroize::Zeroizing;

fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}
#[derive(Clone, Copy, Debug)]
enum Mode {
    Good,
    WrongKind,
    WrongReportedPublic,
    MissingActualBinding,
    MissingActualLineage,
    ChangedOriginalKey,
    Key(usize),
    Pin(usize),
    Public(usize),
    Credential(usize),
    Receiver(usize),
    PanicAfterPublication,
}
struct Store {
    actual: FilePairingState,
    mode: Mutex<Mode>,
    reads: AtomicUsize,
    adoption: AtomicUsize,
    generic: AtomicUsize,
    held: Mutex<Option<Arc<super::product_tests::fixtures::Held>>>,
    end_failure: Mutex<Option<PrivateStateError>>,
}
impl ClientPendingStore for Store {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.actual.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        let saved = self.actual.load_credential()?;
        let number = self.reads.fetch_add(1, Ordering::SeqCst) + 1;
        let mode = *self.mode.lock().unwrap();
        let Some(saved) = saved else {
            return Ok(None);
        };
        let at = match mode {
            Mode::Key(at)
            | Mode::Pin(at)
            | Mode::Public(at)
            | Mode::Credential(at)
            | Mode::Receiver(at) => at,
            _ => return Ok(Some(saved)),
        };
        if number != at {
            return Ok(Some(saved));
        }
        let credential = saved.credential().clone();
        let receiver = saved.receiver().clone();
        let (key, pin, public) = saved.into_enrollment().into_parts();
        let enrollment = PendingEnrollment::new(
            if matches!(mode, Mode::Key(_)) {
                PrivateKeyMaterial::new(Zeroizing::new([99; 32]))
            } else {
                key
            },
            if matches!(mode, Mode::Pin(_)) {
                identity(99).public_spki()
            } else {
                pin
            },
            if matches!(mode, Mode::Public(_)) {
                public.with_attempt(AttemptId::new([99; 16]))
            } else {
                public
            },
        );
        Ok(Some(DeviceCredential::new(
            enrollment,
            if matches!(mode, Mode::Credential(_)) {
                CredentialId::new("other-issued").unwrap()
            } else {
                credential
            },
            if matches!(mode, Mode::Receiver(_)) {
                ResourceId::new("other-receiver").unwrap()
            } else {
                receiver
            },
        )))
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        public: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.generic.fetch_add(1, Ordering::SeqCst);
        self.actual.save_pending(key, pin, public, expected)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.generic.fetch_add(1, Ordering::SeqCst);
        self.actual.save_credential(credential, receiver, expected)
    }
    fn end_enrollment(&self, expected: PublicIntent) -> Result<(), PrivateStateError> {
        if let Some(error) = *self.end_failure.lock().unwrap() {
            return Err(error);
        }
        self.actual.end_enrollment(expected)
    }
}
impl OwnerPhoneOriginalStore for Store {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<Box<dyn OwnerPhoneOriginalCreation + '_>, PrivateStateError> {
        self.actual.reserve_original(reservation)
    }
    fn load_original(&self) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError> {
        let saved = self.actual.load_original()?;
        if self.adoption.load(Ordering::SeqCst) == 0 {
            return Ok(saved);
        }
        let mode = *self.mode.lock().unwrap();
        if !matches!(mode, Mode::MissingActualLineage | Mode::ChangedOriginalKey) {
            return Ok(saved);
        }
        let Some(saved) = saved else {
            return Ok(None);
        };
        let public = saved.public_intent();
        let (reservation, key) = saved.into_parts();
        Ok(Some(if matches!(mode, Mode::MissingActualLineage) {
            OwnerPhonePrivateOriginal::prepared(reservation, key.unwrap())
        } else {
            OwnerPhonePrivateOriginal::bound(
                reservation,
                PrivateKeyMaterial::new(Zeroizing::new([99; 32])),
                public.unwrap(),
            )
        }))
    }
}
impl OwnerPhoneEnrollmentStore for Store {
    fn adopt_owner_phone(
        &self,
        original: &OwnerPhoneRequest,
        digest: &[u8; 32],
        consent: &DisclosedConsent,
        issued: Option<(&CredentialId, &ResourceId)>,
    ) -> Result<OwnerPhoneAdoption, OwnerPhoneAdoptionFailure> {
        self.adoption.fetch_add(1, Ordering::SeqCst);
        if let Some(held) = self.held.lock().unwrap().clone() {
            held.hold();
        }
        self.reads.store(0, Ordering::SeqCst);
        let mode = *self.mode.lock().unwrap();
        if matches!(mode, Mode::MissingActualBinding) {
            return Ok(OwnerPhoneAdoption::new(
                consent.public(),
                OwnerPhoneEnrollmentKind::Issued,
            ));
        }
        let value = self
            .actual
            .adopt_owner_phone(original, digest, consent, issued)?;
        if matches!(mode, Mode::PanicAfterPublication) {
            panic!("injected worker unwind after actual publication");
        }
        Ok(match mode {
            Mode::WrongKind => {
                OwnerPhoneAdoption::new(value.public(), OwnerPhoneEnrollmentKind::Pending)
            }
            Mode::WrongReportedPublic => OwnerPhoneAdoption::new(
                value.public().with_attempt(AttemptId::new([99; 16])),
                value.kind(),
            ),
            _ => value,
        })
    }
}
struct Connector(Mutex<Vec<SocketAddr>>);
impl EnrollmentConnector for Connector {
    fn connect(&self, address: SocketAddr) -> Result<TcpStream, NativeClientError> {
        self.0.lock().unwrap().push(address);
        TcpStream::connect_timeout(&address, Duration::from_secs(3))
            .map_err(|error| NativeClientError::Io(error.kind()))
    }
}
struct NoEntropy;
impl EnrollmentEntropyFactory for NoEntropy {
    fn create(&self) -> Box<dyn EnrollmentEntropy> {
        panic!("recovery generated entropy");
    }
}
struct Fixture {
    root: PrivateFixture,
    listener: TcpListener,
    state: Arc<Store>,
    proof: Arc<OwnerPhonePreparation>,
    connector: Arc<Connector>,
}
impl Fixture {
    fn new(mode: Mode) -> Self {
        let root = PrivateFixture::new();
        create_directory_beneath(root.path(), Path::new("private")).unwrap();
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let source = OwnerPhoneSource::new(
            PrincipalId::new("owner").unwrap(),
            MembershipId::new("member").unwrap(),
            AudienceId::new("gateway").unwrap(),
            CredentialId::new("bootstrap-public").unwrap(),
            Resource::new(
                OrganizationId::new("org").unwrap(),
                ResourceId::new("gateway").unwrap(),
            ),
        )
        .unwrap();
        let reservation = OwnerPhoneReservation::new(
            OwnerPhoneRequestId::new([1; 16]).unwrap(),
            OwnerPhoneContext::new(
                source,
                identity(8).public_spki(),
                listener.local_addr().unwrap(),
            )
            .unwrap(),
        );
        let actual = FilePairingState::open(root.path(), Path::new("private")).unwrap();
        actual
            .reserve_original(reservation.clone())
            .unwrap()
            .prepare(identity(7).key_material())
            .unwrap();
        let proof = Arc::new(restore_owner_phone(&actual, &reservation).unwrap());
        Self {
            root,
            listener,
            proof,
            state: Arc::new(Store {
                actual,
                mode: Mutex::new(mode),
                reads: AtomicUsize::new(0),
                adoption: AtomicUsize::new(0),
                generic: AtomicUsize::new(0),
                held: Mutex::new(None),
                end_failure: Mutex::new(None),
            }),
            connector: Arc::new(Connector(Mutex::new(Vec::new()))),
        }
    }
    fn parent(&self) -> NativeSession {
        self.parent_with_connector(self.connector.clone())
    }
    fn parent_with_connector(&self, connector: Arc<dyn EnrollmentConnector>) -> NativeSession {
        NativeSession::compose(Dependencies {
            state: self.state.clone(),
            connector,
            entropy: Arc::new(NoEntropy),
            path: self.root.path().join("private/cache.sqlite"),
            // A different valid configured address must never substitute for the original.
            address: "127.0.0.1:9".parse().unwrap(),
            wall: Arc::new(Wall),
            clock: Arc::new(Monotonic::new()),
            pause: Arc::new(ThreadPause),
            runtime: Builder::new_current_thread().enable_all().build().unwrap(),
            product: ProductDependencies::default(),
        })
        .unwrap()
    }
    fn enrollment(&self) -> PathBuf {
        self.root.path().join("private/client-enrollment")
    }
    fn original(&self) -> PathBuf {
        self.root.path().join("private/owner-phone-original")
    }
    fn peer(&self, replies: Vec<Reply>) -> JoinHandle<()> {
        let listener = self.listener.try_clone().unwrap();
        let proof = self.proof.clone();
        thread::spawn(move || {
            for reply in replies {
                let (socket, _) = listener.accept().unwrap();
                socket
                    .set_read_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                socket
                    .set_write_timeout(Some(Duration::from_secs(3)))
                    .unwrap();
                let transport = NativeTransport::accept(socket, &identity(8)).unwrap();
                assert_eq!(
                    transport.device_proof().key(),
                    proof.original().device_key()
                );
                let mut channel = EnrollmentChannel::new(transport);
                if matches!(reply, Reply::ReadRefused) {
                    assert!(matches!(
                        wire::decode_request(&channel.receive_envelope().unwrap()).unwrap(),
                        NativePairingRequest::OpenProduct
                    ));
                    channel
                        .send_envelope(&wire::encode_refused().unwrap())
                        .unwrap();
                    continue;
                }
                if matches!(reply, Reply::Product) {
                    assert!(matches!(
                        wire::decode_request(&channel.receive_envelope().unwrap()).unwrap(),
                        NativePairingRequest::OpenProduct
                    ));
                    let mut wire =
                        super::product_tests::fixtures::protected_wire(channel.into_transport());
                    let request = wire.request().unwrap();
                    assert_eq!(request.method, "conversation.list");
                    wire.send(nessa_protocol::protocol::OutgoingMessage::Response(
                        nessa_protocol::protocol::ResponseFrame::success(
                            &request.id,
                            &serde_json::json!({"conversations":[],"complete":true}),
                        )
                        .unwrap(),
                    ));
                    // A delivered response is not physical listener completion.
                    // Keep the real peer until the parent's owned close drains it.
                    assert!(wire.request().is_none());
                    continue;
                }
                let NativePairingRequest::RecoverOwnerPhone { request_id, digest } =
                    wire::decode_request(&channel.receive_envelope().unwrap()).unwrap()
                else {
                    panic!("owner recovery used a different exchange");
                };
                assert_eq!(request_id, proof.original().id());
                assert_eq!(&digest, proof.digest());
                let record = if matches!(reply, Reply::WrongDisclosure) {
                    active_record_in_scope(
                        &proof,
                        Resource::new(
                            OrganizationId::new("other-org").unwrap(),
                            ResourceId::new("gateway").unwrap(),
                        ),
                    )
                } else if matches!(reply, Reply::WrongPublic) {
                    active_record_with_invitation(
                        &proof,
                        proof.original().source().resource().clone(),
                        InvitationId::new([99; 16]),
                    )
                } else {
                    active_record(&proof)
                };
                let public =
                    PublicIntent::from_record(&record, AttemptId::new(*request_id.bytes()))
                        .unwrap();
                let status = DevicePairingStatus::Active {
                    record: Box::new(record),
                    access_epoch: u64::MAX,
                };
                let bytes = match reply {
                    Reply::Active | Reply::WrongDisclosure => {
                        wire::encode_owner_phone_recovery(request_id, digest, public, &status)
                    }
                    Reply::Approved => wire::encode_owner_phone_recovery(
                        request_id,
                        digest,
                        public,
                        &DevicePairingStatus::Claimed(Box::new(approved_record(
                            &proof,
                            proof.original().source().resource().clone(),
                        ))),
                    ),
                    Reply::Terminal => {
                        let DevicePairingStatus::Active { record, .. } = status else {
                            unreachable!()
                        };
                        let ended = record
                            .transition(
                                PairingEvent::end(
                                    nessa_auth::domain::pairing::TerminalCause::Cancelled,
                                ),
                                PairingInitiator::Principal(
                                    proof.original().source().owner().clone(),
                                ),
                                5,
                            )
                            .unwrap()
                            .after()
                            .clone();
                        wire::encode_owner_phone_recovery(
                            request_id,
                            digest,
                            public,
                            &DevicePairingStatus::Claimed(Box::new(ended)),
                        )
                    }
                    Reply::Product | Reply::ReadRefused => unreachable!(),
                    Reply::WrongPublic => {
                        wire::encode_owner_phone_recovery(request_id, digest, public, &status)
                    }
                    Reply::WrongId => wire::encode_owner_phone_recovery(
                        OwnerPhoneRequestId::new([99; 16]).unwrap(),
                        digest,
                        public,
                        &status,
                    ),
                    Reply::WrongDigest => {
                        wire::encode_owner_phone_recovery(request_id, [99; 32], public, &status)
                    }
                    Reply::WrongVariant => wire::encode_status(public, &status),
                    Reply::Refused => wire::encode_refused(),
                }
                .unwrap();
                channel.send_envelope(&bytes).unwrap();
            }
        })
    }
}
#[derive(Clone, Copy)]
enum Reply {
    Active,
    Approved,
    Terminal,
    Product,
    ReadRefused,
    WrongDisclosure,
    WrongPublic,
    WrongId,
    WrongDigest,
    WrongVariant,
    Refused,
}
fn active_record(proof: &OwnerPhonePreparation) -> PairingRecord {
    active_record_in_scope(proof, proof.original().source().resource().clone())
}
fn approved_record(proof: &OwnerPhonePreparation, resource: Resource) -> PairingRecord {
    approved_record_with_invitation(proof, resource, InvitationId::new([2; 16]))
}
fn approved_record_with_invitation(
    proof: &OwnerPhonePreparation,
    resource: Resource,
    invitation: InvitationId,
) -> PairingRecord {
    let original = proof.original();
    let source = original.source();
    let consent = ConsentIntent::new(
        ConsentIntentId::new([4; 16]),
        1,
        source.audience().clone(),
        source.owner().clone(),
        source.membership().clone(),
        resource,
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let initial =
        PairingRecord::new_owner_phone(invitation, consent, 0, PairingPolicy::initial()).unwrap();
    let owner = PairingInitiator::Principal(source.owner().clone());
    let approved = initial
        .transition(
            PairingEvent::owner_phone_approved(
                original.id(),
                original.device_key(),
                *proof.digest(),
            ),
            owner.clone(),
            1,
        )
        .unwrap()
        .after()
        .clone();
    approved
}
fn active_record_in_scope(proof: &OwnerPhonePreparation, resource: Resource) -> PairingRecord {
    active_record_with_invitation(proof, resource, InvitationId::new([2; 16]))
}
fn active_record_with_invitation(
    proof: &OwnerPhonePreparation,
    resource: Resource,
    invitation: InvitationId,
) -> PairingRecord {
    let original = proof.original();
    let owner = PairingInitiator::Principal(original.source().owner().clone());
    let approved = approved_record_with_invitation(proof, resource, invitation);
    let credential = CredentialId::new("issued").unwrap();
    let stage = AttemptId::new([9; 16]);
    let staged = approved
        .transition(
            PairingEvent::stage(credential.clone(), stage, 1),
            owner.clone(),
            2,
        )
        .unwrap()
        .after()
        .clone();
    let paired = staged
        .transition(
            PairingEvent::receiver(
                credential,
                stage,
                ResourceId::new("receiver").unwrap(),
                1,
                1,
            ),
            PairingInitiator::System,
            3,
        )
        .unwrap()
        .after()
        .clone();
    paired
        .transition(PairingEvent::activate(1), owner, 4)
        .unwrap()
        .after()
        .clone()
}
fn captured(status: SessionStatus) -> RetainedOwnerStatus {
    let SessionStatus::Owner(Ok(value)) = status else {
        panic!("expected completed owner status");
    };
    value
}

#[test]
fn actual_pinned_recovery_has_no_adoption_then_each_fresh_status_reconciles_original_issued_identity(
) {
    let fixture = Fixture::new(Mode::Good);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::Active, Reply::Active, Reply::Active]);
    let recovered = parent
        .recover_owner_original(fixture.proof.clone())
        .unwrap()
        .unwrap();
    assert!(matches!(
        recovered.recovery().unwrap().status(),
        NativePairingStatus::Active {
            access_epoch: u64::MAX,
            ..
        }
    ));
    assert!(recovered.settlement().is_none() && recovered.purge().is_none());
    assert!(!fixture.enrollment().exists());
    assert_eq!(
        fixture
            .state
            .actual
            .load_original()
            .unwrap()
            .unwrap()
            .public_intent(),
        None
    );
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 0);
    for count in [1, 2] {
        let status = captured(parent.status().unwrap());
        assert!(matches!(
            status.settlement(),
            Some(Ok(OwnerPhoneSettlement::Adopted(_)))
        ));
        let foreign = NativeOwnerStatus { value: Ok(status) };
        assert_eq!(
            foreign.recovered().unwrap().receipt().access_epoch(),
            Some(u64::MAX)
        );
        assert!(foreign.adoption().is_some() && foreign.settlement_failure().is_none());
        assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), count);
    }
    assert_eq!(fixture.state.generic.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture.connector.0.lock().unwrap().as_slice(),
        &[fixture.proof.original().endpoint(); 3]
    );
    peer.join().unwrap();
    let proof = fixture.proof.clone();
    parent.close().unwrap();
    assert_eq!(
        proof.original().source().credential().as_str(),
        "bootstrap-public"
    );
    assert_eq!(
        fixture
            .state
            .actual
            .load_credential()
            .unwrap()
            .unwrap()
            .credential()
            .as_str(),
        "issued"
    );
}

#[test]
fn contradictory_compact_replies_refuse_before_original_binding_or_adoption() {
    for reply in [
        Reply::WrongId,
        Reply::WrongDigest,
        Reply::WrongVariant,
        Reply::Refused,
    ] {
        let fixture = Fixture::new(Mode::Good);
        let parent = fixture.parent();
        let peer = fixture.peer(vec![reply]);
        let value = captured(parent.status().unwrap());
        assert!(value.recovery().is_err());
        assert!(value.settlement().is_none() && value.purge().is_none());
        assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 0);
        assert!(!fixture.enrollment().exists());
        assert_eq!(
            fixture
                .state
                .actual
                .load_original()
                .unwrap()
                .unwrap()
                .public_intent(),
            None
        );
        let foreign = NativeOwnerStatus { value: Ok(value) };
        assert!(
            foreign.recovered().is_none() && foreign.recovery_failure().unwrap().native().is_some()
        );
        peer.join().unwrap();
    }
}

#[test]
fn reported_success_and_actual_saved_contradictions_retain_active_and_nominal_receipt_without_selection(
) {
    for mode in [
        Mode::WrongKind,
        Mode::WrongReportedPublic,
        Mode::MissingActualBinding,
        Mode::MissingActualLineage,
        Mode::ChangedOriginalKey,
        Mode::Key(1),
        Mode::Pin(1),
        Mode::Public(1),
        Mode::Credential(1),
        Mode::Receiver(1),
    ] {
        let fixture = Fixture::new(mode);
        let parent = fixture.parent();
        let peer = fixture.peer(vec![Reply::Active]);
        let refresh = parent.refresh(ReadStop::new()).unwrap();
        assert!(refresh.read().is_none() && refresh.recheck().is_none());
        let SessionStatus::Owner(Ok(value)) = refresh.initial() else {
            panic!("owner completion");
        };
        assert!(matches!(
            value.recovery().unwrap().status(),
            NativePairingStatus::Active { .. }
        ));
        assert!(matches!(
            value.settlement(),
            Some(Err(OwnerPhoneSettlementFailure::SavedRecord { .. }))
        ));
        let foreign = NativeOwnerStatus {
            value: Ok(value.clone()),
        };
        assert!(foreign.recovered().is_some() && foreign.adoption().is_none());
        let failure = foreign.settlement_failure().unwrap();
        assert_eq!(
            failure.kind(),
            NativeOwnerSettlementFailureKind::SavedRecord
        );
        assert!(failure.reported_adoption().is_some() && failure.selection().is_some());
        if matches!(mode, Mode::ChangedOriginalKey) {
            assert!(failure.selection().unwrap().preparation().is_some());
        } else {
            assert!(failure.selection().unwrap().native().is_some());
        }
        assert!(!fixture.root.path().join("private/cache.sqlite").exists());
        peer.join().unwrap();
    }
}

#[test]
fn second_load_contradictory_valid_record_refuses_reader_and_product_while_positive_record_is_retained(
) {
    for mode in [
        Mode::Key(2),
        Mode::Pin(2),
        Mode::Public(2),
        Mode::Credential(2),
        Mode::Receiver(2),
    ] {
        let fixture = Fixture::new(mode);
        let parent = fixture.parent();
        let peer = fixture.peer(vec![Reply::Active, Reply::Active]);
        let refresh = parent.refresh(ReadStop::new()).unwrap();
        let SessionStatus::Owner(Ok(value)) = refresh.initial() else {
            panic!("owner completion");
        };
        assert!(matches!(
            value.settlement(),
            Some(Ok(OwnerPhoneSettlement::Adopted(_)))
        ));
        assert!(refresh.selection_failure().is_some() && refresh.read().is_none());
        assert!(!fixture.root.path().join("private/cache.sqlite").exists());
        assert!(matches!(
            parent.open_product("phone-instance".into()),
            Err(SessionFailure::OwnerSelection { .. })
        ));
        assert!(fixture.state.actual.load_credential().unwrap().is_some());
        peer.join().unwrap();
    }
}

#[cfg(unix)]
#[test]
fn actual_second_acknowledgement_failure_retains_remote_active_and_first_exact_lineage_in_foreign_getters(
) {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(Mode::Good);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::Active, Reply::Active]);
    captured(parent.status().unwrap());
    let original = fs::read(fixture.original()).unwrap();
    fs::set_permissions(fixture.enrollment(), fs::Permissions::from_mode(0o400)).unwrap();
    let status = captured(parent.status().unwrap());
    let foreign = NativeOwnerStatus { value: Ok(status) };
    assert!(foreign.recovered().is_some() && foreign.adoption().is_none());
    let failure = foreign.settlement_failure().unwrap().adoption().unwrap();
    assert_eq!(failure.phase(), NativeOwnerAdoptionPhase::PublishEnrollment);
    assert_eq!(
        failure.private_state().kind(),
        crate::NativePrivateStateKind::Uncertain
    );
    assert!(failure.acknowledged_lineage().is_some());
    assert_eq!(fs::read(fixture.original()).unwrap(), original);
    fs::set_permissions(fixture.enrollment(), fs::Permissions::from_mode(0o600)).unwrap();
    peer.join().unwrap();
}

#[test]
fn missing_original_with_valid_owner_pending_or_issued_never_falls_back_or_connects() {
    for issued in [false, true] {
        let fixture = Fixture::new(Mode::Good);
        let record = active_record(&fixture.proof);
        let public = PublicIntent::from_record(
            &record,
            AttemptId::new(*fixture.proof.original().id().bytes()),
        )
        .unwrap();
        let consent = DisclosedConsent::from_intent(public, record.intent()).unwrap();
        let credential = CredentialId::new("issued").unwrap();
        let receiver = ResourceId::new("receiver").unwrap();
        fixture
            .state
            .actual
            .adopt_owner_phone(
                fixture.proof.original(),
                fixture.proof.digest(),
                &consent,
                if issued {
                    Some((&credential, &receiver))
                } else {
                    None
                },
            )
            .unwrap();
        fs::remove_file(fixture.original()).unwrap();
        let parent = fixture.parent();
        assert!(matches!(
            parent.status().unwrap(),
            SessionStatus::Owner(Err(OwnerPhoneRecoveryFailure::Preparation(
                OwnerPhonePreparationError::Missing
            )))
        ));
        let refresh = parent.refresh(ReadStop::new()).unwrap();
        assert!(refresh.read().is_none() && refresh.recheck().is_none());
        assert!(matches!(
            parent.open_product("phone-instance".into()),
            Err(SessionFailure::Owner(_))
        ));
        assert!(matches!(
            parent
                .recover_owner_original(fixture.proof.clone())
                .unwrap(),
            Err(OwnerPhoneRecoveryFailure::Preparation(
                OwnerPhonePreparationError::Missing
            ))
        ));
        assert!(fixture.connector.0.lock().unwrap().is_empty());
        assert!(!fixture.root.path().join("private/cache.sqlite").exists());
    }
}

#[test]
fn exact_saved_original_selects_actual_product_ready_and_response_on_original_endpoint() {
    let fixture = Fixture::new(Mode::Good);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::Active, Reply::Active, Reply::Product]);
    let status = captured(parent.status().unwrap());
    let recovered = status.recovery().unwrap();
    let issued = issued_owner_phone(fixture.state.as_ref(), recovered).unwrap();
    assert!(
        RetainedDeviceReader::from_issued(recovered.status(), issued)
            .unwrap()
            .is_some()
    );
    let child = parent.open_product("phone-instance".into()).unwrap();
    let ready = child.session().wait_ready().unwrap();
    assert_eq!(ready.credential_id, "issued");
    assert_ne!(
        ready.credential_id,
        fixture.proof.original().source().credential().as_str()
    );
    let ticket = child.session().request("conversation.list", "{}").unwrap();
    let response = ticket.wait();
    assert!(matches!(
        response.value(),
        nessa_client_core::product::ProductCompletion::Response(_)
    ));
    parent.close().unwrap();
    assert_eq!(fixture.state.generic.load(Ordering::SeqCst), 0);
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 2);
    assert_eq!(
        fixture.connector.0.lock().unwrap().as_slice(),
        &[fixture.proof.original().endpoint(); 2]
    );
    peer.join().unwrap();
}

#[test]
fn terminal_remote_receipt_and_actual_purge_survive_independent_private_removal_failure() {
    let fixture = Fixture::new(Mode::Good);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::Active, Reply::Terminal]);
    captured(parent.status().unwrap());
    *fixture.state.end_failure.lock().unwrap() = Some(PrivateStateError::Uncertain);
    let original = fs::read(fixture.original()).unwrap();
    let status = captured(parent.status().unwrap());
    let foreign = NativeOwnerStatus { value: Ok(status) };
    assert_eq!(
        foreign.recovered().unwrap().receipt().phase(),
        crate::NativeEnrollmentPhase::Terminal
    );
    assert_eq!(foreign.purge().unwrap().receiver(), "receiver");
    assert_eq!(
        foreign.settlement_failure().unwrap().end().unwrap().kind(),
        crate::NativePrivateStateKind::Uncertain
    );
    assert!(foreign.adoption().is_none());
    assert!(fixture.state.actual.load_credential().unwrap().is_some());
    assert_eq!(fs::read(fixture.original()).unwrap(), original);
    assert_eq!(
        parent
            .purge_receipt("receiver")
            .unwrap()
            .unwrap()
            .receiver()
            .as_str(),
        "receiver"
    );
    peer.join().unwrap();
}

struct ReleaseHeld(Arc<super::product_tests::fixtures::Held>);
impl Drop for ReleaseHeld {
    fn drop(&mut self) {
        self.0.release();
    }
}

#[test]
fn actual_owner_read_refusal_rechecks_through_original_recovery_and_retains_both_outcomes() {
    for final_reply in [Reply::Active, Reply::Refused] {
        let fixture = Fixture::new(Mode::Good);
        let parent = fixture.parent();
        let peer = fixture.peer(vec![Reply::Active, Reply::ReadRefused, final_reply]);
        let refresh = parent.refresh(ReadStop::new()).unwrap();
        assert!(matches!(refresh.initial(), SessionStatus::Owner(Ok(value))
            if value.status_if_settled().is_some()));
        assert_eq!(refresh.read().unwrap().result(), &Err(ReadFailure::Refused));
        let Some(SessionStatus::Owner(Ok(recheck))) = refresh.recheck() else {
            panic!("read refusal must use owner original recovery");
        };
        if matches!(final_reply, Reply::Active) {
            assert!(recheck.status_if_settled().is_some());
            assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 2);
        } else {
            assert!(matches!(
                recheck.recovery(),
                Err(OwnerPhoneRecoveryFailure::Native(
                    NativeClientError::Refused
                ))
            ));
            assert!(recheck.purge().is_none() && recheck.settlement().is_none());
            assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 1);
        }
        assert_eq!(fixture.state.generic.load(Ordering::SeqCst), 0);
        assert!(fixture.state.actual.load_credential().unwrap().is_some());
        peer.join().unwrap();
    }
}

#[test]
fn different_valid_escaped_preparation_refuses_before_dial_without_replacement() {
    let fixture = Fixture::new(Mode::Good);
    let other = Fixture::new(Mode::Good);
    assert_ne!(
        fixture.proof.original().endpoint(),
        other.proof.original().endpoint()
    );
    let parent = fixture.parent();
    let original = fs::read(fixture.original()).unwrap();
    assert!(matches!(
        parent.recover_owner_original(other.proof.clone()).unwrap(),
        Err(OwnerPhoneRecoveryFailure::Preparation(
            OwnerPhonePreparationError::Private(PrivateStateError::Conflict)
        ))
    ));
    assert!(fixture.connector.0.lock().unwrap().is_empty());
    assert!(!fixture.enrollment().exists());
    assert_eq!(fs::read(fixture.original()).unwrap(), original);
}

#[test]
fn readonly_recovery_refuses_otherwise_valid_complete_disclosure_in_another_original_scope() {
    let fixture = Fixture::new(Mode::Good);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::WrongDisclosure, Reply::Active]);
    let value = parent
        .recover_owner_original(fixture.proof.clone())
        .unwrap()
        .unwrap();
    assert!(matches!(
        value.recovery(),
        Err(OwnerPhoneRecoveryFailure::Native(NativeClientError::Wire(
            nessa_protocol::pairing::wire::NativeWireError::Correlation(
                nessa_auth::domain::pairing::PairingError::Conflict
            )
        )))
    ));
    assert!(value.settlement().is_none() && value.purge().is_none());
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 0);
    assert!(!fixture.enrollment().exists());
    let positive = parent
        .recover_owner_original(fixture.proof.clone())
        .unwrap()
        .unwrap();
    assert!(positive.recovery().is_ok() && positive.settlement().is_none());
    peer.join().unwrap();
}

#[test]
fn actual_worker_unwind_after_publication_is_unconfirmed_completion_with_durable_recovery() {
    let fixture = Fixture::new(Mode::PanicAfterPublication);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::Active, Reply::Active]);
    let SessionStatus::Owner(value) = parent.status().unwrap() else {
        panic!("owner scope");
    };
    let foreign = NativeOwnerStatus { value };
    assert!(!foreign.completion_captured());
    assert!(foreign.recovered().is_none() && foreign.adoption().is_none());
    assert_eq!(
        foreign
            .recovery_failure()
            .unwrap()
            .native()
            .unwrap()
            .worker(),
        Some(crate::NativeWorkerFault::Panic)
    );
    // Lost callback is not evidence that the admitted mutation never happened.
    assert!(fixture.state.actual.load_credential().unwrap().is_some());
    *fixture.state.mode.lock().unwrap() = Mode::Good;
    let recovered = captured(parent.status().unwrap());
    assert!(recovered.status_if_settled().is_some());
    peer.join().unwrap();
}
#[test]
fn observer_loss_and_close_join_actual_held_adoption_before_private_release_and_keep_late_fact() {
    let fixture = Fixture::new(Mode::Good);
    let gate = super::product_tests::fixtures::Held::new();
    let _release_on_failure = ReleaseHeld(gate.clone());
    *fixture.state.held.lock().unwrap() = Some(gate.clone());
    let parent = Arc::new(fixture.parent());
    let peer = fixture.peer(vec![Reply::Active]);
    let (reported, observer) = mpsc::channel();
    let owned = parent.clone();
    let operation = thread::spawn(move || {
        let status = owned.status().unwrap();
        let _ = reported.send(status);
    });
    gate.entered();
    assert!(matches!(
        parent.local_enrollment(),
        Err(SessionFailure::Busy)
    ));
    drop(observer);
    parent.request_close().unwrap();
    let (closing, closed) = mpsc::channel();
    let closer = parent.clone();
    let close = thread::spawn(move || {
        closing.send(closer.close_observed()).unwrap();
    });
    assert!(closed.recv_timeout(Duration::from_millis(50)).is_err());
    let Fixture {
        root,
        state,
        listener,
        proof,
        connector,
    } = fixture;
    drop(state);
    drop(listener);
    drop(connector);
    drop(parent);
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    gate.release();
    operation.join().unwrap();
    let observation = closed.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(observation.released() && observation.outcome().is_ok());
    close.join().unwrap();
    peer.join().unwrap();
    let restored = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    assert_eq!(
        restored
            .load_credential()
            .unwrap()
            .unwrap()
            .credential()
            .as_str(),
        "issued"
    );
    assert_eq!(
        restored
            .load_original()
            .unwrap()
            .unwrap()
            .reservation()
            .id(),
        proof.original().id()
    );
}

#[test]
fn actual_wrong_gateway_pin_refuses_recovery_before_lineage_or_adoption() {
    let fixture = Fixture::new(Mode::Good);
    let listener = fixture.listener.try_clone().unwrap();
    let peer = thread::spawn(move || {
        let (socket, _) = listener.accept().unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        socket
            .set_write_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        assert!(NativeTransport::accept(socket, &identity(99)).is_err());
    });
    let parent = fixture.parent();
    let value = parent
        .recover_owner_original(fixture.proof.clone())
        .unwrap()
        .unwrap();
    assert!(matches!(
        value.recovery(),
        Err(OwnerPhoneRecoveryFailure::Native(
            NativeClientError::Handshake(_)
        ))
    ));
    assert!(value.settlement().is_none() && value.purge().is_none());
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 0);
    assert_eq!(
        fixture
            .state
            .actual
            .load_original()
            .unwrap()
            .unwrap()
            .public_intent(),
        None
    );
    assert!(!fixture.enrollment().exists());
    peer.join().unwrap();
}

#[test]
fn never_issued_terminal_has_no_adoption_purge_or_cache_and_keeps_original() {
    let fixture = Fixture::new(Mode::Good);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::Terminal]);
    let original = fs::read(fixture.original()).unwrap();
    let refresh = parent.refresh(ReadStop::new()).unwrap();
    assert!(refresh.selection_failure().is_none() && refresh.read().is_none());
    let SessionStatus::Owner(Ok(value)) = refresh.initial() else {
        panic!("terminal owner");
    };
    assert!(matches!(
        value.recovery().unwrap().status(),
        NativePairingStatus::Terminal { .. }
    ));
    assert_eq!(value.settlement(), Some(&Ok(OwnerPhoneSettlement::Ended)));
    assert!(value.purge().is_none());
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 0);
    assert!(!fixture.enrollment().exists());
    assert!(!fixture.root.path().join("private/cache.sqlite").exists());
    assert_eq!(fs::read(fixture.original()).unwrap(), original);
    peer.join().unwrap();
}

#[cfg(unix)]
#[test]
fn later_close_drains_held_failed_adoption_without_relabeling_active_or_private_failure() {
    use std::os::unix::fs::PermissionsExt;
    let fixture = Fixture::new(Mode::Good);
    let parent = Arc::new(fixture.parent());
    let peer = fixture.peer(vec![Reply::Active, Reply::Active]);
    captured(parent.status().unwrap());
    fs::set_permissions(fixture.enrollment(), fs::Permissions::from_mode(0o400)).unwrap();
    let gate = super::product_tests::fixtures::Held::new();
    let _release_on_failure = ReleaseHeld(gate.clone());
    *fixture.state.held.lock().unwrap() = Some(gate.clone());
    let owned = parent.clone();
    let operation = thread::spawn(move || captured(owned.status().unwrap()));
    gate.entered();
    parent.request_close().unwrap();
    let (closing, closed) = mpsc::channel();
    let closer = parent.clone();
    let close = thread::spawn(move || closing.send(closer.close_observed()).unwrap());
    assert!(closed.recv_timeout(Duration::from_millis(50)).is_err());
    gate.release();
    let value = operation.join().unwrap();
    assert!(matches!(
        value.recovery().unwrap().status(),
        NativePairingStatus::Active { .. }
    ));
    let Some(Err(OwnerPhoneSettlementFailure::Adoption(error))) = value.settlement() else {
        panic!("original adoption failure");
    };
    assert_eq!(error.error(), PrivateStateError::Uncertain);
    assert!(error.acknowledged_lineage().is_some());
    let observation = closed.recv_timeout(Duration::from_secs(3)).unwrap();
    assert!(observation.released() && observation.outcome().is_ok());
    close.join().unwrap();
    fs::set_permissions(fixture.enrollment(), fs::Permissions::from_mode(0o600)).unwrap();
    peer.join().unwrap();
}

#[test]
fn actual_approved_adopts_pending_without_issued_selection_or_read() {
    let fixture = Fixture::new(Mode::Good);
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::Approved]);
    let refresh = parent.refresh(ReadStop::new()).unwrap();
    assert!(
        refresh.selection_failure().is_none()
            && refresh.read().is_none()
            && refresh.recheck().is_none()
    );
    let SessionStatus::Owner(Ok(value)) = refresh.initial() else {
        panic!("owner completion");
    };
    assert!(
        matches!(value.settlement(), Some(Ok(OwnerPhoneSettlement::Adopted(value))) if value.kind() == OwnerPhoneEnrollmentKind::Pending)
    );
    let saved = fixture.state.actual.load_pending().unwrap().unwrap();
    assert_eq!(saved.intent(), value.recovery().unwrap().status().public());
    assert!(fixture.state.actual.load_credential().unwrap().is_none());
    assert!(!fixture.root.path().join("private/cache.sqlite").exists());
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 1);
    assert_eq!(fixture.state.generic.load(Ordering::SeqCst), 0);
    peer.join().unwrap();
}

#[test]
fn cold_bound_original_refuses_otherwise_valid_public_lineage_before_adoption() {
    let fixture = Fixture::new(Mode::Good);
    let record = active_record(&fixture.proof);
    let public = PublicIntent::from_record(
        &record,
        AttemptId::new(*fixture.proof.original().id().bytes()),
    )
    .unwrap();
    let consent = DisclosedConsent::from_intent(public, record.intent()).unwrap();
    fixture
        .state
        .actual
        .adopt_owner_phone(
            fixture.proof.original(),
            fixture.proof.digest(),
            &consent,
            Some((
                &CredentialId::new("issued").unwrap(),
                &ResourceId::new("receiver").unwrap(),
            )),
        )
        .unwrap();
    let parent = fixture.parent();
    let peer = fixture.peer(vec![Reply::WrongPublic, Reply::Active]);
    let value = parent
        .recover_owner_original(fixture.proof.clone())
        .unwrap()
        .unwrap();
    assert!(matches!(
        value.recovery(),
        Err(OwnerPhoneRecoveryFailure::Native(NativeClientError::Wire(
            _
        )))
    ));
    assert!(value.settlement().is_none());
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 0);
    let positive = parent
        .recover_owner_original(fixture.proof.clone())
        .unwrap()
        .unwrap();
    assert!(positive.recovery().is_ok());
    peer.join().unwrap();
}

#[test]
fn substituted_actual_tcp_peer_refuses_original_recovery_without_tls_or_adoption() {
    struct DifferentPeer(SocketAddr);
    impl EnrollmentConnector for DifferentPeer {
        fn connect(&self, _: SocketAddr) -> Result<TcpStream, NativeClientError> {
            TcpStream::connect(self.0).map_err(|error| NativeClientError::Io(error.kind()))
        }
    }
    let fixture = Fixture::new(Mode::Good);
    let other = TcpListener::bind("127.0.0.1:0").unwrap();
    let parent =
        fixture.parent_with_connector(Arc::new(DifferentPeer(other.local_addr().unwrap())));
    let value = parent
        .recover_owner_original(fixture.proof.clone())
        .unwrap()
        .unwrap();
    assert!(matches!(
        value.recovery(),
        Err(OwnerPhoneRecoveryFailure::Native(NativeClientError::Wire(
            nessa_protocol::pairing::wire::NativeWireError::Correlation(
                nessa_auth::domain::pairing::PairingError::Conflict
            )
        )))
    ));
    let (mut socket, _) = other.accept().unwrap();
    socket
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    assert_eq!(std::io::Read::read(&mut socket, &mut [0; 1]).unwrap(), 0);
    assert_eq!(fixture.state.adoption.load(Ordering::SeqCst), 0);
    assert!(!fixture.enrollment().exists());
}
