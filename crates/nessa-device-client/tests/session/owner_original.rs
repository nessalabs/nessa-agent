//! Public Rust parent calls with one actual FS owner and held admitted publication.
use super::*;

#[path = "../../../../scripts/rust-test-support/isolated.rs"]
mod isolated;
use crate::test_support::PrivateFixture;
use nessa_auth::{
    adapters::pairing::verify_owner_phone_request,
    application::pairing::{
        DeviceCredential, OwnerPhoneOriginalCreation, OwnerPhonePrivateOriginal, PendingEnrollment,
        PrivateKeyMaterial,
    },
    domain::{
        pairing::{OwnerPhoneContext, OwnerPhoneRequestId, OwnerPhoneSource},
        AudienceId, MembershipId, OrganizationId, PrincipalId, Resource,
    },
};
use nessa_local_storage::create_directory_beneath;
use std::{
    fs,
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
};
use zeroize::Zeroizing;

fn root() -> PrivateFixture {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    root
}
fn reservation() -> OwnerPhoneReservation {
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
    OwnerPhoneReservation::new(
        OwnerPhoneRequestId::new([1; 16]).unwrap(),
        OwnerPhoneContext::new(source, pin, "127.0.0.1:9".parse().unwrap()).unwrap(),
    )
}
struct Counts {
    factory: AtomicUsize,
    entropy: AtomicUsize,
    network: AtomicUsize,
}
impl Counts {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            factory: AtomicUsize::new(0),
            entropy: AtomicUsize::new(0),
            network: AtomicUsize::new(0),
        })
    }
}
struct NoNetwork(Arc<Counts>);
impl EnrollmentConnector for NoNetwork {
    fn connect(&self, _: SocketAddr) -> Result<TcpStream, NativeClientError> {
        self.0.network.fetch_add(1, Ordering::SeqCst);
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    }
}
struct EntropyFactory {
    counts: Arc<Counts>,
    path: PathBuf,
}
impl EnrollmentEntropyFactory for EntropyFactory {
    fn create(&self) -> Box<dyn EnrollmentEntropy> {
        self.counts.factory.fetch_add(1, Ordering::SeqCst);
        Box::new(ReservedEntropy {
            counts: self.counts.clone(),
            path: self.path.clone(),
        })
    }
}
struct ReservedEntropy {
    counts: Arc<Counts>,
    path: PathBuf,
}
impl RngCore for ReservedEntropy {
    fn next_u32(&mut self) -> u32 {
        panic!("unexpected scalar entropy")
    }
    fn next_u64(&mut self) -> u64 {
        panic!("unexpected scalar entropy")
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.try_fill_bytes(bytes).unwrap();
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), EntropyError> {
        assert_eq!(
            &fs::read(&self.path).unwrap()[..4],
            b"NSOR",
            "actual durable reservation precedes entropy"
        );
        self.counts.entropy.fetch_add(1, Ordering::SeqCst);
        bytes.fill(7);
        Ok(())
    }
}
impl CryptoRng for ReservedEntropy {}
fn parent(root: &Path, state: Arc<dyn NativePrivateState>, counts: Arc<Counts>) -> NativeSession {
    NativeSession::compose(Dependencies {
        state,
        connector: Arc::new(NoNetwork(counts.clone())),
        entropy: Arc::new(EntropyFactory {
            counts,
            path: root.join("private/owner-phone-original"),
        }),
        path: root.join("private/cache.sqlite"),
        address: "127.0.0.1:9".parse().unwrap(),
        wall: Arc::new(Wall),
        clock: Arc::new(Monotonic::new()),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
        product: ProductDependencies::default(),
    })
    .unwrap()
}
struct Gate {
    entered: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    after_release: Duration,
}
impl Gate {
    fn hold(&self) {
        self.entered.send(()).unwrap();
        self.release.lock().unwrap().recv().unwrap();
        thread::sleep(self.after_release);
    }
}
struct Release(Option<mpsc::Sender<()>>);
impl Drop for Release {
    fn drop(&mut self) {
        if let Some(tx) = self.0.take() {
            let _ = tx.send(());
        }
    }
}
struct Store {
    owned: FilePairingState,
    gate: Option<Arc<Gate>>,
    failure: Option<PrivateStateError>,
}
impl ClientPendingStore for Store {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.owned.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.owned.load_credential()
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        public: PublicIntent,
        prior: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.owned.save_pending(key, pin, public, prior)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        public: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        self.owned.save_credential(credential, receiver, public)
    }
    fn end_enrollment(&self, public: PublicIntent) -> Result<(), PrivateStateError> {
        self.owned.end_enrollment(public)
    }
}
struct Creation<'a> {
    value: Box<dyn OwnerPhoneOriginalCreation + 'a>,
    gate: Option<Arc<Gate>>,
    failure: Option<PrivateStateError>,
}
impl OwnerPhoneOriginalCreation for Creation<'_> {
    fn reservation(&self) -> &OwnerPhoneReservation {
        self.value.reservation()
    }
    fn prepare(self: Box<Self>, key: &PrivateKeyMaterial) -> Result<(), PrivateStateError> {
        if let Some(gate) = &self.gate {
            gate.hold();
        }
        self.value.prepare(key)?;
        match self.failure {
            Some(failure) => Err(failure),
            None => Ok(()),
        }
    }
}
impl OwnerPhoneOriginalStore for Store {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<Box<dyn OwnerPhoneOriginalCreation + '_>, PrivateStateError> {
        Ok(Box::new(Creation {
            value: self.owned.reserve_original(reservation)?,
            gate: self.gate.clone(),
            failure: self.failure,
        }))
    }
    fn load_original(&self) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError> {
        self.owned.load_original()
    }
}

