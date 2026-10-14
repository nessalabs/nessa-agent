//! Receipt capability agreement; these tests assert no persistence outcome.
use super::*;
use nessa_sync::replication::domain::Id;

fn snapshot(session: &str, incarnation: [u8; 16], revision: u64) -> SessionSaveGeneration {
    SessionSaveGeneration::snapshot(SessionId::new(session).unwrap(), incarnation, revision)
}
fn record(base: u64, generation: u64) -> SessionSaveGeneration {
    SessionSaveGeneration::record(Id::new("conversation").unwrap(), [1; 16], base, generation)
}

#[test]
fn receipt_requires_original_scope_and_entire_unit_count_before_next_work() {
    let original = snapshot("conversation", [1; 16], 4);
    let next = snapshot("conversation", [1; 16], 5);
    let receipt = SessionSaveReceipt::new(original.clone(), next.clone(), 2, [3; 32]).unwrap();
    assert_eq!(receipt.next_for(&original, 2).unwrap(), next);
    for units in [1, 3] {
        assert_eq!(
            receipt.next_for(&original, units),
            Err(StorageError::IdentityMismatch)
        );
    }
    assert_eq!(
        receipt.next_for(&snapshot("conversation", [2; 16], 4), 2),
        Err(StorageError::IdentityMismatch)
    );
    assert!(SessionSaveReceipt::new(original.clone(), original.clone(), 2, [3; 32]).is_err());
    assert!(SessionSaveReceipt::new(original, next, 0, [3; 32]).is_err());
}

#[test]
fn record_receipt_cannot_skip_generation_or_cross_reset_incarnation() {
    let original = record(8, 3);
    assert_eq!((original.base(), original.generation()), (8, 3));
    assert!(SessionSaveReceipt::new(original.clone(), record(10, 5), 1, [3; 32]).is_err());
    let replacement =
        SessionSaveGeneration::record(Id::new("conversation").unwrap(), [2; 16], 10, 4);
    assert!(SessionSaveReceipt::new(original, replacement, 1, [3; 32]).is_err());
    assert!(SessionSaveReceipt::new(record(8, u64::MAX), record(10, 0), 1, [3; 32]).is_err());
}

#[test]
fn immutable_unit_and_snapshot_receipt_constructor_refuse_each_invalid_capability() {
    assert!(SessionSaveUnit::new(Vec::new()).is_err());
    let original = snapshot("conversation", [1; 16], 4);
    let next = snapshot("conversation", [1; 16], 5);
    let receipt = SessionSaveReceipt::new(original.clone(), next, 1, [3; 32]).unwrap();
    assert_eq!(
        receipt.next_for(&original, 1).unwrap(),
        snapshot("conversation", [1; 16], 5)
    );
    for revision in [3, 5] {
        assert_eq!(
            receipt.next_for(&snapshot("conversation", [1; 16], revision), 1),
            Err(StorageError::IdentityMismatch)
        );
    }
    for revision in [3, 4] {
        assert!(SessionSaveReceipt::new(
            original.clone(),
            snapshot("conversation", [1; 16], revision),
            1,
            [3; 32]
        )
        .is_err());
    }
}

#[test]
fn snapshot_receipt_cannot_cross_conversations_with_the_same_incarnation_and_counters() {
    let a = snapshot("a", [1; 16], 4);
    let b = snapshot("b", [1; 16], 4);
    let next_b = snapshot("b", [1; 16], 5);
    let receipt = SessionSaveReceipt::new(b.clone(), next_b.clone(), 1, [3; 32]).unwrap();
    assert_eq!(receipt.next_for(&b, 1).unwrap(), next_b);
    assert_eq!(receipt.next_for(&a, 1), Err(StorageError::IdentityMismatch));
    assert!(matches!(
        SessionSaveReceipt::new(a, next_b, 1, [3; 32]),
        Err(StorageError::Corrupt(_))
    ));
}

#[test]
fn snapshot_binding_uses_one_revision_for_load_and_acknowledgement() {
    let id = SessionId::new("conversation").unwrap();
    for revision in [0, 4, u64::MAX] {
        let original = snapshot("conversation", [1; 16], revision);
        assert_eq!(original.base(), revision);
        assert_eq!(original.generation(), revision);
        let (_, loaded) = SessionLoad::new(None, original.clone(), SessionLoadState::Published)
            .into_published(&id)
            .unwrap();
        assert_eq!(loaded, original);
        if let Some(next) = revision.checked_add(1) {
            let next = snapshot("conversation", [1; 16], next);
            let receipt =
                SessionSaveReceipt::new(original.clone(), next.clone(), 1, [3; 32]).unwrap();
            assert_eq!(receipt.next_for(&original, 1).unwrap(), next);
        } else {
            assert!(SessionSaveReceipt::new(
                original,
                snapshot("conversation", [1; 16], 0),
                1,
                [3; 32]
            )
            .is_err());
        }
    }
}
