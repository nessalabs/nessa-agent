use super::gateway_audit::{INTENT_FILE, OUTCOME_FILE};
use super::*;
#[cfg(unix)]
use crate::adapters::pairing::tests::io::{message_lengths, CryptoFixtureTransport};
use crate::adapters::pairing::{
    tests::Entropy, ClientAttempt, ManualCode, NativeIdentity, PairingContext, PairingCryptoError,
    ServerInvitation,
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
    sync::{
        atomic::{AtomicUsize, Ordering},
        mpsc,
    },
    thread,
    time::Duration,
};
use tempfile::TempDir;

struct Fixture {
    root: TempDir,
    store: FilePairingState,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().canonicalize().unwrap();
        #[cfg(unix)]
        {
            std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        }
        nessa_local_storage::create_directory_beneath(&path, Path::new("private")).unwrap();
        let store = FilePairingState::open(&path, Path::new("private")).unwrap();
        Self { root, store }
    }
    fn path(&self) -> PathBuf {
        self.root.path().canonicalize().unwrap().join("private")
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
    let root = fixture.root.path().canonicalize().unwrap();
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
        FilePairingState::open(
            &fixture.root.path().canonicalize().unwrap(),
            Path::new("private")
        ),
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
fn pending_uncertain_publication_preserves_fact() {
    let fixture = Fixture::new();
    fixture.store.fault.store(3, Ordering::Release);
    assert_eq!(
        fixture
            .store
            .save_pending(&key(7), &[5; 44], intent(2), None),
        Err(PrivateStateError::Uncertain)
    );
    assert_eq!(
        fixture.store.load_pending().unwrap().unwrap().intent(),
        intent(2)
    );
    fixture.store.fault.store(2, Ordering::Release);
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let acknowledged_after_rename = Fixture::new();
    acknowledged_after_rename
        .store
        .fault
        .store(2, Ordering::Release);
    acknowledged_after_rename
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    assert_eq!(
        acknowledged_after_rename
            .store
            .load_pending()
            .unwrap()
            .unwrap()
            .intent(),
        intent(2)
    );

    let root = fixture.root.path().canonicalize().unwrap();
    drop(fixture.store);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    assert_eq!(
        reopened
            .load_pending()
            .unwrap()
            .unwrap()
            .key()
            .expose_bytes(),
        &[7; 32]
    );
    assert_eq!(
        reopened.load_pending().unwrap().unwrap().intent(),
        intent(2)
    );
}
#[test]
fn private_state_is_bounded_and_redacted() {
    let fixture = Fixture::new();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let path = fixture.path().join(PENDING_FILE);
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
        let path = fixture.path().join(PENDING_FILE);
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
        let path = fixture.path().join(PENDING_FILE);
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
    let root = fixture.root.path().canonicalize().unwrap();
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
    fixture.store.fault.store(1, Ordering::Release);
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
    let context = PairingContext::from_transport(
        intent(2),
        gateway.public_spki(),
        device.public_spki(),
        [4; 32],
    )
    .unwrap();
    let (attempt, request) = ClientAttempt::start(&mut Entropy, &code).unwrap();
    let (_, response) = invitation
        .start(
            &mut Entropy,
            &request,
            PairingContext::from_transport(
                intent(2),
                gateway.public_spki(),
                device.public_spki(),
                [4; 32],
            )
            .unwrap(),
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
    let root = fixture.root.path().canonicalize().unwrap();
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
fn gateway_audit_failure_keeps_original_operation() {
    let fixture = Fixture::new();
    fixture.store.fault.store(6, Ordering::Release);
    assert_eq!(
        fixture.store.save_gateway_key(&key(42), &gateway(), &Time),
        Err(PrivateStateError::AuditUnavailable(
            GatewayPublicationState::NotPublished
        ))
    );
    assert!(!fixture.path().join(GATEWAY_FILE).exists());
    assert!(!fixture.path().join(INTENT_FILE).exists());
    fixture.store.fault.store(7, Ordering::Release);
    assert_eq!(
        fixture.store.save_gateway_key(&key(42), &gateway(), &Time),
        Err(PrivateStateError::AuditUnavailable(
            GatewayPublicationState::Published
        ))
    );
    let original = std::fs::read(fixture.path().join(INTENT_FILE)).unwrap();
    let intent: serde_json::Value = serde_json::from_slice(&original).unwrap();
    assert_eq!(intent["gateway"], "gateway-1");
    assert_eq!(intent["before"], "absent");
    assert_eq!(intent["cause"], "firstPublication");
    assert_eq!(intent["initiator"], "system");
    assert!(intent.get("seed").is_none());
    assert!(!fixture.path().join(OUTCOME_FILE).exists());
    assert_eq!(
        fixture.store.save_gateway_key(&key(43), &gateway(), &Time),
        Err(PrivateStateError::Conflict)
    );
    assert!(matches!(
        fixture.store.restore_gateway_key(&gateway(), &Time),
        Err(PrivateStateError::AuditUnavailable(
            GatewayPublicationState::Published
        ))
    ));
    fixture.store.fault.store(0, Ordering::Release);
    assert_eq!(
        fixture
            .store
            .restore_gateway_key(&gateway(), &Time)
            .unwrap()
            .unwrap()
            .expose_bytes(),
        &[42; 32]
    );
    fixture
        .store
        .save_gateway_key(&key(42), &gateway(), &Time)
        .unwrap();
    assert_eq!(
        std::fs::read(fixture.path().join(INTENT_FILE)).unwrap(),
        original
    );
    let outcome: serde_json::Value =
        serde_json::from_slice(&std::fs::read(fixture.path().join(OUTCOME_FILE)).unwrap()).unwrap();
    assert_eq!(outcome["intent"], intent);
    assert_eq!(outcome["after"], "published");
    assert!(outcome.get("seed").is_none());
    assert!(matches!(
        fixture
            .store
            .restore_gateway_key(&AudienceId::new("other").unwrap(), &Time),
        Err(PrivateStateError::Conflict)
    ));
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
        let root = fixture.root.path().canonicalize().unwrap();
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
fn gateway_audit_restart_preserves_target_and_operation() {
    let fixture = Fixture::new();
    // This fails the key-file effect only after its original intent was acknowledged.
    // Fault injection is phase-specific to avoid claiming an unobserved publication.
    fixture.store.fault.store(8, Ordering::Release);
    assert_eq!(
        fixture.store.save_gateway_key(&key(42), &gateway(), &Time),
        Err(PrivateStateError::Unavailable)
    );
    let original = std::fs::read(fixture.path().join(INTENT_FILE)).unwrap();
    assert!(!fixture.path().join(GATEWAY_FILE).exists());
    let root = fixture.root.path().canonicalize().unwrap();
    drop(fixture.store);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    assert!(reopened
        .restore_gateway_key(&gateway(), &Time)
        .unwrap()
        .is_none());
    reopened.fault.store(7, Ordering::Release);
    assert_eq!(
        reopened.save_gateway_key(&key(42), &gateway(), &Time),
        Err(PrivateStateError::AuditUnavailable(
            GatewayPublicationState::Published
        ))
    );
    drop(reopened);
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
        std::fs::read(fixture.root.path().join("private").join(INTENT_FILE)).unwrap(),
        original
    );
    let outcome = std::fs::read(fixture.root.path().join("private").join(OUTCOME_FILE)).unwrap();
    // A lost acknowledgement and another restart restore the same immutable receipt.
    drop(reopened);
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    reopened.restore_gateway_key(&gateway(), &Time).unwrap();
    assert_eq!(
        std::fs::read(fixture.root.path().join("private").join(OUTCOME_FILE)).unwrap(),
        outcome
    );
    std::fs::remove_file(fixture.root.path().join("private").join(GATEWAY_FILE)).unwrap();
    assert!(matches!(
        reopened.restore_gateway_key(&gateway(), &Time),
        Err(PrivateStateError::Corrupt)
    ));
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

fn publish_fixture(store: &FilePairingState, name: &str, bytes: &[u8]) -> PublishedPrivateFile {
    let mut temporary = store.directory.reserve_temp().unwrap();
    temporary.as_file_mut().write_all(bytes).unwrap();
    temporary.publish_new(OsStr::new(name)).unwrap()
}
#[test]
fn live_reconcile_accepts_original_and_correlates_name_bytes() {
    let fixture = Fixture::new();
    let published = publish_fixture(&fixture.store, "original", b"exact bytes");
    fixture
        .store
        .reconcile_live("original", b"exact bytes", published)
        .unwrap();
    let published = publish_fixture(&fixture.store, "wrong-name", b"exact bytes");
    assert_eq!(
        fixture
            .store
            .reconcile_live("other", b"exact bytes", published),
        Err(PrivateStorageFailure::UnsafeStorage)
    );
    let published = publish_fixture(&fixture.store, "wrong-bytes", b"other bytes");
    assert_eq!(
        fixture
            .store
            .reconcile_live("wrong-bytes", b"exact bytes", published),
        Err(PrivateStorageFailure::UnsafeStorage)
    );
}
#[cfg(unix)]
#[test]
fn live_reconcile_keeps_original_identity() {
    let fixture = Fixture::new();
    let published = publish_fixture(&fixture.store, "original", b"exact bytes");
    std::fs::rename(
        fixture.path().join("original"),
        fixture.path().join("moved"),
    )
    .unwrap();
    let foreign = publish_fixture(&fixture.store, "original", b"exact bytes");
    let synced = Arc::new(AtomicUsize::new(0));
    let observed = synced.clone();
    *fixture.store.after_file_sync.lock().unwrap() = Some(Arc::new(move || {
        observed.fetch_add(1, Ordering::SeqCst);
    }));
    assert_eq!(
        fixture
            .store
            .reconcile_live("original", b"exact bytes", published),
        Err(PrivateStorageFailure::UnsafeStorage)
    );
    assert_eq!(synced.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(fixture.path().join("moved")).unwrap(),
        b"exact bytes"
    );
    assert!(fixture
        .store
        .directory
        .named_file_is(OsStr::new("original"), foreign.as_file())
        .unwrap());
    // Restart knows only the current exact bytes and current binding, not the old native object.
    fixture.store.reconcile("original", b"exact bytes").unwrap();
    assert_eq!(synced.load(Ordering::SeqCst), 1);
}
#[test]
#[cfg(unix)]
fn live_reconcile_refuses_identity_replacement_after_original_sync() {
    for replace in [true, false] {
        let fixture = Fixture::new();
        let published = publish_fixture(&fixture.store, "original", b"exact bytes");
        let original = published.as_file().try_clone().unwrap();
        let (synced, reached) = mpsc::sync_channel(1);
        let (release, resume) = mpsc::sync_channel(1);
        let resume = Mutex::new(resume);
        *fixture.store.after_file_sync.lock().unwrap() = Some(Arc::new(move || {
            synced.send(()).unwrap();
            resume
                .lock()
                .unwrap()
                .recv_timeout(Duration::from_secs(30))
                .unwrap();
        }));
        let (result, replacement) = thread::scope(|scope| {
            let store = &fixture.store;
            let worker =
                scope.spawn(move || store.reconcile_live("original", b"exact bytes", published));
            reached.recv_timeout(Duration::from_secs(30)).unwrap();
            assert!(matches!(
                FilePairingState::open(
                    &fixture.root.path().canonicalize().unwrap(),
                    Path::new("private")
                ),
                Err(PrivateStateError::Locked)
            ));
            let replacement = if replace {
                std::fs::rename(
                    fixture.path().join("original"),
                    fixture.path().join("moved"),
                )
                .unwrap();
                Some(publish_fixture(store, "original", b"exact bytes"))
            } else {
                None
            };
            release.send(()).unwrap();
            (worker.join().unwrap(), replacement)
        });
        *fixture.store.after_file_sync.lock().unwrap() = None;
        assert_eq!(
            result,
            if replace {
                Err(PrivateStorageFailure::UnsafeStorage)
            } else {
                Ok(())
            }
        );
        assert_eq!(
            std::fs::read(fixture.path().join("original")).unwrap(),
            b"exact bytes"
        );
        if let Some(replacement) = replacement {
            assert_eq!(
                std::fs::read(fixture.path().join("moved")).unwrap(),
                b"exact bytes"
            );
            assert!(fixture
                .store
                .directory
                .named_file_is(OsStr::new("moved"), &original)
                .unwrap());
            assert!(fixture
                .store
                .directory
                .named_file_is(OsStr::new("original"), replacement.as_file())
                .unwrap());
        } else {
            assert!(fixture
                .store
                .directory
                .named_file_is(OsStr::new("original"), &original)
                .unwrap());
        }
        // Restart may acknowledge the current object, not the retired original binding.
        fixture.store.reconcile("original", b"exact bytes").unwrap();
    }
}

#[test]
fn missing_reconcile_creates_nothing() {
    let fixture = Fixture::new();
    assert_eq!(
        fixture.store.reconcile("missing", b"exact bytes"),
        Err(PrivateStateError::Uncertain)
    );
    assert!(!fixture.path().join("missing").exists());
    let original = publish_fixture(&fixture.store, "original", b"exact bytes");
    assert!(fixture
        .store
        .directory
        .named_file_is(OsStr::new("original"), original.as_file())
        .unwrap());
    fixture
        .store
        .reconcile_live("original", b"exact bytes", original)
        .unwrap();
    // Restart has no retained publication handle; open the existing file writable.
    fixture.store.reconcile("original", b"exact bytes").unwrap();
}
#[test]
fn publication_refusal_without_cleanup_failure() {
    let fixture = Fixture::new();
    let mut temporary = fixture.store.directory.reserve_temp().unwrap();
    let reservation = temporary.name().to_owned();
    temporary.as_file_mut().write_all(b"bytes").unwrap();
    let failure = temporary
        .publish_new(OsStr::new("invalid/name"))
        .unwrap_err();
    let error = fixture
        .store
        .publication_failure("invalid/name", b"bytes", failure)
        .unwrap_err();
    let PrivateStateError::Publication(error) = error else {
        panic!("publication evidence missing")
    };
    assert_eq!(error.step(), PrivatePublicationStep::ValidateDestination);
    assert_eq!(error.effect(), PrivatePublicationEffect::NotPublished);
    assert_eq!(error.cleanup(), None);
    assert_eq!(error.reconciliation(), None);
    assert!(!fixture.path().join(reservation).exists());
}
#[cfg(unix)]
#[test]
fn publication_refusal_retains_independent_cleanup() {
    let fixture = Fixture::new();
    let mut temporary = fixture.store.directory.reserve_temp().unwrap();
    temporary
        .as_file_mut()
        .write_all(b"original bytes")
        .unwrap();
    let reservation = temporary.name().to_owned();
    std::fs::rename(
        fixture.path().join(&reservation),
        fixture.path().join("moved"),
    )
    .unwrap();
    let mut witness = fixture
        .store
        .directory
        .open_file(&reservation, OpenMode::CreateNew)
        .unwrap();
    witness.write_all(b"foreign witness").unwrap();
    let failure = temporary
        .publish_new(OsStr::new("destination"))
        .unwrap_err();
    let error = fixture
        .store
        .publication_failure("destination", b"original bytes", failure)
        .unwrap_err();
    let PrivateStateError::Publication(error) = error else {
        panic!("publication evidence missing")
    };
    assert_eq!(error.step(), PrivatePublicationStep::ValidateReservation);
    assert_eq!(error.primary(), PrivateStorageFailure::UnsafeStorage);
    assert_eq!(error.cleanup(), Some(PrivateStorageFailure::UnsafeStorage));
    assert_eq!(error.effect(), PrivatePublicationEffect::NotPublished);
    assert_eq!(error.reconciliation(), None);
    assert!(!fixture.path().join("destination").exists());
    assert_eq!(
        std::fs::read(fixture.path().join(reservation)).unwrap(),
        b"foreign witness"
    );
    assert_eq!(
        std::fs::read(fixture.path().join("moved")).unwrap(),
        b"original bytes"
    );
}
#[test]
fn audit_publication_keeps_both_effect_meanings() {
    for key in [
        GatewayPublicationState::NotPublished,
        GatewayPublicationState::Published,
    ] {
        let fixture = Fixture::new();
        let temporary = fixture.store.directory.reserve_temp().unwrap();
        let failure = temporary
            .publish_new(OsStr::new("invalid/name"))
            .unwrap_err();
        let error = fixture
            .store
            .publication_failure("invalid/name", b"bytes", failure)
            .unwrap_err();
        let PrivateStateError::Publication(original) = error else {
            panic!("publication evidence missing")
        };
        let mapped = gateway_audit::audit_error(error, key);
        assert_eq!(
            mapped,
            PrivateStateError::AuditPublication {
                key,
                failure: original
            }
        );
        assert_eq!(original.effect(), PrivatePublicationEffect::NotPublished);
        assert_eq!(gateway_audit::audit_error(mapped, key), mapped);
    }
}
#[test]
fn publication_error_is_redacted() {
    let fixture = Fixture::new();
    let mut temporary = fixture.store.directory.reserve_temp().unwrap();
    temporary
        .as_file_mut()
        .write_all(b"private seed bytes")
        .unwrap();
    let failure = temporary
        .publish_new(OsStr::new("private/path"))
        .unwrap_err();
    let error = fixture
        .store
        .publication_failure("private/path", b"private seed bytes", failure)
        .unwrap_err();
    let diagnostic = format!("{error:?} {error}");
    assert!(!diagnostic.contains("private seed bytes"));
    assert!(!diagnostic.contains("private/path"));
    assert!(!diagnostic.contains(&fixture.path().to_string_lossy().to_string()));
}

#[test]
fn publication_occupied_destination_preserves_original_and_stage() {
    let fixture = Fixture::new();
    let original = publish_fixture(&fixture.store, "destination", b"original bytes");
    let mut temporary = fixture.store.directory.reserve_temp().unwrap();
    let reservation = temporary.name().to_owned();
    temporary.as_file_mut().write_all(b"new bytes").unwrap();
    let failure = temporary
        .publish_new(OsStr::new("destination"))
        .unwrap_err();
    let error = fixture
        .store
        .publication_failure("destination", b"new bytes", failure)
        .unwrap_err();
    let PrivateStateError::Publication(error) = error else {
        panic!("publication evidence missing")
    };
    assert_eq!(error.step(), PrivatePublicationStep::Rename);
    assert_eq!(error.primary(), PrivateStorageFailure::Unavailable);
    assert_eq!(error.cleanup(), None);
    assert_eq!(error.effect(), PrivatePublicationEffect::NotPublished);
    assert_eq!(error.reconciliation(), None);
    assert!(fixture
        .store
        .directory
        .named_file_is(OsStr::new("destination"), original.as_file())
        .unwrap());
    assert_eq!(
        std::fs::read(fixture.path().join("destination")).unwrap(),
        b"original bytes"
    );
    assert!(!fixture.path().join(reservation).exists());
}

#[test]
fn pending_live_lost_acknowledgement_reconciles() {
    let fixture = Fixture::new();
    fixture.store.fault.store(2, Ordering::Release);
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let restored = fixture.store.load_pending().unwrap().unwrap();
    assert_eq!(restored.intent(), intent(2));
    assert_eq!(restored.gateway_pin(), &[5; 44]);
    assert_eq!(restored.key().expose_bytes(), key(7).expose_bytes());
}
#[cfg(unix)]
#[test]
fn live_reconcile_refuses_changed_lock_binding() {
    let fixture = Fixture::new();
    let published = publish_fixture(&fixture.store, "original", b"exact bytes");
    std::fs::rename(
        fixture.path().join(LOCK_FILE),
        fixture.path().join("original-lock"),
    )
    .unwrap();
    let replacement = fixture
        .store
        .directory
        .open_file(OsStr::new(LOCK_FILE), OpenMode::CreateNew)
        .unwrap();
    assert_eq!(
        fixture
            .store
            .reconcile_live("original", b"exact bytes", published),
        Err(PrivateStorageFailure::UnsafeStorage)
    );
    assert!(fixture
        .store
        .directory
        .named_file_is(OsStr::new(LOCK_FILE), &replacement)
        .unwrap());
    assert_eq!(
        std::fs::read(fixture.path().join("original")).unwrap(),
        b"exact bytes"
    );
}

#[test]
fn restart_acknowledgement_reopens_writable_existing_file_and_reflushes() {
    let fixture = Fixture::new();
    let published = publish_fixture(&fixture.store, "restart", b"exact bytes");
    drop(published);
    drop(fixture.store);
    let root = fixture.root.path().canonicalize().unwrap();
    let reopened = FilePairingState::open(&root, Path::new("private")).unwrap();
    let mut original = reopened.reopen_for_acknowledgement("restart").unwrap();
    original.seek(SeekFrom::Start(0)).unwrap();
    original.write_all(b"exact bytes").unwrap();
    original.sync_all().unwrap();
    reopened
        .acknowledge("restart", b"exact bytes", &original)
        .unwrap();
    reopened.reconcile("restart", b"exact bytes").unwrap();
    assert!(matches!(
        reopened.reopen_for_acknowledgement("absent"),
        Err(PrivateStateError::Uncertain)
    ));
    assert!(!root.join("private").join("absent").exists());
}

#[test]
fn failed_acknowledgement_preserves_original_published_handle_for_reconciliation() {
    let fixture = Fixture::new();
    let published = publish_fixture(&fixture.store, "original", b"exact bytes");
    assert_eq!(
        fixture
            .store
            .acknowledge("original", b"wrong bytes", published.as_file()),
        Err(PrivateStorageFailure::UnsafeStorage)
    );
    assert!(fixture
        .store
        .directory
        .named_file_is(OsStr::new("original"), published.as_file())
        .unwrap());
    fixture
        .store
        .acknowledge("original", b"exact bytes", published.as_file())
        .unwrap();
    fixture
        .store
        .reconcile_live("original", b"exact bytes", published)
        .unwrap();
}
