//! Synthetic retained-file recovery states from the C324 ordering table.
use super::*;

fn publish(fixture: &Fixture, bytes: &[u8], replace: bool) -> PublishedPrivateFile {
    let mut temporary = fixture.store.directory.reserve_temp().unwrap();
    temporary.as_file_mut().write_all(bytes).unwrap();
    if replace {
        temporary.replace(OsStr::new(ENROLLMENT_FILE)).unwrap()
    } else {
        temporary.publish_new(OsStr::new(ENROLLMENT_FILE)).unwrap()
    }
}

fn assert_only_record_and_lock(fixture: &Fixture, expected: &[u8]) {
    assert_eq!(
        fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap(),
        expected
    );
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 2);
}

#[test]
fn live_reconciliation_accepts_original_publication() {
    for replace in [false, true] {
        let fixture = Fixture::new();
        if replace {
            fixture
                .store
                .save_pending(&key(7), &[5; 44], intent(2), None)
                .unwrap();
        }
        let bytes = encode_pending(&key(7), &[5; 44], intent(4));
        let published = publish(&fixture, &bytes, replace);
        assert!(fixture
            .store
            .directory
            .named_file_is(published.name(), published.as_file())
            .unwrap());
        fixture
            .store
            .reconcile_live(ENROLLMENT_FILE, &bytes, published)
            .unwrap();
        assert_only_record_and_lock(&fixture, &bytes);
    }
}

#[test]
fn live_reconciliation_refuses_same_bytes_at_a_different_identity() {
    let fixture = Fixture::new();
    let bytes = encode_pending(&key(7), &[5; 44], intent(2));
    let original = publish(&fixture, &bytes, false);
    let mut witness = original.as_file().try_clone().unwrap();
    let replacement = publish(&fixture, &bytes, true);
    assert_ne!(original.identity(), replacement.identity());
    assert_eq!(
        fixture
            .store
            .reconcile_live(ENROLLMENT_FILE, &bytes, original),
        Err(PrivateStorageFailure::UnsafeStorage)
    );
    assert!(fixture
        .store
        .directory
        .named_file_is(replacement.name(), replacement.as_file())
        .unwrap());
    let mut actual = Vec::new();
    witness.seek(SeekFrom::Start(0)).unwrap();
    witness.read_to_end(&mut actual).unwrap();
    assert_eq!(actual, bytes.as_slice());
    assert_only_record_and_lock(&fixture, &bytes);
    drop(replacement);
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
}

#[test]
fn live_reconciliation_refuses_wrong_bytes_or_name() {
    for wrong_name in [false, true] {
        let fixture = Fixture::new();
        let bytes = encode_pending(&key(7), &[5; 44], intent(2));
        let published = publish(&fixture, &bytes, false);
        let expected = encode_pending(&key(8), &[5; 44], intent(2));
        let (name, expected) = if wrong_name {
            (GATEWAY_FILE, bytes.as_slice())
        } else {
            (ENROLLMENT_FILE, expected.as_slice())
        };
        assert_eq!(
            fixture.store.reconcile_live(name, expected, published),
            Err(PrivateStorageFailure::UnsafeStorage)
        );
        assert_only_record_and_lock(&fixture, &bytes);
        fixture
            .store
            .save_pending(&key(7), &[5; 44], intent(2), None)
            .unwrap();
    }
}

#[test]
fn restart_reconciliation_reflushes_existing_record() {
    let fixture = Fixture::new();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(4), Some(intent(2)))
        .unwrap();
    let bytes = fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap();
    let anchor = fixture.anchor();
    drop(fixture.store);
    let store = FilePairingState::open(&anchor, Path::new("private")).unwrap();
    // Exercise the actual restart helper's access rights, including native
    // FlushFileBuffers on Windows, rather than opening a separate test handle.
    let reflush = store
        .reopen_for_acknowledgement(ENROLLMENT_FILE)
        .unwrap()
        .sync_all();
    if let Err(error) = &reflush {
        eprintln!("::error title=C324-4 native reflush assertion::C324_REFLUSH_ASSERTION_FAILED: restart_reconciliation_reflushes_existing_record expected Ok(()) from the reopened original file; actual {error:?}");
    }
    reflush.unwrap();
    store
        .save_pending(&key(7), &[5; 44], intent(4), Some(intent(2)))
        .unwrap();
    let pending = store.load_pending().unwrap().unwrap();
    assert_eq!(pending.key().expose_bytes(), &[7; 32]);
    assert_eq!(pending.gateway_pin(), &[5; 44]);
    assert_eq!(pending.intent(), intent(4));
    assert_eq!(
        store.save_pending(&key(7), &[5; 44], intent(6), Some(intent(2))),
        Err(PrivateStateError::Conflict)
    );
    assert_eq!(
        fs::read(anchor.join("private").join(ENROLLMENT_FILE)).unwrap(),
        bytes
    );
    assert_eq!(fs::read_dir(anchor.join("private")).unwrap().count(), 2);
    eprintln!("::notice title=C324-4 native recovery passed::C324_REFLUSH_AND_RETRY_PASSED: exact original key/pin/attempt and stale-CAS refusal verified after restart");
}

#[test]
fn restart_reconciliation_does_not_create_a_missing_record() {
    let fixture = Fixture::new();
    let bytes = encode_pending(&key(7), &[5; 44], intent(2));
    assert_eq!(
        fixture.store.reconcile(ENROLLMENT_FILE, &bytes),
        Err(PrivateStateError::Uncertain)
    );
    assert!(!fixture.path().join(ENROLLMENT_FILE).exists());
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 1);
}

#[cfg(windows)]
#[test]
fn windows_pending_replacement_recovers_after_pinned_destination() {
    let fixture = Fixture::new();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(2), None)
        .unwrap();
    let before = fs::read(fixture.path().join(ENROLLMENT_FILE)).unwrap();
    let pinned = fixture
        .store
        .directory
        .open_file(OsStr::new(ENROLLMENT_FILE), OpenMode::ReadWrite)
        .unwrap();
    let error = fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(4), Some(intent(2)))
        .unwrap_err();
    let PrivateStateError::Publication(failure) = error else {
        panic!("expected typed publication failure: {error:?}")
    };
    assert_eq!(failure.step(), PrivatePublicationStep::Rename);
    assert_eq!(failure.effect(), PrivatePublicationEffect::NotPublished);
    assert!(failure.cleanup().is_none());
    assert!(failure.reconciliation().is_none());
    assert_only_record_and_lock(&fixture, &before);
    assert_eq!(
        fixture.store.load_pending().unwrap().unwrap().intent(),
        intent(2)
    );
    drop(pinned);
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(4), Some(intent(2)))
        .unwrap();
    fixture
        .store
        .save_pending(&key(7), &[5; 44], intent(4), Some(intent(2)))
        .unwrap();
    let saved = fixture.store.load_pending().unwrap().unwrap();
    assert_eq!(saved.intent(), intent(4));
    assert_eq!(saved.key().expose_bytes(), &[7; 32]);
    assert_eq!(saved.gateway_pin(), &[5; 44]);
    assert_eq!(fs::read_dir(fixture.path()).unwrap().count(), 2);
}
