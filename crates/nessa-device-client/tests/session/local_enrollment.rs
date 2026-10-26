//! Local public correlation uses the admitted private owner, without transport effects.
use super::*;
use crate::test_support::PrivateFixture;
use nessa_auth::{
    application::pairing::{DeviceCredential, PendingEnrollment, PrivateKeyMaterial},
    domain::pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId},
};
use nessa_local_storage::create_directory_beneath;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Condvar,
};
use zeroize::Zeroizing;

fn intent(attempt: u8) -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([attempt; 16]),
        ConsentIntentId::new([3; 16]),
        u64::MAX,
        u64::MAX,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([seed; 32]))).unwrap()
}
fn pending(attempt: u8) -> PendingEnrollment {
    PendingEnrollment::new(
        PrivateKeyMaterial::new(Zeroizing::new([7; 32])),
        identity(8).public_spki(),
        intent(attempt),
    )
}
#[derive(Clone, Copy)]
enum Saved {
    Empty,
    Pending,
    Issued,
    Both,
    PendingFailure(PrivateStateError),
    IssuedFailure(PrivateStateError),
}
struct Store {
    saved: Saved,
}
impl ClientPendingStore for Store {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        match self.saved {
            Saved::Pending | Saved::Both => Ok(Some(pending(2))),
            Saved::PendingFailure(error) => Err(error),
            _ => Ok(None),
        }
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        match self.saved {
            Saved::Issued | Saved::Both => Ok(Some(DeviceCredential::new(
                pending(2),
                CredentialId::new("issued").unwrap(),
                ResourceId::new("receiver").unwrap(),
            ))),
            Saved::IssuedFailure(error) => Err(error),
            _ => Ok(None),
        }
    }
    fn save_credential(
        &self,
        _: &CredentialId,
        _: &ResourceId,
        _: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        panic!("unexpected state write")
    }
    fn end_enrollment(&self, _: PublicIntent) -> Result<(), PrivateStateError> {
        panic!("unexpected record removal")
    }
    fn save_pending(
        &self,
        _: &PrivateKeyMaterial,
        _: &[u8; 44],
        _: PublicIntent,
        _: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        panic!("unexpected seed publication")
    }
}
struct NoEntropy;
impl EnrollmentEntropyFactory for NoEntropy {
    fn create(&self) -> Box<dyn EnrollmentEntropy> {
        panic!("local read acquired entropy")
    }
}
struct NoNetwork(AtomicUsize);
impl EnrollmentConnector for NoNetwork {
    fn connect(&self, _: SocketAddr) -> Result<TcpStream, NativeClientError> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    }
}
fn root() -> PrivateFixture {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    root
}
fn compose(
    root: &Path,
    state: Arc<dyn NativePrivateState>,
    connector: Arc<NoNetwork>,
) -> NativeSession {
    NativeSession::compose(Dependencies {
        state,
        connector,
        entropy: Arc::new(NoEntropy),
        path: root.join("private/cache.sqlite"),
        address: "127.0.0.1:9".parse().unwrap(),
        wall: Arc::new(Wall),
        clock: Arc::new(Monotonic::new()),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
        product: ProductDependencies::default(),
    })
    .unwrap()
}
#[test]
fn local_enrollment_preserves_saved_public_correlation_without_external_effects() {
    for saved in [Saved::Empty, Saved::Pending, Saved::Issued] {
        let root = root();
        let network = Arc::new(NoNetwork(AtomicUsize::new(0)));
        let session = compose(root.path(), Arc::new(Store { saved }), network.clone());
        let local = session.local_enrollment().unwrap();
        if let Some(local) = local {
            assert_eq!(local.intent(), intent(2));
            assert_eq!(local.device_key(), identity(7).device_key());
            assert_eq!(local.gateway_pin(), identity(8).public_spki());
            if matches!(saved, Saved::Issued) {
                let issued = local.issued().unwrap();
                assert_eq!(issued.credential().as_str(), "issued");
                assert_eq!(issued.receiver().as_str(), "receiver");
            } else {
                assert!(local.issued().is_none());
            }
        } else {
            assert!(matches!(saved, Saved::Empty));
        }
        assert_eq!(network.0.load(Ordering::SeqCst), 0);
        assert!(!root.path().join("private/cache.sqlite").exists());
        session.request_close().unwrap();
        assert!(matches!(
            session.local_enrollment(),
            Err(SessionFailure::Closed)
        ));
        session.close().unwrap();
        assert!(matches!(
            session.local_enrollment(),
            Err(SessionFailure::Closed)
        ));
    }
}
#[test]
fn local_enrollment_does_not_hide_private_failure_or_contradictory_records() {
    for (saved, expected) in [
        (Saved::Both, PrivateStateError::Conflict),
        (
            Saved::PendingFailure(PrivateStateError::Corrupt),
            PrivateStateError::Corrupt,
        ),
        (
            Saved::PendingFailure(PrivateStateError::Uncertain),
            PrivateStateError::Uncertain,
        ),
        (
            Saved::IssuedFailure(PrivateStateError::Unavailable),
            PrivateStateError::Unavailable,
        ),
    ] {
        let root = root();
        let network = Arc::new(NoNetwork(AtomicUsize::new(0)));
        let session = compose(root.path(), Arc::new(Store { saved }), network.clone());
        assert!(
            matches!(session.local_enrollment(),Err(SessionFailure::PrivateState(error)) if error==expected)
        );
        assert_eq!(network.0.load(Ordering::SeqCst), 0);
        assert!(!root.path().join("private/cache.sqlite").exists());
        session.close().unwrap();
    }
}
#[test]
fn local_enrollment_real_private_record_survives_retry_reopen_and_issued_replacement() {
    let root = root();
    let state = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    state
        .save_pending(
            identity(7).key_material(),
            &identity(8).public_spki(),
            intent(2),
            None,
        )
        .unwrap();
    drop(state);
    let session = NativeSession::open(
        root.path(),
        Path::new("private"),
        "127.0.0.1:9".parse().unwrap(),
        None,
    )
    .unwrap();
    let original = session.local_enrollment().unwrap().unwrap();
    session.close().unwrap();
    let state = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    state
        .save_pending(
            identity(7).key_material(),
            &identity(8).public_spki(),
            intent(4),
            Some(intent(2)),
        )
        .unwrap();
    state
        .save_credential(
            &CredentialId::new("issued").unwrap(),
            &ResourceId::new("receiver").unwrap(),
            intent(4),
        )
        .unwrap();
    drop(state);
    let session = NativeSession::open(
        root.path(),
        Path::new("private"),
        "127.0.0.1:9".parse().unwrap(),
        None,
    )
    .unwrap();
    let retry = session.local_enrollment().unwrap().unwrap();
    assert_ne!(retry.intent().attempt(), original.intent().attempt());
    assert_eq!(retry.device_key(), original.device_key());
    assert_eq!(retry.gateway_pin(), original.gateway_pin());
    assert_eq!(retry.intent().invitation(), original.intent().invitation());
    assert!(original.issued().is_none());
    assert_eq!(retry.issued().unwrap().credential().as_str(), "issued");
    session.close().unwrap();
    let state = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    state.end_enrollment(intent(4)).unwrap();
    drop(state);
    let session = NativeSession::open(
        root.path(),
        Path::new("private"),
        "127.0.0.1:9".parse().unwrap(),
        None,
    )
    .unwrap();
    assert!(session.local_enrollment().unwrap().is_none());
    assert!(retry.issued().is_some());
    session.close().unwrap();
}

