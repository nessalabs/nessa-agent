//! Domain construction without a persisted representation or filesystem.

use crate::backup::domain::{
    DeferredEntry, Fingerprint, ListedRecord, ListedRecords, ListedStore, ManifestListing,
    RecordError, SkipReason, SkippedRecord, StoreKind, Watermark, WRITER_INVENTORY,
};

fn record(identity: &str, public: bool) -> ListedRecord {
    ListedRecord::new(identity.into(), Fingerprint::of(b"synthetic"), public).unwrap()
}

fn store(kind: StoreKind) -> ListedStore {
    ListedStore::new(
        kind,
        Watermark::new("cut").unwrap(),
        ListedRecords::default(),
        vec![],
        vec![],
    )
    .unwrap()
}

#[test]
fn listed_public_names_are_valid_and_distinct_while_omissions_may_overlap() {
    for name in ["", "bad\nname", ".", ".."] {
        assert_eq!(
            ListedRecord::new(name.into(), Fingerprint::of(b""), true),
            Err(RecordError::Identity)
        );
    }
    assert!(ListedRecord::new("é".repeat(257), Fingerprint::of(b""), false).is_err());
    let cut = |records: Vec<ListedRecord>| {
        records
            .into_iter()
            .try_fold(ListedRecords::default(), ListedRecords::with_record)
    };
    assert_eq!(
        cut(vec![record("a", true), record("a", true)]),
        Err(RecordError::Identity)
    );
    assert!(cut(vec![
        record("a", true),
        record("a", false),
        record("a", false),
        record(".", false)
    ])
    .is_ok());
}

#[test]
fn listed_store_owns_its_omissions_and_deferred_entries() {
    let kind = StoreKind::CanonicalRecords;
    let other = StoreKind::ConversationAudit;
    assert_eq!(
        ListedStore::new(
            kind,
            Watermark::new("cut").unwrap(),
            ListedRecords::default(),
            vec![SkippedRecord::new(other, "a", SkipReason::Bounded).unwrap()],
            vec![]
        ),
        Err(RecordError::Store)
    );
    assert_eq!(
        ListedStore::new(
            kind,
            Watermark::new("cut").unwrap(),
            ListedRecords::default(),
            vec![],
            vec![DeferredEntry::new(other, "a", 5, Fingerprint::of(b"bytes")).unwrap()]
        ),
        Err(RecordError::Store)
    );
}

#[test]
fn listing_requires_each_writer_store_once_in_order() {
    let stores = || {
        WRITER_INVENTORY
            .iter()
            .copied()
            .map(store)
            .collect::<Vec<_>>()
    };
    assert!(ManifestListing::new(123, stores()).is_ok());
    let mut missing = stores();
    missing.pop();
    assert_eq!(ManifestListing::new(123, missing), Err(RecordError::Store));
    let mut reordered = stores();
    reordered.swap(0, 1);
    assert_eq!(
        ManifestListing::new(123, reordered),
        Err(RecordError::Store)
    );
    let mut repeated = stores();
    repeated[1] = store(WRITER_INVENTORY[0]);
    assert_eq!(ManifestListing::new(123, repeated), Err(RecordError::Store));
}
