//! Receipt capability agreement; these tests assert no persistence outcome.
use super::*;
use nessa_sync::replication::domain::Id;

#[test]
fn receipt_requires_original_scope_and_entire_unit_count_before_next_work() {
    let backend = SessionSaveBackend::Snapshot {
        incarnation: [1; 16],
    };
    let original = SessionSaveGeneration::new(backend.clone(), 4, 4);
    let next = SessionSaveGeneration::new(backend, 5, 5);
    let receipt = SessionSaveReceipt::new(original.clone(), next.clone(), 2, [3; 32]).unwrap();
    assert_eq!(receipt.next_for(&original, 2).unwrap(), next);
    assert_eq!(
        receipt.next_for(&original, 1),
        Err(StorageError::IdentityMismatch)
    );
    assert_eq!(
        receipt.next_for(&original, 3),
        Err(StorageError::IdentityMismatch)
    );
    let foreign = SessionSaveGeneration::new(
        SessionSaveBackend::Snapshot {
            incarnation: [2; 16],
        },
        4,
        4,
    );
    assert_eq!(
        receipt.next_for(&foreign, 2),
        Err(StorageError::IdentityMismatch)
    );
    assert!(SessionSaveReceipt::new(original.clone(), original.clone(), 2, [3; 32]).is_err());
    assert!(SessionSaveReceipt::new(original, next, 0, [3; 32]).is_err());
}

#[test]
fn record_receipt_cannot_skip_generation_or_cross_reset_incarnation() {
    let backend = SessionSaveBackend::Record {
        stream: Id::new("conversation").unwrap(),
        incarnation: [1; 16],
    };
    let original = SessionSaveGeneration::new(backend.clone(), 8, 3);
    let skipped = SessionSaveGeneration::new(backend.clone(), 10, 5);
    assert!(SessionSaveReceipt::new(original.clone(), skipped, 1, [3; 32]).is_err());
    let replacement = SessionSaveGeneration::new(
        SessionSaveBackend::Record {
            stream: Id::new("conversation").unwrap(),
            incarnation: [2; 16],
        },
        10,
        4,
    );
    assert!(SessionSaveReceipt::new(original, replacement, 1, [3; 32]).is_err());
    let exhausted = SessionSaveGeneration::new(backend.clone(), 8, u64::MAX);
    let wrapped = SessionSaveGeneration::new(backend, 10, 0);
    assert!(SessionSaveReceipt::new(exhausted, wrapped, 1, [3; 32]).is_err());
}

#[test]
fn immutable_unit_and_snapshot_receipt_constructor_refuse_each_invalid_capability() {
    assert!(SessionSaveUnit::new(Vec::new()).is_err());
    let backend = SessionSaveBackend::Snapshot {
        incarnation: [1; 16],
    };
    let original = SessionSaveGeneration::new(backend.clone(), 4, 4);
    let next = SessionSaveGeneration::new(backend.clone(), 5, 5);
    let receipt = SessionSaveReceipt::new(original.clone(), next.clone(), 1, [3; 32]).unwrap();
    assert_eq!(receipt.next_for(&original, 1).unwrap(), next);
    for changed in [
        SessionSaveGeneration::new(backend.clone(), 3, 4),
        SessionSaveGeneration::new(backend.clone(), 4, 3),
    ] {
        assert_eq!(
            receipt.next_for(&changed, 1),
            Err(StorageError::IdentityMismatch)
        );
    }
    let invalid_original = SessionSaveGeneration::new(backend.clone(), 4, 3);
    assert!(SessionSaveReceipt::new(invalid_original, next.clone(), 1, [3; 32]).is_err());
    let invalid_next = SessionSaveGeneration::new(backend, 5, 6);
    assert!(SessionSaveReceipt::new(original, invalid_next, 1, [3; 32]).is_err());
}