struct HeldRead {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
impl HeldRead {
    fn wait(&self) {
        let mut held = self.state.lock().unwrap();
        held.0 = true;
        self.changed.notify_all();
        while !held.1 {
            held = self.changed.wait(held).unwrap();
        }
    }
    fn entered(&self) {
        let (held, timeout) = self
            .changed
            .wait_timeout_while(self.state.lock().unwrap(), Duration::from_secs(3), |v| !v.0)
            .unwrap();
        assert!(!timeout.timed_out() && held.0);
    }
    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
}
struct HeldStore {
    owned: FilePairingState,
    gate: Arc<HeldRead>,
}
impl ClientPendingStore for HeldStore {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.gate.wait();
        self.owned.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.owned.load_credential()
    }
    fn save_credential(
        &self,
        _: &CredentialId,
        _: &ResourceId,
        _: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        panic!("unexpected write")
    }
    fn end_enrollment(&self, _: PublicIntent) -> Result<(), PrivateStateError> {
        panic!("unexpected removal")
    }
    fn save_pending(
        &self,
        _: &PrivateKeyMaterial,
        _: &[u8; 44],
        _: PublicIntent,
        _: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        panic!("unexpected seed")
    }
}
#[test]
fn local_enrollment_held_read_reserves_admission_and_private_scope_until_close() {
    let root = root();
    let gate = Arc::new(HeldRead {
        state: Mutex::new((false, false)),
        changed: Condvar::new(),
    });
    let owned = FilePairingState::open(root.path(), Path::new("private")).unwrap();
    owned
        .save_pending(
            identity(7).key_material(),
            &identity(8).public_spki(),
            intent(2),
            None,
        )
        .unwrap();
    let session = Arc::new(compose(
        root.path(),
        Arc::new(HeldStore {
            owned,
            gate: gate.clone(),
        }),
        Arc::new(NoNetwork(AtomicUsize::new(0))),
    ));
    let read = {
        let session = session.clone();
        std::thread::spawn(move || session.local_enrollment())
    };
    gate.entered();
    assert!(matches!(
        session.local_enrollment(),
        Err(SessionFailure::Busy)
    ));
    session.request_close().unwrap();
    let (sent, received) = std::sync::mpsc::channel();
    let close = {
        let session = session.clone();
        std::thread::spawn(move || {
            session.close().unwrap();
            sent.send(()).unwrap();
        })
    };
    assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
    assert!(matches!(
        FilePairingState::open(root.path(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    gate.release();
    let historical = read.join().unwrap().unwrap().unwrap();
    close.join().unwrap();
    assert_eq!(historical.intent(), intent(2));
    assert_eq!(historical.device_key(), identity(7).device_key());
    assert!(matches!(
        session.local_enrollment(),
        Err(SessionFailure::Closed)
    ));
    drop(FilePairingState::open(root.path(), Path::new("private")).unwrap());
}

impl OwnerPhoneOriginalStore for Store {
    fn reserve_original(
        &self,
        _: OwnerPhoneReservation,
    ) -> Result<
        Box<dyn nessa_auth::application::pairing::OwnerPhoneOriginalCreation + '_>,
        PrivateStateError,
    > {
        panic!("unexpected original reservation")
    }
    fn load_original(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::OwnerPhonePrivateOriginal>,
        PrivateStateError,
    > {
        Ok(None)
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        Ok(false)
    }
}

impl OwnerPhoneOriginalStore for HeldStore {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<
        Box<dyn nessa_auth::application::pairing::OwnerPhoneOriginalCreation + '_>,
        PrivateStateError,
    > {
        self.owned.reserve_original(reservation)
    }
    fn load_original(
        &self,
    ) -> Result<
        Option<nessa_auth::application::pairing::OwnerPhonePrivateOriginal>,
        PrivateStateError,
    > {
        self.owned.load_original()
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        self.owned.enrollment_signed()
    }
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
        let _ = (original, digest, consent, issued);
        panic!("read-only fixture unexpectedly adopted owner-phone enrollment")
    }
}

impl nessa_auth::application::pairing::OwnerPhoneEnrollmentStore for HeldStore {
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