#[test]
fn parent_prepares_after_actual_reservation_and_revalidates_restoration_without_entropy_or_network()
{
    let root = root();
    let counts = Counts::new();
    let parent = parent(
        root.path(),
        Arc::new(FilePairingState::open(root.path(), Path::new("private")).unwrap()),
        counts.clone(),
    );
    assert!(matches!(
        parent.restore_owner_original(&reservation()),
        Err(SessionPreparationFailure::Preparation(
            OwnerPhonePreparationError::Missing
        ))
    ));
    assert_eq!(counts.factory.load(Ordering::SeqCst), 0);
    let proof = parent.prepare_owner_original(reservation()).unwrap();
    assert_eq!(counts.factory.load(Ordering::SeqCst), 1);
    assert_eq!(counts.entropy.load(Ordering::SeqCst), 1);
    for restore in [true, false] {
        let replay = if restore {
            parent.restore_owner_original(&reservation())
        } else {
            parent.prepare_owner_original(reservation())
        }
        .unwrap();
        assert_eq!(replay.original(), proof.original());
        assert_eq!(replay.signature(), proof.signature());
        assert_eq!(replay.digest(), proof.digest());
    }
    assert_eq!(counts.factory.load(Ordering::SeqCst), 1);
    assert_eq!(counts.entropy.load(Ordering::SeqCst), 1);
    assert_eq!(counts.network.load(Ordering::SeqCst), 0);
    verify_owner_phone_request(proof.original().clone(), proof.signature()).unwrap();
    parent.close().unwrap();
    assert!(matches!(
        parent.restore_owner_original(&reservation()),
        Err(SessionPreparationFailure::Session(SessionFailure::Closed))
    ));
    assert_eq!(counts.factory.load(Ordering::SeqCst), 1);
}
#[test]
fn admitted_preparation_keeps_private_lease_through_close_and_preserves_late_fact_or_failure() {
    isolated::run(
        "session::owner_original_tests::admitted_preparation_keeps_private_lease_through_close_and_preserves_late_fact_or_failure",
        || admitted_preparation(Duration::ZERO),
    );
}

#[test]
fn admitted_preparation_can_finish_after_the_former_three_second_wait() {
    isolated::run(
        "session::owner_original_tests::admitted_preparation_can_finish_after_the_former_three_second_wait",
        || admitted_preparation(Duration::from_secs(4)),
    );
}

