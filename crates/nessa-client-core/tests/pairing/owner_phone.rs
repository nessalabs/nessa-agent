//! Actual private original keys and canonical crypto, with injected refusal seams.
use super::*;
use nessa_auth::{
    adapters::pairing::{rand::Error as RngError, FilePairingState},
    application::pairing::{
        ClientPendingStore, OwnerPhoneOriginalCreation, OwnerPhonePrivateOriginal,
        PrivateKeyMaterial,
    },
    domain::{
        pairing::{
            AttemptId, ConsentClass, ConsentIntentId, InvitationId, OwnerPhoneContext,
            OwnerPhoneRequestId, OwnerPhoneSource, PublicIntent,
        },
        AudienceId, CredentialId, MembershipId, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Barrier,
    },
};
use tempfile::TempDir;
use zeroize::Zeroizing;
struct Fixture {
    _root: TempDir,
    root: PathBuf,
    store: FilePairingState,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        // Windows validates an ordinary drive path and creates the protected
        // DACL. A canonicalized temp path is a reparse point there, which the
        // private-directory check refuses. Unix resolves the temp alias first.
        #[cfg(windows)]
        let path = root.path().join("root");
        #[cfg(not(windows))]
        let path = root.path().canonicalize().unwrap().join("root");
        nessa_local_storage::create_directory(&path).unwrap();
        nessa_local_storage::create_directory_beneath(&path, Path::new("private")).unwrap();
        let store = FilePairingState::open(&path, Path::new("private")).unwrap();
        Self {
            _root: root,
            root: path,
            store,
        }
    }
    fn original_path(&self) -> PathBuf {
        self.root.join("private/owner-phone-original")
    }
}
#[derive(Default)]
struct Entropy {
    calls: usize,
    reservation_path: Option<PathBuf>,
}
impl RngCore for Entropy {
    fn next_u32(&mut self) -> u32 {
        let mut bytes = [0; 4];
        self.fill_bytes(&mut bytes);
        u32::from_le_bytes(bytes)
    }
    fn next_u64(&mut self) -> u64 {
        let mut bytes = [0; 8];
        self.fill_bytes(&mut bytes);
        u64::from_le_bytes(bytes)
    }
    fn fill_bytes(&mut self, bytes: &mut [u8]) {
        self.calls += 1;
        bytes.fill(42);
    }
    fn try_fill_bytes(&mut self, bytes: &mut [u8]) -> Result<(), RngError> {
        if let Some(path) = &self.reservation_path {
            assert!(
                path.exists(),
                "original reservation is durable before entropy"
            );
            let bytes = fs::read(path).unwrap();
            assert_eq!(
                &bytes[..4],
                b"NSOR",
                "durable original exists before entropy"
            );
        }
        self.fill_bytes(bytes);
        Ok(())
    }
}
impl CryptoRng for Entropy {}
fn reservation(id: u8, owner: &str, endpoint: &str, pin_seed: u8) -> OwnerPhoneReservation {
    let source = OwnerPhoneSource::new(
        PrincipalId::new(owner).unwrap(),
        MembershipId::new("member").unwrap(),
        AudienceId::new("gateway").unwrap(),
        CredentialId::new("bootstrap").unwrap(),
        Resource::new(
            OrganizationId::new("organization").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
    )
    .unwrap();
    let pin = NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([pin_seed; 32])))
        .unwrap()
        .public_spki();
    OwnerPhoneReservation::new(
        OwnerPhoneRequestId::new([id; 16]).unwrap(),
        OwnerPhoneContext::new(source, pin, endpoint.parse().unwrap()).unwrap(),
    )
}
fn expected() -> OwnerPhoneReservation {
    reservation(1, "owner", "127.0.0.1:1234", 7)
}
fn verify(proof: &OwnerPhonePreparation) {
    let verified = verify_owner_phone_request(proof.original().clone(), proof.signature()).unwrap();
    assert_eq!(verified.digest(), proof.digest());
    let saved_identity =
        NativeIdentity::restore(PrivateKeyMaterial::new(Zeroizing::new([42; 32]))).unwrap();
    assert_eq!(proof.original().device_key(), saved_identity.device_key());
}
#[test]
fn initial_preparation_replay_and_cold_restore_preserve_one_real_key_and_signed_original() {
    let fixture = Fixture::new();
    let original = expected();
    let mut rng = Entropy {
        reservation_path: Some(fixture.original_path()),
        ..Entropy::default()
    };
    let first = prepare_owner_phone(&fixture.store, original.clone(), &mut rng).unwrap();
    verify(&first);
    assert_eq!(rng.calls, 1);
    let bytes = fs::read(fixture.original_path()).unwrap();
    let repeated = prepare_owner_phone(&fixture.store, original.clone(), &mut rng).unwrap();
    assert_eq!(rng.calls, 1);
    assert_eq!(repeated.signature(), first.signature());
    assert_eq!(repeated.digest(), first.digest());
    assert_eq!(
        repeated.original().transcript(),
        first.original().transcript()
    );
    assert_eq!(fs::read(fixture.original_path()).unwrap(), bytes);
    drop(fixture.store);
    let reopened = FilePairingState::open(&fixture.root, Path::new("private")).unwrap();
    let restored = restore_owner_phone(&reopened, &original).unwrap();
    verify(&restored);
    assert_eq!(restored.signature(), first.signature());
    assert_eq!(restored.digest(), first.digest());
    assert_eq!(restored.original(), first.original());
}
#[test]
fn missing_restore_and_incomplete_reservation_never_generate_or_publish_a_replacement() {
    let fixture = Fixture::new();
    let original = expected();
    let mut rng = Entropy::default();
    assert!(matches!(
        restore_owner_phone(&fixture.store, &original),
        Err(OwnerPhonePreparationError::Missing)
    ));
    assert!(!fixture.original_path().exists());
    drop(fixture.store.reserve_original(original.clone()).unwrap());
    let before = fs::read(fixture.original_path()).unwrap();
    assert!(matches!(
        restore_owner_phone(&fixture.store, &original),
        Err(OwnerPhonePreparationError::Incomplete)
    ));
    assert!(matches!(
        prepare_owner_phone(&fixture.store, original, &mut rng),
        Err(OwnerPhonePreparationError::Incomplete)
    ));
    assert_eq!(rng.calls, 0);
    assert_eq!(fs::read(fixture.original_path()).unwrap(), before);
}
#[test]
fn changed_original_refuses_before_entropy_and_preserves_previous_proof() {
    let fixture = Fixture::new();
    let mut rng = Entropy::default();
    let first = prepare_owner_phone(&fixture.store, expected(), &mut rng).unwrap();
    let before = fs::read(fixture.original_path()).unwrap();
    for changed in [
        reservation(2, "owner", "127.0.0.1:1234", 7),
        reservation(1, "other", "127.0.0.1:1234", 7),
        reservation(1, "owner", "127.0.0.1:1235", 7),
        reservation(1, "owner", "127.0.0.1:1234", 8),
    ] {
        assert!(matches!(
            prepare_owner_phone(&fixture.store, changed.clone(), &mut rng),
            Err(OwnerPhonePreparationError::Private(
                PrivateStateError::Conflict
            ))
        ));
        assert!(matches!(
            restore_owner_phone(&fixture.store, &changed),
            Err(OwnerPhonePreparationError::Private(
                PrivateStateError::Conflict
            ))
        ));
        assert_eq!(rng.calls, 1);
        assert_eq!(fs::read(fixture.original_path()).unwrap(), before);
    }
    assert_eq!(
        restore_owner_phone(&fixture.store, &expected())
            .unwrap()
            .signature(),
        first.signature()
    );
}
struct Refusing;
impl OwnerPhoneOriginalStore for Refusing {
    fn reserve_original(
        &self,
        _: OwnerPhoneReservation,
    ) -> Result<Box<dyn OwnerPhoneOriginalCreation + '_>, PrivateStateError> {
        panic!("invalid pin cannot reserve")
    }
    fn load_original(&self) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError> {
        panic!("invalid pin cannot read")
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        panic!("invalid pin cannot read")
    }
}
#[test]
fn invalid_pin_refuses_before_any_private_effect_or_entropy() {
    let valid = expected();
    let invalid = OwnerPhoneReservation::new(
        valid.id(),
        OwnerPhoneContext::new(
            valid.context().source().clone(),
            [0; 44],
            valid.context().endpoint(),
        )
        .unwrap(),
    );
    let mut rng = Entropy::default();
    assert!(matches!(
        prepare_owner_phone(&Refusing, invalid.clone(), &mut rng),
        Err(OwnerPhonePreparationError::Crypto(
            PairingCryptoError::InvalidContext
        ))
    ));
    assert!(matches!(
        restore_owner_phone(&Refusing, &invalid),
        Err(OwnerPhonePreparationError::Crypto(
            PairingCryptoError::InvalidContext
        ))
    ));
    assert_eq!(rng.calls, 0);
}
#[test]
fn existing_legacy_pending_and_issued_scope_refuses_without_key_generation() {
    for issued in [false, true] {
        let fixture = Fixture::new();
        let original = expected();
        let mut rng = Entropy::default();
        let intent = PublicIntent::new(
            InvitationId::new([1; 16]),
            AttemptId::new([2; 16]),
            ConsentIntentId::new([3; 16]),
            1,
            600_000,
            ConsentClass::DeviceRead,
        )
        .unwrap();
        fixture
            .store
            .save_pending(
                &PrivateKeyMaterial::new(Zeroizing::new([9; 32])),
                original.context().gateway_pin(),
                intent,
                None,
            )
            .unwrap();
        if issued {
            fixture
                .store
                .save_credential(
                    &CredentialId::new("issued").unwrap(),
                    &ResourceId::new("receiver").unwrap(),
                    intent,
                )
                .unwrap();
        }
        let path = fixture.root.join("private/client-enrollment");
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            prepare_owner_phone(&fixture.store, original, &mut rng),
            Err(OwnerPhonePreparationError::Private(
                PrivateStateError::Conflict
            ))
        ));
        assert_eq!(rng.calls, 0);
        assert_eq!(fs::read(path).unwrap(), before);
        assert!(!fixture.original_path().exists());
    }
}
struct ReportedCreation {
    reservation: OwnerPhoneReservation,
    failure: PrivateStateError,
}
impl OwnerPhoneOriginalCreation for ReportedCreation {
    fn reservation(&self) -> &OwnerPhoneReservation {
        &self.reservation
    }
    fn prepare(self: Box<Self>, _: &PrivateKeyMaterial) -> Result<(), PrivateStateError> {
        Err(self.failure)
    }
}
struct Reported {
    different: bool,
    failure: PrivateStateError,
}
impl OwnerPhoneOriginalStore for Reported {
    fn reserve_original(
        &self,
        mut reservation: OwnerPhoneReservation,
    ) -> Result<Box<dyn OwnerPhoneOriginalCreation + '_>, PrivateStateError> {
        if self.different {
            reservation = super::tests::reservation(2, "owner", "127.0.0.1:1234", 7);
        }
        Ok(Box::new(ReportedCreation {
            reservation,
            failure: self.failure,
        }))
    }
    fn load_original(&self) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError> {
        Ok(None)
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        Ok(false)
    }
}
#[test]
fn contradictory_creation_refuses_before_entropy_and_uncertain_publication_stays_typed() {
    let error = PrivateStateError::Uncertain;
    let mut rng = Entropy::default();
    assert!(matches!(
        prepare_owner_phone(
            &Reported {
                different: true,
                failure: error
            },
            expected(),
            &mut rng
        ),
        Err(OwnerPhonePreparationError::Private(
            PrivateStateError::Conflict
        ))
    ));
    assert_eq!(rng.calls, 0);
    let result = prepare_owner_phone(
        &Reported {
            different: false,
            failure: error,
        },
        expected(),
        &mut rng,
    );
    assert!(matches!(result,Err(OwnerPhonePreparationError::Private(value)) if value==error));
    assert_eq!(rng.calls, 1);
}

