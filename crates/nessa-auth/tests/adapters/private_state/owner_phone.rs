//! Actual private original publication, restoration and reciprocal exclusion.
use super::*;
use crate::{
    adapters::pairing::{FilePairingState, NativeIdentity},
    application::pairing::{ClientPendingStore, OwnerPhoneOriginalStore},
    domain::pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId, PublicIntent},
};
#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    fs,
    path::{Path, PathBuf},
    sync::{mpsc, Arc},
    thread,
    time::Duration,
};
use tempfile::TempDir;
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
fn seed(value: u8) -> PrivateKeyMaterial {
    PrivateKeyMaterial::new(Zeroizing::new([value; 32]))
}
fn reservation(address: SocketAddr) -> OwnerPhoneReservation {
    let source = OwnerPhoneSource::new(
        PrincipalId::new("owner").unwrap(),
        MembershipId::new("membership").unwrap(),
        AudienceId::new("gateway").unwrap(),
        CredentialId::new("bootstrap").unwrap(),
        Resource::new(
            OrganizationId::new("organization").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
    )
    .unwrap();
    let context = OwnerPhoneContext::new(
        source,
        NativeIdentity::restore(seed(7)).unwrap().public_spki(),
        address,
    )
    .unwrap();
    OwnerPhoneReservation::new(OwnerPhoneRequestId::new([1; 16]).unwrap(), context)
}
fn intent() -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        600_000,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
#[test]
fn original_records_round_trip_exact_context_seed_and_cold_durability() {
    for address in [
        "127.0.0.1:1234".parse().unwrap(),
        SocketAddr::V6(SocketAddrV6::new(Ipv6Addr::LOCALHOST, 1234, 42, 7)),
    ] {
        let fixture = Fixture::new();
        let expected = reservation(address);
        let creation = fixture.store.reserve_original(expected.clone()).unwrap();
        assert_eq!(creation.reservation(), &expected);
        creation.prepare(&seed(9)).unwrap();
        let bytes = fs::read(fixture.original_path()).unwrap();
        assert!(bytes.len() <= MAX_BYTES);
        let saved = fixture.store.load_original().unwrap().unwrap();
        assert_eq!(saved.reservation(), &expected);
        assert_eq!(saved.key().unwrap().expose_bytes(), &[9; 32]);
        drop(fixture.store);
        let reopened = FilePairingState::open(&fixture.root, Path::new("private")).unwrap();
        let saved = reopened.load_original().unwrap().unwrap();
        assert_eq!(saved.reservation(), &expected);
        assert_eq!(saved.key().unwrap().expose_bytes(), &[9; 32]);
    }
}
#[test]
fn dropped_creation_stays_reserved_and_cannot_generate_again() {
    let fixture = Fixture::new();
    let expected = reservation("127.0.0.1:1234".parse().unwrap());
    drop(fixture.store.reserve_original(expected.clone()).unwrap());
    let before = fs::read(fixture.original_path()).unwrap();
    let saved = fixture.store.load_original().unwrap().unwrap();
    assert_eq!(saved.reservation(), &expected);
    assert!(saved.key().is_none());
    assert!(matches!(
        fixture.store.reserve_original(expected),
        Err(PrivateStateError::Conflict)
    ));
    assert_eq!(fs::read(fixture.original_path()).unwrap(), before);
}
#[test]
fn reciprocal_legacy_exclusion_preserves_the_winning_record_in_both_orders() {
    for issued in [false, true] {
        let fixture = Fixture::new();
        let expected = reservation("127.0.0.1:1234".parse().unwrap());
        let pin = *expected.context().gateway_pin();
        fixture
            .store
            .save_pending(&seed(9), &pin, intent(), None)
            .unwrap();
        if issued {
            fixture
                .store
                .save_credential(
                    &CredentialId::new("issued").unwrap(),
                    &ResourceId::new("receiver").unwrap(),
                    intent(),
                )
                .unwrap();
        }
        let path = fixture.root.join("private/client-enrollment");
        let before = fs::read(&path).unwrap();
        assert!(matches!(
            fixture.store.reserve_original(expected),
            Err(PrivateStateError::Conflict)
        ));
        assert_eq!(fs::read(path).unwrap(), before);
        assert!(!fixture.original_path().exists());
    }
    for prepared in [false, true] {
        let fixture = Fixture::new();
        let expected = reservation("127.0.0.1:1234".parse().unwrap());
        let pin = *expected.context().gateway_pin();
        let creation = fixture.store.reserve_original(expected).unwrap();
        if prepared {
            creation.prepare(&seed(9)).unwrap();
        } else {
            drop(creation);
        }
        let before = fs::read(fixture.original_path()).unwrap();
        assert_eq!(
            fixture.store.save_pending(&seed(9), &pin, intent(), None),
            Err(PrivateStateError::Conflict)
        );
        assert_eq!(
            fixture.store.save_credential(
                &CredentialId::new("issued").unwrap(),
                &ResourceId::new("receiver").unwrap(),
                intent()
            ),
            Err(PrivateStateError::Conflict)
        );
        assert_eq!(fs::read(fixture.original_path()).unwrap(), before);
        assert!(!fixture.root.join("private/client-enrollment").exists());
    }
}
#[test]
fn live_creation_owns_operation_until_seed_acknowledgement() {
    let fixture = Fixture::new();
    let store = Arc::new(fixture.store);
    let expected = reservation("127.0.0.1:1234".parse().unwrap());
    let pin = *expected.context().gateway_pin();
    let creation = store.reserve_original(expected.clone()).unwrap();
    let (sent, received) = mpsc::channel();
    let writing = {
        let store = store.clone();
        thread::spawn(move || {
            sent.send(store.save_pending(&seed(9), &pin, intent(), None))
                .unwrap()
        })
    };
    assert!(received.recv_timeout(Duration::from_millis(40)).is_err());
    assert!(matches!(
        FilePairingState::open(&fixture.root, Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    creation.prepare(&seed(9)).unwrap();
    assert_eq!(
        received.recv_timeout(Duration::from_secs(3)).unwrap(),
        Err(PrivateStateError::Conflict)
    );
    writing.join().unwrap();
    assert_eq!(
        store.load_original().unwrap().unwrap().reservation(),
        &expected
    );
}
#[test]
fn corrupt_or_mixed_original_is_refused_on_every_cold_read() {
    let fixture = Fixture::new();
    let expected = reservation("127.0.0.1:1234".parse().unwrap());
    fixture
        .store
        .reserve_original(expected.clone())
        .unwrap()
        .prepare(&seed(9))
        .unwrap();
    let valid = encode(&expected, Some(&seed(9)), None).unwrap();
    for bytes in [
        valid[..valid.len() - 1].to_vec(),
        [valid.to_vec(), vec![0]].concat(),
        vec![0; MAX_BYTES + 1],
    ] {
        fs::write(fixture.original_path(), bytes).unwrap();
        assert!(matches!(
            fixture.store.load_original(),
            Err(PrivateStateError::Corrupt)
        ));
        assert!(matches!(
            fixture.store.load_pending(),
            Err(PrivateStateError::Corrupt)
        ));
        assert!(matches!(
            fixture.store.load_credential(),
            Err(PrivateStateError::Corrupt)
        ));
    }
    fs::write(fixture.original_path(), &valid).unwrap();
    let pending =
        super::super::encode_pending(&seed(9), expected.context().gateway_pin(), intent());
    fixture
        .store
        .publish(super::super::ENROLLMENT_FILE, &pending, false)
        .unwrap();
    assert!(matches!(
        fixture.store.load_original(),
        Err(PrivateStateError::Conflict)
    ));
    assert!(matches!(
        fixture.store.load_pending(),
        Err(PrivateStateError::Conflict)
    ));
    assert!(matches!(
        fixture.store.load_credential(),
        Err(PrivateStateError::Conflict)
    ));
}
#[test]
fn replaced_original_during_live_creation_does_not_get_seed_or_success() {
    let fixture = Fixture::new();
    let expected = reservation("127.0.0.1:1234".parse().unwrap());
    let creation = fixture.store.reserve_original(expected).unwrap();
    fs::write(fixture.original_path(), b"broken").unwrap();
    assert_eq!(creation.prepare(&seed(9)), Err(PrivateStateError::Corrupt));
    assert_eq!(fs::read(fixture.original_path()).unwrap(), b"broken");
}

#[test]
fn seed_and_published_public_key_must_agree_before_cold_exposure() {
    let fixture = Fixture::new();
    let expected = reservation("127.0.0.1:1234".parse().unwrap());
    fixture
        .store
        .reserve_original(expected.clone())
        .unwrap()
        .prepare(&seed(9))
        .unwrap();
    let valid = fs::read(fixture.original_path()).unwrap();
    for offset in [
        valid.len() - 1 - 2 * DeviceKey::LENGTH,
        valid.len() - 1 - DeviceKey::LENGTH,
    ] {
        let mut changed = valid.clone();
        changed[offset] ^= 1;
        fs::write(fixture.original_path(), &changed).unwrap();
        assert!(matches!(
            fixture.store.load_original(),
            Err(PrivateStateError::Corrupt)
        ));
        assert_eq!(fs::read(fixture.original_path()).unwrap(), changed);
    }
    fs::write(fixture.original_path(), &valid).unwrap();
    assert_eq!(
        fixture
            .store
            .load_original()
            .unwrap()
            .unwrap()
            .reservation(),
        &expected
    );
}

#[test]
fn largest_valid_original_fits_private_bound_and_round_trips_all_fields() {
    let fixture = Fixture::new();
    let gateway = "g".repeat(MAX_IDENTIFIER_BYTES);
    let source = OwnerPhoneSource::new(
        PrincipalId::new("p".repeat(MAX_IDENTIFIER_BYTES)).unwrap(),
        MembershipId::new("m".repeat(MAX_IDENTIFIER_BYTES)).unwrap(),
        AudienceId::new(gateway.clone()).unwrap(),
        CredentialId::new("c".repeat(MAX_IDENTIFIER_BYTES)).unwrap(),
        Resource::new(
            OrganizationId::new("o".repeat(MAX_IDENTIFIER_BYTES)).unwrap(),
            ResourceId::new(gateway).unwrap(),
        ),
    )
    .unwrap();
    let pin = NativeIdentity::restore(seed(7)).unwrap().public_spki();
    let endpoint = SocketAddr::V6(SocketAddrV6::new(
        Ipv6Addr::LOCALHOST,
        u16::MAX,
        u32::MAX,
        u32::MAX,
    ));
    let expected = OwnerPhoneReservation::new(
        OwnerPhoneRequestId::new([255; 16]).unwrap(),
        OwnerPhoneContext::new(source, pin, endpoint).unwrap(),
    );
    fixture
        .store
        .reserve_original(expected.clone())
        .unwrap()
        .prepare(&seed(9))
        .unwrap();
    assert_eq!(
        fs::read(fixture.original_path()).unwrap().len(),
        MAX_BYTES - PUBLIC_INTENT_BYTES
    );
    let public = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        u64::MAX,
        u64::MAX,
        ConsentClass::OwnerPhoneReadWrite,
    )
    .unwrap();
    let bound = encode(&expected, Some(&seed(9)), Some(public)).unwrap();
    assert_eq!(bound.len(), MAX_BYTES);
    fs::write(fixture.original_path(), bound).unwrap();
    assert_eq!(
        fixture
            .store
            .load_original()
            .unwrap()
            .unwrap()
            .reservation(),
        &expected
    );
}

#[cfg(unix)]
#[test]
fn readable_prepared_bytes_do_not_replace_failed_durability_acknowledgement() {
    // This permission fault fixture runs as the ordinary unprivileged CI/host user.
    let fixture = Fixture::new();
    let expected = reservation("127.0.0.1:1234".parse().unwrap());
    fixture
        .store
        .reserve_original(expected.clone())
        .unwrap()
        .prepare(&seed(9))
        .unwrap();
    let path = fixture.original_path();
    let bytes = fs::read(&path).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o400)).unwrap();
    assert!(matches!(
        fixture.store.load_original(),
        Err(PrivateStateError::Uncertain)
    ));
    assert_eq!(fs::read(&path).unwrap(), bytes);
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    assert_eq!(
        fixture
            .store
            .load_original()
            .unwrap()
            .unwrap()
            .reservation(),
        &expected
    );
}

#[test]
fn changed_valid_reservation_or_prepared_record_cannot_take_live_creation_seed() {
    for already_prepared in [false, true] {
        let fixture = Fixture::new();
        let expected = reservation("127.0.0.1:1234".parse().unwrap());
        let creation = fixture.store.reserve_original(expected.clone()).unwrap();
        let other = if already_prepared {
            expected
        } else {
            reservation("127.0.0.1:1235".parse().unwrap())
        };
        let other_seed = seed(8);
        let bytes = encode(
            &other,
            if already_prepared {
                Some(&other_seed)
            } else {
                None
            },
            None,
        )
        .unwrap();
        fs::write(fixture.original_path(), &bytes).unwrap();
        assert_eq!(creation.prepare(&seed(9)), Err(PrivateStateError::Conflict));
        assert_eq!(fs::read(fixture.original_path()).unwrap(), *bytes);
    }
}

#[test]
fn invalid_pin_reserves_no_private_original() {
    let fixture = Fixture::new();
    let expected = reservation("127.0.0.1:1234".parse().unwrap());
    let context = OwnerPhoneContext::new(
        expected.context().source().clone(),
        [0; 44],
        expected.context().endpoint(),
    )
    .unwrap();
    assert!(matches!(
        fixture
            .store
            .reserve_original(OwnerPhoneReservation::new(expected.id(), context)),
        Err(PrivateStateError::Corrupt)
    ));
    assert!(!fixture.original_path().exists());
}