fn admitted_preparation(after_release: Duration) {
    for failure in [None, Some(PrivateStateError::Uncertain)] {
        let root = root();
        let counts = Counts::new();
        let (entered_tx, entered) = mpsc::sync_channel(0);
        let (release, released) = mpsc::channel();
        let gate = Arc::new(Gate {
            entered: entered_tx,
            release: Mutex::new(released),
            after_release,
        });
        let parent = Arc::new(parent(
            root.path(),
            Arc::new(Store {
                owned: FilePairingState::open(root.path(), Path::new("private")).unwrap(),
                gate: Some(gate),
                failure,
            }),
            counts.clone(),
        ));
        let (sent, received) = mpsc::channel();
        let call = {
            let parent = parent.clone();
            thread::spawn(move || {
                sent.send(parent.prepare_owner_original(reservation())).ok();
            })
        };
        let release = Release(Some(release));
        entered.recv().unwrap();
        let (admission_tx, admission_rx) = mpsc::channel();
        let concurrent = {
            let parent = parent.clone();
            thread::spawn(move || {
                admission_tx.send(parent.local_enrollment()).ok();
            })
        };
        assert!(matches!(admission_rx.recv(), Ok(Err(SessionFailure::Busy))));
        concurrent.join().unwrap();
        assert_eq!(
            &fs::read(root.path().join("private/owner-phone-original")).unwrap()[..4],
            b"NSOR"
        );
        parent.request_close().unwrap();
        let (closed_tx, closed) = mpsc::channel();
        let drain = {
            let parent = parent.clone();
            thread::spawn(move || closed_tx.send(parent.close_observed()).unwrap())
        };
        assert!(closed.recv_timeout(Duration::from_millis(40)).is_err());
        assert!(matches!(
            FilePairingState::open(root.path(), Path::new("private")),
            Err(PrivateStateError::Locked)
        ));
        drop(release);
        match (failure, received.recv().unwrap()) {
            (None, Ok(proof)) => {
                assert_eq!(proof.original().id(), reservation().id());
                verify_owner_phone_request(proof.original().clone(), proof.signature()).unwrap();
            }
            (
                Some(expected),
                Err(SessionPreparationFailure::Preparation(OwnerPhonePreparationError::Private(
                    actual,
                ))),
            ) => assert_eq!(actual, expected),
            _ => panic!("admitted physical result was lost or fabricated"),
        }
        let receipt = closed.recv().unwrap();
        assert!(receipt.released());
        assert_eq!(receipt.outcome(), &Ok(()));
        call.join().unwrap();
        drain.join().unwrap();
        let state = FilePairingState::open(root.path(), Path::new("private")).unwrap();
        assert!(state.load_original().unwrap().unwrap().key().is_some());
        assert_eq!(counts.network.load(Ordering::SeqCst), 0);
        assert_eq!(counts.entropy.load(Ordering::SeqCst), 1);
    }
}

#[test]
fn disappearing_observer_and_last_parent_owner_do_not_abandon_admitted_private_work() {
    let root = root();
    let counts = Counts::new();
    let (entered_tx, entered) = mpsc::sync_channel(0);
    let (release, released) = mpsc::channel();
    let parent = Arc::new(parent(
        root.path(),
        Arc::new(Store {
            owned: FilePairingState::open(root.path(), Path::new("private")).unwrap(),
            gate: Some(Arc::new(Gate {
                entered: entered_tx,
                release: Mutex::new(released),
                after_release: Duration::ZERO,
            })),
            failure: None,
        }),
        counts,
    ));
    let (sent, received) = mpsc::channel();
    let call = {
        let parent = parent.clone();
        thread::spawn(move || {
            let result = parent.prepare_owner_original(reservation());
            sent.send(result).ok();
        })
    };
    entered.recv_timeout(Duration::from_secs(3)).unwrap();
    drop(received);
    drop(parent);
    let release = Release(Some(release));
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    drop(release);
    call.join().unwrap();
    let state = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    assert!(state.load_original().unwrap().unwrap().key().is_some());
}
#[test]
fn unknown_unwind_after_driver_handoff_does_not_confirm_release_from_empty_slot() {
    let root = root();
    let parent = Arc::new(parent(
        root.path(),
        Arc::new(FilePairingState::open(root.path(), Path::new("private")).unwrap()),
        Counts::new(),
    ));
    let (sent, received) = mpsc::channel();
    let abandoned = {
        let parent = parent.clone();
        thread::spawn(move || {
            let mut guard = parent.driver.lock().unwrap();
            let retained = guard.take().unwrap();
            assert!(sent.send(retained).is_ok());
            panic!("unknown unwind after private owner handoff");
        })
    };
    assert!(abandoned.join().is_err());
    let actual_private_owner = received.recv().unwrap();
    let receipt = parent.close_observed();
    assert!(!receipt.released());
    assert_eq!(receipt.outcome(), &Err(SessionFailure::Damaged));
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    drop(actual_private_owner);
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
    assert!(!parent.close_observed().released());
}

impl nessa_auth::application::pairing::OwnerPhoneEnrollmentStore for Store {
    fn adopt_owner_phone(
        &self,
        original: &nessa_auth::domain::pairing::OwnerPhoneRequest,
        digest: &[u8; 32],
        consent: &DisclosedConsent,
        issued: Option<(&CredentialId, &ResourceId)>,
    ) -> Result<
        nessa_auth::application::pairing::OwnerPhoneAdoption,
        nessa_auth::application::pairing::OwnerPhoneAdoptionFailure,
    > {
        self.owned
            .adopt_owner_phone(original, digest, consent, issued)
    }
}