struct RaceStore {
    owned: FilePairingState,
    first_reads: AtomicUsize,
    read_barrier: Barrier,
}
impl OwnerPhoneOriginalStore for RaceStore {
    fn reserve_original(
        &self,
        reservation: OwnerPhoneReservation,
    ) -> Result<Box<dyn OwnerPhoneOriginalCreation + '_>, PrivateStateError> {
        self.owned.reserve_original(reservation)
    }
    fn load_original(&self) -> Result<Option<OwnerPhonePrivateOriginal>, PrivateStateError> {
        let saved = self.owned.load_original()?;
        if self.first_reads.fetch_add(1, Ordering::SeqCst) < 2 {
            self.read_barrier.wait();
        }
        Ok(saved)
    }
    fn enrollment_signed(&self) -> Result<bool, PrivateStateError> {
        self.owned.enrollment_signed()
    }
}
#[test]
fn concurrent_initial_reads_do_not_create_two_keys_or_overwrite_the_winner() {
    let fixture = Fixture::new();
    let store = Arc::new(RaceStore {
        owned: fixture.store,
        first_reads: AtomicUsize::new(0),
        read_barrier: Barrier::new(2),
    });
    let handles: Vec<_> = (0..2)
        .map(|_| {
            let store = store.clone();
            std::thread::spawn(move || {
                let mut rng = Entropy::default();
                let result = prepare_owner_phone(store.as_ref(), expected(), &mut rng);
                (result, rng.calls)
            })
        })
        .collect();
    let values: Vec<_> = handles
        .into_iter()
        .map(|handle| handle.join().unwrap())
        .collect();
    assert_eq!(values.iter().map(|(_, calls)| calls).sum::<usize>(), 1);
    assert_eq!(
        values.iter().filter(|(result, _)| result.is_ok()).count(),
        1
    );
    assert!(values.iter().any(|(result, _)| matches!(
        result,
        Err(OwnerPhonePreparationError::Private(
            PrivateStateError::Conflict
        ))
    )));
    let proof = restore_owner_phone(store.as_ref(), &expected()).unwrap();
    verify(&proof);
}

#[test]
fn corrupt_saved_seed_is_not_a_new_original_or_successful_restore() {
    let fixture = Fixture::new();
    let mut rng = Entropy::default();
    prepare_owner_phone(&fixture.store, expected(), &mut rng).unwrap();
    let path = fixture.original_path();
    let mut bytes = fs::read(&path).unwrap();
    let seed_offset = bytes.len() - 64;
    bytes[seed_offset] ^= 1;
    fs::write(&path, &bytes).unwrap();
    assert!(matches!(
        restore_owner_phone(&fixture.store, &expected()),
        Err(OwnerPhonePreparationError::Private(
            PrivateStateError::Corrupt
        ))
    ));
    assert!(matches!(
        prepare_owner_phone(&fixture.store, expected(), &mut rng),
        Err(OwnerPhonePreparationError::Private(
            PrivateStateError::Corrupt
        ))
    ));
    assert_eq!(rng.calls, 1);
    assert_eq!(fs::read(path).unwrap(), bytes);
}
