//! Validated cut metadata, without persisted grammar or public record bodies.

use std::collections::HashSet;

use super::encoding::encode_identity;
use super::record::checked_identity;
use super::{
    DeferredEntry, Fingerprint, RecordError, SkippedRecord, StoreKind, Watermark, WRITER_INVENTORY,
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedRecord {
    identity: String,
    fingerprint: Fingerprint,
    public: bool,
}

impl ListedRecord {
    pub fn new(
        identity: String,
        fingerprint: Fingerprint,
        public: bool,
    ) -> Result<Self, RecordError> {
        let identity = checked_identity(identity)?;
        if public && matches!(identity.as_str(), "." | "..") {
            return Err(RecordError::Identity);
        }
        Ok(Self {
            identity,
            fingerprint,
            public,
        })
    }

    pub fn identity(&self) -> &str {
        &self.identity
    }
    pub fn fingerprint(&self) -> Fingerprint {
        self.fingerprint
    }
    pub fn is_public(&self) -> bool {
        self.public
    }
    pub fn encoded_identity(&self) -> String {
        encode_identity(&self.identity)
    }
}

/// A store's named records, constructed incrementally so a duplicate is refused
/// before the decoder consumes the next entry or its allocation budget
/// (`duplicate_public_names_are_refused_before_a_later_entry_limit`).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct ListedRecords {
    records: Vec<ListedRecord>,
    public_identities: HashSet<String>,
}

impl ListedRecords {
    pub fn with_record(mut self, record: ListedRecord) -> Result<Self, RecordError> {
        if record.is_public() && !self.public_identities.insert(record.identity().to_owned()) {
            return Err(RecordError::Identity);
        }
        self.records.push(record);
        Ok(self)
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ListedStore {
    store: StoreKind,
    watermark: Watermark,
    records: Vec<ListedRecord>,
    skipped: Vec<SkippedRecord>,
    deferred: Vec<DeferredEntry>,
}

impl ListedStore {
    pub fn new(
        store: StoreKind,
        watermark: Watermark,
        records: ListedRecords,
        skipped: Vec<SkippedRecord>,
        deferred: Vec<DeferredEntry>,
    ) -> Result<Self, RecordError> {
        if skipped.iter().any(|entry| entry.store() != store)
            || deferred.iter().any(|entry| entry.store() != store)
        {
            return Err(RecordError::Store);
        }
        Ok(Self {
            store,
            watermark,
            records: records.records,
            skipped,
            deferred,
        })
    }

    pub fn store(&self) -> StoreKind {
        self.store
    }
    pub fn watermark(&self) -> &Watermark {
        &self.watermark
    }
    pub fn records(&self) -> &[ListedRecord] {
        &self.records
    }
    pub fn skipped(&self) -> &[SkippedRecord] {
        &self.skipped
    }
    pub fn deferred(&self) -> &[DeferredEntry] {
        &self.deferred
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ManifestListing {
    captured_at_ms: u64,
    stores: Vec<ListedStore>,
}

impl ManifestListing {
    pub fn new(captured_at_ms: u64, stores: Vec<ListedStore>) -> Result<Self, RecordError> {
        if stores.len() != WRITER_INVENTORY.len()
            || stores
                .iter()
                .zip(WRITER_INVENTORY)
                .any(|(store, kind)| store.store() != *kind)
        {
            return Err(RecordError::Store);
        }
        Ok(Self {
            captured_at_ms,
            stores,
        })
    }

    pub fn captured_at_ms(&self) -> u64 {
        self.captured_at_ms
    }
    pub fn stores(&self) -> &[ListedStore] {
        &self.stores
    }
}
