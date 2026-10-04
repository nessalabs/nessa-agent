use super::gateway_audit::{INTENT_FILE, OUTCOME_FILE};
use super::*;
#[cfg(unix)]
use crate::adapters::pairing::tests::io::{message_lengths, CryptoFixtureTransport};
use crate::adapters::pairing::{
    tests::{io::native_channels, Entropy},
    ClientAttempt, ManualCode, NativeIdentity, PairingCryptoError, ServerInvitation,
};
#[cfg(unix)]
use crate::adapters::pairing::{GatewayTrust, NativeTransport};
#[cfg(unix)]
use std::os::unix::{
    fs::{symlink, PermissionsExt},
    net::UnixStream,
};
use std::{
    fs,
    path::PathBuf,
    sync::atomic::{AtomicUsize, Ordering},
};
#[cfg(unix)]
use std::{thread, time::Duration};
use tempfile::TempDir;

struct Fixture {
    _root: TempDir,
    anchor: PathBuf,
    store: FilePairingState,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        #[cfg(windows)]
        let path = root.path().join("root");
        #[cfg(not(windows))]
        let path = root.path().canonicalize().unwrap().join("root");
        nessa_local_storage::create_directory(&path).unwrap();
        nessa_local_storage::create_directory_beneath(&path, Path::new("private")).unwrap();
        let store = FilePairingState::open(&path, Path::new("private")).unwrap();
        Self {
            _root: root,
            anchor: path,
            store,
        }
    }
    fn anchor(&self) -> PathBuf {
        self.anchor.clone()
    }
    fn path(&self) -> PathBuf {
        self.anchor.join("private")
    }
}
struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        100_000
    }
}
fn gateway() -> AudienceId {
    AudienceId::new("gateway-1").unwrap()
}
fn key(value: u8) -> PrivateKeyMaterial {
    PrivateKeyMaterial::new(Zeroizing::new([value; 32]))
}
fn intent(attempt: u8) -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([attempt; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        600_000,
    )
    .unwrap()
}
#[test]
fn gateway_key_is_exclusive_and_reopens() {
    let fixture = Fixture::new();
    assert!(fixture
        .store
        .restore_gateway_key(&gateway(), &Time)
        .unwrap()
        .is_none());
    fixture
        .store
        .save_gateway_key(&key(42), &gateway(), &Time)
        .unwrap();
    fixture
        .store
        .save_gateway_key(&key(42), &gateway(), &Time)
        .unwrap();
    assert_eq!(
        fixture.store.save_gateway_key(&key(43), &gateway(), &Time),
        Err(PrivateStateError::Conflict)
    );
    let root = fixture.anchor();
    drop(fixture.store);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    assert_eq!(
        reopened
            .restore_gateway_key(&gateway(), &Time)
            .unwrap()
            .unwrap()
            .expose_bytes(),
        &[42; 32]
    );
}
#[test]
fn private_state_lock_excludes_second_handle() {
    let fixture = Fixture::new();
    assert!(matches!(
        FilePairingState::open(&fixture.anchor(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    #[cfg(unix)]
    {
        // Replacing the lock name does not grant authority to this old handle.
        std::fs::rename(
            fixture.path().join(LOCK_FILE),
            fixture.path().join("old-lock"),
        )
        .unwrap();
        assert!(matches!(
            fixture.store.load_pending(),
            Err(PrivateStateError::Corrupt)
        ));
    }
    #[cfg(windows)]
    {
        // The actual mutation-capable lock pins its name while it is retained.
        assert!(std::fs::rename(
            fixture.path().join(LOCK_FILE),
            fixture.path().join("old-lock")
        )
        .is_err());
        assert!(fixture.store.load_pending().unwrap().is_none());
        assert!(!fixture.path().join("old-lock").exists());
    }
}
#[test]
fn pending_exact_retry_and_conflicting_state() {
    let fixture = Fixture::new();
    let pin = [5; 44];
    fixture
        .store
        .save_pending(&key(7), &pin, intent(2), None)
        .unwrap();
    fixture
        .store
        .save_pending(&key(7), &pin, intent(2), None)
        .unwrap();
    assert_eq!(
        fixture.store.save_pending(&key(8), &pin, intent(2), None),
        Err(PrivateStateError::Conflict)
    );
    assert_eq!(
        fixture
            .store
            .save_pending(&key(7), &[6; 44], intent(2), None),
        Err(PrivateStateError::Conflict)
    );
    assert_eq!(
        fixture.store.save_pending(&key(7), &pin, intent(4), None),
        Err(PrivateStateError::Conflict)
    );
    let saved = fixture.store.load_pending().unwrap().unwrap();
    assert_eq!(saved.intent(), intent(2));
    assert_eq!(saved.key().expose_bytes(), &[7; 32]);
    assert_eq!(saved.gateway_pin(), &pin);
}
#[test]
fn pending_retry_preserves_identity_and_requires_exact_prior() {
    let fixture = Fixture::new();
    let pin = [5; 44];
    assert_eq!(
        fixture
            .store
            .save_pending(&key(7), &pin, intent(4), Some(intent(2))),
        Err(PrivateStateError::Conflict)
    );
    fixture
        .store
        .save_pending(&key(7), &pin, intent(2), None)
        .unwrap();
    for (seed, pin, next, expected) in [
        (8, [5; 44], intent(4), Some(intent(2))),
        (7, [6; 44], intent(4), Some(intent(2))),
        (
            7,
            [5; 44],
            PublicIntent::new(
                InvitationId::new([9; 16]),
                AttemptId::new([4; 16]),
                ConsentIntentId::new([3; 16]),
                1,
                600_000,
            )
            .unwrap(),
            Some(intent(2)),
        ),
        (
            7,
            [5; 44],
            PublicIntent::new(
                InvitationId::new([1; 16]),
                AttemptId::new([4; 16]),
                ConsentIntentId::new([9; 16]),
                1,
                600_000,
            )
            .unwrap(),
            Some(intent(2)),
        ),
        (
            7,
            [5; 44],
            PublicIntent::new(
                InvitationId::new([1; 16]),
                AttemptId::new([4; 16]),
                ConsentIntentId::new([3; 16]),
                2,
                600_000,
            )
            .unwrap(),
            Some(intent(2)),
        ),
        (
            7,
            [5; 44],
            PublicIntent::new(
                InvitationId::new([1; 16]),
                AttemptId::new([4; 16]),
                ConsentIntentId::new([3; 16]),
                1,
                600_001,
            )
            .unwrap(),
            Some(intent(2)),
        ),
        (7, [5; 44], intent(4), Some(intent(9))),
    ] {
        assert_eq!(
            fixture.store.save_pending(&key(seed), &pin, next, expected),
            Err(PrivateStateError::Conflict)
        );
        assert_eq!(
            fixture.store.load_pending().unwrap().unwrap().intent(),
            intent(2)
        );
    }
    {
        fixture
            .store
            .save_pending(&key(7), &pin, intent(4), Some(intent(2)))
            .unwrap();
        // Exact retry after a lost answer succeeds even with the former expectation.
        fixture
            .store
            .save_pending(&key(7), &pin, intent(4), Some(intent(2)))
            .unwrap();
        assert_eq!(
            fixture.store.load_pending().unwrap().unwrap().intent(),
            intent(4)
        );
        assert_eq!(
            fixture
                .store
                .save_pending(&key(7), &pin, intent(6), Some(intent(2))),
            Err(PrivateStateError::Conflict)
        );
    }
}
#[test]
fn private_state_is_bounded_and_redacted() {
    let fixture = Fixture::new();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let path = fixture.path().join(ENROLLMENT_FILE);
    let encoded = fs::read(&path).unwrap();
    assert_eq!(encoded.len(), PENDING_BYTES);
    assert_eq!(format!("{:?}", key(7)), "PrivateKeyMaterial([REDACTED])");
    let pending = fixture.store.load_pending().unwrap().unwrap();
    assert_eq!(pending.key().expose_bytes(), &[7; 32]);
    assert_eq!(pending.gateway_pin(), &[5; 44]);
    assert_eq!(pending.intent(), intent(2));
    let debug = format!("{pending:?}");
    assert!(debug.contains("[REDACTED]"));
    assert!(!debug.contains("gateway_pin"));
    let original = fixture.path().join("pending-original-fixture");
    fs::rename(&path, &original).unwrap();
    for bytes in [
        vec![0; PENDING_BYTES - 1],
        vec![0; PENDING_BYTES + 1],
        vec![0; PENDING_BYTES],
        {
            let mut bad = encoded.to_vec();
            bad[128..136].fill(0);
            bad
        },
    ] {
        let path = fixture.path().join(ENROLLMENT_FILE);
        std::fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600)).unwrap();
        }
        assert!(matches!(
            fixture.store.load_pending(),
            Err(PrivateStateError::Corrupt)
        ));
    }
    #[cfg(unix)]
    {
        let path = fixture.path().join(ENROLLMENT_FILE);
        std::fs::remove_file(&path).unwrap();
        symlink("gateway-key", &path).unwrap();
        assert!(matches!(
            fixture.store.load_pending(),
            Err(PrivateStateError::Corrupt)
        ));
        std::fs::remove_file(&path).unwrap();
        std::fs::write(&path, &encoded).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(matches!(
            fixture.store.load_pending(),
            Err(PrivateStateError::Corrupt)
        ));
    }
    fs::remove_file(&path).unwrap();
    fs::rename(&original, &path).unwrap();
    assert!(!original.exists());
    assert_eq!(fs::read(&path).unwrap(), encoded);
    let restored = fixture.store.load_pending().unwrap().unwrap();
    assert_eq!(restored.key().expose_bytes(), &[7; 32]);
    assert_eq!(restored.gateway_pin(), &[5; 44]);
    assert_eq!(restored.intent(), intent(2));
    assert_eq!(format!("{restored:?}"), debug);
    let root = fixture.anchor();
    drop(fixture.store);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    let pending = reopened.load_pending().unwrap().unwrap();
    assert_eq!(pending.key().expose_bytes(), &[7; 32]);
    assert_eq!(pending.gateway_pin(), &[5; 44]);
    assert_eq!(pending.intent(), intent(2));
    assert_eq!(format!("{pending:?}"), debug);
    assert_eq!(fs::read(&path).unwrap(), encoded);
}
#[test]
fn pending_publication_failure_sends_no_ke3() {
    let fixture = Fixture::new();
    fs::create_dir(fixture.path().join(ENROLLMENT_FILE)).unwrap();
    let gateway = NativeIdentity::restore(key(10)).unwrap();
    let device = NativeIdentity::restore(key(11)).unwrap();
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let invitation = ServerInvitation::register(
        &mut Entropy,
        &code,
        intent(2).invitation(),
        gateway.public_spki(),
    )
    .unwrap();
    let (server_channel, client_channel) = native_channels(&gateway, &device);
    let context = client_channel.pairing_context(intent(2)).unwrap();
    let (attempt, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    let (_, response) = invitation
        .start(
            &mut Entropy,
            &request,
            server_channel.pairing_context(intent(2)).unwrap(),
        )
        .unwrap();
    assert_eq!(
        attempt.finish(&mut Entropy, &code, &response, &context, |authenticated| {
            fixture
                .store
                .save_pending(
                    device.key_material(),
                    &authenticated.gateway_spki(),
                    authenticated.public(),
                    None,
                )
                .map_err(|_| PairingCryptoError::PendingStorage)
        }),
        Err(PairingCryptoError::PendingStorage)
    );
    fs::remove_dir(fixture.path().join(ENROLLMENT_FILE)).unwrap();
    assert!(fixture.store.load_pending().unwrap().is_none());
}

#[cfg(unix)]
#[test]
fn real_pending_save_precedes_ke3_and_survives_restart() {
    fn streams() -> (UnixStream, UnixStream) {
        let (a, b) = UnixStream::pair().unwrap();
        for stream in [&a, &b] {
            stream
                .set_read_timeout(Some(Duration::from_secs(30)))
                .unwrap();
            stream
                .set_write_timeout(Some(Duration::from_secs(30)))
                .unwrap();
        }
        (a, b)
    }
    let fixture = Fixture::new();
    let gateway = NativeIdentity::restore(key(10)).unwrap();
    let device = NativeIdentity::restore(key(11)).unwrap();
    let original_device = device.public_spki();
    let original_gateway = gateway.public_spki();
    let code = ManualCode::parse(b"ABCD2345").unwrap();
    let invitation = ServerInvitation::register(
        &mut Entropy,
        &code,
        intent(2).invitation(),
        original_gateway,
    )
    .unwrap();
    let (a, b) = streams();
    let (server_lengths, client_lengths) = message_lengths();
    let server = thread::spawn(move || {
        let mut transport = CryptoFixtureTransport::new(
            NativeTransport::accept(a, &gateway).unwrap(),
            server_lengths,
        );
        let context = transport.pairing_context(intent(2)).unwrap();
        let request = transport.read_message().unwrap();
        let (_, response) = invitation.start(&mut Entropy, &request, context).unwrap();
        transport.write_message(&response).unwrap();
        // A crash after save but before KE3 creates no mutual confirmation.
        assert!(transport.read_message().is_err());
    });
    let mut transport = CryptoFixtureTransport::new(
        NativeTransport::connect(b, &device, GatewayTrust::ManualBootstrap).unwrap(),
        client_lengths,
    );
    let context = transport.pairing_context(intent(2)).unwrap();
    let (attempt, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    transport.write_message(&request).unwrap();
    let ke3 = attempt
        .finish(
            &mut Entropy,
            &code,
            &transport.read_message().unwrap(),
            &context,
            |authenticated| {
                fixture
                    .store
                    .save_pending(
                        device.key_material(),
                        &authenticated.gateway_spki(),
                        authenticated.public(),
                        None,
                    )
                    .map_err(|_| PairingCryptoError::PendingStorage)
            },
        )
        .unwrap();
    let stored = fixture.store.load_pending().unwrap().unwrap();
    assert_eq!(stored.gateway_pin(), &original_gateway);
    assert_eq!(stored.intent(), intent(2));
    drop(ke3);
    drop(transport);
    drop(device);
    server.join().unwrap();
    let root = fixture.anchor();
    drop(fixture.store);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    let (material, pin, public) = reopened.load_pending().unwrap().unwrap().into_parts();
    let restored = NativeIdentity::restore(material).unwrap();
    assert_eq!(restored.public_spki(), original_device);
    assert_eq!(public, intent(2));
    let gateway = NativeIdentity::restore(key(10)).unwrap();
    let (a, b) = streams();
    let (server_lengths, client_lengths) = message_lengths();
    let server = thread::spawn(move || {
        let mut transport = CryptoFixtureTransport::new(
            NativeTransport::accept(a, &gateway).unwrap(),
            server_lengths,
        );
        assert_eq!(
            transport.device_proof().key().bytes(),
            &original_device[12..]
        );
        transport.write_message(b"pending").unwrap();
    });
    let mut transport = CryptoFixtureTransport::new(
        NativeTransport::connect(b, &restored, GatewayTrust::Pinned(pin)).unwrap(),
        client_lengths,
    );
    assert_eq!(transport.read_message().unwrap(), b"pending");
    server.join().unwrap();
}

#[test]
fn gateway_audit_missing_key_refuses_save_before_effect() {
    for remove_intent in [false, true] {
        let fixture = Fixture::new();
        fixture
            .store
            .save_gateway_key(&key(42), &gateway(), &Time)
            .unwrap();
        let key_path = fixture.path().join(GATEWAY_FILE);
        let intent_path = fixture.path().join(INTENT_FILE);
        let outcome_path = fixture.path().join(OUTCOME_FILE);
        let key_backup = fixture.path().join("fixture-original-key");
        let intent_backup = fixture.path().join("fixture-original-intent");
        assert!(!key_backup.exists());
        assert!(!intent_backup.exists());
        let original_key = fs::read(&key_path).unwrap();
        let original_intent = fs::read(&intent_path).unwrap();
        let original_outcome = fs::read(&outcome_path).unwrap();
        fs::rename(&key_path, &key_backup).unwrap();
        if remove_intent {
            fs::rename(&intent_path, &intent_backup).unwrap();
        }
        assert_eq!(
            fixture.store.save_gateway_key(&key(42), &gateway(), &Time),
            Err(PrivateStateError::Corrupt)
        );
        assert!(!key_path.exists());
        if remove_intent {
            assert!(!intent_path.exists());
        } else {
            assert_eq!(fs::read(&intent_path).unwrap(), original_intent);
        }
        assert_eq!(fs::read(&outcome_path).unwrap(), original_outcome);
        fs::rename(&key_backup, &key_path).unwrap();
        if remove_intent {
            fs::rename(&intent_backup, &intent_path).unwrap();
        }
        assert!(!key_backup.exists());
        assert!(!intent_backup.exists());
        assert_eq!(fs::read(&key_path).unwrap(), original_key);
        assert_eq!(fs::read(&intent_path).unwrap(), original_intent);
        fixture
            .store
            .save_gateway_key(&key(42), &gateway(), &Time)
            .unwrap();
        let root = fixture.anchor();
        drop(fixture.store);
        let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
        assert_eq!(
            reopened
                .restore_gateway_key(&gateway(), &Time)
                .unwrap()
                .unwrap()
                .expose_bytes(),
            &[42; 32]
        );
        reopened
            .save_gateway_key(&key(42), &gateway(), &Time)
            .unwrap();
        assert_eq!(fs::read(&key_path).unwrap(), original_key);
        assert_eq!(fs::read(&intent_path).unwrap(), original_intent);
        assert_eq!(fs::read(&outcome_path).unwrap(), original_outcome);
    }
}

#[test]
fn gateway_audit_rejects_relationship_and_bounds_contradictions() {
    let fixture = Fixture::new();
    fixture
        .store
        .save_gateway_key(&key(42), &gateway(), &Time)
        .unwrap();
    let path = fixture.path().join(OUTCOME_FILE);
    let original = std::fs::read(&path).unwrap();
    let value: serde_json::Value = serde_json::from_slice(&original).unwrap();
    for changed in [
        {
            let mut v = value.clone();
            v["intent"]["operation"][0] =
                serde_json::json!(v["intent"]["operation"][0].as_u64().unwrap() ^ 1);
            v
        },
        {
            let mut v = value.clone();
            v["publicKey"][0] = serde_json::json!(v["publicKey"][0].as_u64().unwrap() ^ 1);
            v
        },
        {
            let mut v = value.clone();
            v["intent"]["gateway"] = serde_json::json!("other");
            v
        },
    ] {
        std::fs::write(&path, serde_json::to_vec(&changed).unwrap()).unwrap();
        assert!(matches!(
            fixture.store.restore_gateway_key(&gateway(), &Time),
            Err(PrivateStateError::Conflict)
        ));
    }
    let mut oversized = original.clone();
    oversized.resize(4096, b' ');
    std::fs::write(&path, oversized).unwrap();
    assert!(matches!(
        fixture.store.restore_gateway_key(&gateway(), &Time),
        Err(PrivateStateError::Corrupt)
    ));
    std::fs::write(&path, &original).unwrap();
    std::fs::remove_file(fixture.path().join(INTENT_FILE)).unwrap();
    assert!(matches!(
        fixture.store.restore_gateway_key(&gateway(), &Time),
        Err(PrivateStateError::Corrupt)
    ));
    // Existing key facts do not permit constructing a new first-publication intent.
    assert_eq!(
        fixture.store.save_gateway_key(&key(42), &gateway(), &Time),
        Err(PrivateStateError::Corrupt)
    );
}

#[test]
fn pending_public_retry_and_reopen_preserve_original_fact() {
    let fixture = Fixture::new();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let original = fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let root = fixture.anchor();
    drop(fixture.store);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    reopened
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let saved = reopened.load_pending().unwrap().unwrap();
    assert_eq!(saved.intent(), intent(2));
    assert_eq!(saved.gateway_pin(), &[5; 44]);
    assert_eq!(saved.key().expose_bytes(), &[7; 32]);
    assert_eq!(
        fs::read(root.join("private").join(ENROLLMENT_FILE)).unwrap(),
        original
    );
}

struct ObstructPublication {
    path: PathBuf,
    call: AtomicUsize,
    at: usize,
}
impl Clock for ObstructPublication {
    fn unix_milliseconds(&self) -> u64 {
        if self.call.fetch_add(1, Ordering::SeqCst) == self.at {
            fs::create_dir(&self.path).unwrap();
        }
        100_000
    }
}
#[test]
fn gateway_publication_failure_preserves_original_operation_and_effect() {
    for (name, at, key_effect) in [
        (INTENT_FILE, 0, GatewayPublicationState::NotPublished),
        (GATEWAY_FILE, 0, GatewayPublicationState::NotPublished),
        (OUTCOME_FILE, 1, GatewayPublicationState::Published),
    ] {
        let fixture = Fixture::new();
        let obstruct = ObstructPublication {
            path: fixture.path().join(name),
            call: AtomicUsize::new(0),
            at,
        };
        let error = fixture
            .store
            .save_gateway_key(&key(42), &gateway(), &obstruct)
            .unwrap_err();
        match (&error, key_effect) {
            (PrivateStateError::Publication(failure), GatewayPublicationState::NotPublished)
            | (
                PrivateStateError::AuditPublication {
                    key: GatewayPublicationState::NotPublished,
                    failure,
                },
                GatewayPublicationState::NotPublished,
            )
            | (
                PrivateStateError::AuditPublication {
                    key: GatewayPublicationState::Published,
                    failure,
                },
                GatewayPublicationState::Published,
            ) => {
                assert_eq!(failure.effect(), PrivatePublicationEffect::NotPublished);
            }
            _ => panic!("unexpected public publication evidence: {error:?}"),
        }
        let diagnostic = format!("{error:?} {error}");
        assert!(!diagnostic.contains(&fixture.path().to_string_lossy().to_string()));
        let key_path = fixture.path().join(GATEWAY_FILE);
        match key_effect {
            GatewayPublicationState::Published => {
                assert!(key_path.is_file());
                let original_key = fs::read(&key_path).unwrap();
                assert_eq!(&original_key[4..], &[42; 32]);
            }
            GatewayPublicationState::NotPublished => assert!(!key_path.is_file()),
        }
        assert!(!fixture.path().join(OUTCOME_FILE).is_file());
        let original_intent = if name == INTENT_FILE {
            assert!(!fixture.path().join(INTENT_FILE).is_file());
            None
        } else {
            let original = fs::read(fixture.path().join(INTENT_FILE)).unwrap();
            let intent: serde_json::Value = serde_json::from_slice(&original).unwrap();
            assert_eq!(intent["gateway"], "gateway-1");
            assert_eq!(intent["before"], "absent");
            assert_eq!(intent["cause"], "firstPublication");
            assert_eq!(intent["initiator"], "system");
            Some(original)
        };
        fs::remove_dir(&obstruct.path).unwrap();
        fixture
            .store
            .save_gateway_key(&key(42), &gateway(), &Time)
            .unwrap();
        let retry_intent = fs::read(fixture.path().join(INTENT_FILE)).unwrap();
        if let Some(original) = original_intent {
            assert_eq!(retry_intent, original);
        }
        let intent: serde_json::Value = serde_json::from_slice(&retry_intent).unwrap();
        assert_eq!(intent["gateway"], "gateway-1");
        assert_eq!(intent["before"], "absent");
        assert_eq!(intent["cause"], "firstPublication");
        assert_eq!(intent["initiator"], "system");
        let original_outcome = fs::read(fixture.path().join(OUTCOME_FILE)).unwrap();
        let root = fixture.anchor();
        drop(fixture.store);
        let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
        assert_eq!(
            reopened
                .restore_gateway_key(&gateway(), &Time)
                .unwrap()
                .unwrap()
                .expose_bytes(),
            &[42; 32]
        );
        assert_eq!(
            fs::read(root.join("private").join(INTENT_FILE)).unwrap(),
            retry_intent
        );
        assert_eq!(
            fs::read(root.join("private").join(OUTCOME_FILE)).unwrap(),
            original_outcome
        );
    }
}
/// Slice 2b row A10: the issued credential replaces the exact pending record
/// in one publication, keeping its key, pin and correlation; an exact retry
/// acknowledges it, and nothing else can replace or follow it.
#[test]
fn issued_credential_replaces_pending_once_and_reopens() {
    let fixture = Fixture::new();
    let credential = CredentialId::new("device-1").unwrap();
    let receiver = ResourceId::new("receiver-1").unwrap();
    // No pending record: nothing to replace.
    assert_eq!(
        fixture
            .store
            .save_credential(&credential, &receiver, intent(2)),
        Err(PrivateStateError::Conflict)
    );
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    // Another enrollment's credential does not replace this one.
    assert_eq!(
        fixture
            .store
            .save_credential(&credential, &receiver, intent(4)),
        Err(PrivateStateError::Conflict)
    );
    assert!(fixture.store.load_credential().unwrap().is_none());
    fixture
        .store
        .save_credential(&credential, &receiver, intent(2))
        .unwrap();
    assert!(fixture.store.load_pending().unwrap().is_none());
    let saved = fixture.store.load_credential().unwrap().unwrap();
    assert_eq!(saved.key().expose_bytes(), &[7; 32]);
    assert_eq!(saved.gateway_pin(), &[5; 44]);
    assert_eq!(saved.intent(), intent(2));
    assert_eq!(saved.credential(), &credential);
    assert_eq!(saved.receiver(), &receiver);
    assert!(format!("{saved:?}").contains("[REDACTED]"));
    let encoded = fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap();
    // Exact retry acknowledges; another credential or receiver conflicts.
    fixture
        .store
        .save_credential(&credential, &receiver, intent(2))
        .unwrap();
    for (other_credential, other_receiver) in [
        (CredentialId::new("device-2").unwrap(), receiver.clone()),
        (credential.clone(), ResourceId::new("receiver-2").unwrap()),
    ] {
        assert_eq!(
            fixture
                .store
                .save_credential(&other_credential, &other_receiver, intent(2)),
            Err(PrivateStateError::Conflict)
        );
    }
    // A new enrollment cannot overwrite the issued credential.
    for expected in [None, Some(intent(2))] {
        assert_eq!(
            fixture
                .store
                .save_pending(&key(7), &[5; 44], intent(3), expected),
            Err(PrivateStateError::Conflict)
        );
    }
    assert_eq!(
        fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap(),
        encoded
    );
    let root = fixture.anchor();
    drop(fixture.store);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    let restored = reopened.load_credential().unwrap().unwrap();
    assert_eq!(restored.credential(), &credential);
    assert_eq!(restored.receiver(), &receiver);
    // A truncated or padded credential record is corrupt, never a pending one.
    let path = root.join("private").join(ENROLLMENT_FILE);
    for bytes in [encoded[..encoded.len() - 1].to_vec(), {
        let mut padded = encoded.clone();
        padded.push(0);
        padded
    }] {
        fs::remove_file(&path).unwrap();
        fs::write(&path, bytes).unwrap();
        #[cfg(unix)]
        fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
        assert_eq!(
            reopened.load_credential().map(|saved| saved.is_some()),
            Err(PrivateStateError::Corrupt)
        );
        assert_eq!(
            reopened.load_pending().map(|saved| saved.is_some()),
            Err(PrivateStateError::Corrupt)
        );
    }
}

/// Slice 2b row A14: an ended enrollment's record, pending or credential, is
/// removed for exactly its own enrollment; absence is already ended, and a
/// new enrollment can then be saved.
#[test]
fn ended_enrollment_is_removed_exactly() {
    let fixture = Fixture::new();
    assert_eq!(fixture.store.end_enrollment(intent(2)), Ok(()));
    for issued in [false, true] {
        fixture
            .store
            .save_pending(&key(7), &[5; 44], intent(2), None)
            .unwrap();
        if issued {
            fixture
                .store
                .save_credential(
                    &CredentialId::new("device-1").unwrap(),
                    &ResourceId::new("receiver-1").unwrap(),
                    intent(2),
                )
                .unwrap();
        }
        let before = fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap();
        assert_eq!(
            fixture.store.end_enrollment(intent(4)),
            Err(PrivateStateError::Conflict)
        );
        assert_eq!(
            fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap(),
            before
        );
        fixture.store.end_enrollment(intent(2)).unwrap();
        assert!(!fixture.path().join(ENROLLMENT_FILE).exists());
        assert!(fixture.store.load_pending().unwrap().is_none());
        assert!(fixture.store.load_credential().unwrap().is_none());
        assert_eq!(fixture.store.end_enrollment(intent(2)), Ok(()));
    }
    fixture
        .store
        .save_pending(&key(8), &[6; 44], intent(3), None)
        .unwrap();
    assert_eq!(
        fixture.store.load_pending().unwrap().unwrap().intent(),
        intent(3)
    );
}
