//! The authenticated manifest of one cut. It names watermarks, record
//! fingerprints, and skips. It does not hold secret bytes.

use super::encoding::encode_identity as encode;

use super::inventory::{StoreKind, WRITER_INVENTORY};
use super::record::{
    CapturedRecord, DeferredEntry, Fingerprint, RecordBody, SkippedRecord, SourceCut,
};

pub const EXPORT_HEADER: &str = "nessa-export-cut/1";

/// One cut, after every required store has been read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Manifest {
    captured_at_ms: u64,
    stores: Vec<SourceCut>,
}

impl Manifest {
    /// `stores` must be the writer inventory, each kind once, in inventory order.
    pub fn new(captured_at_ms: u64, stores: Vec<SourceCut>) -> Result<Self, ManifestError> {
        if stores.len() != WRITER_INVENTORY.len() {
            return Err(ManifestError::Inventory);
        }
        for (expected, cut) in WRITER_INVENTORY.iter().zip(stores.iter()) {
            if cut.store() != *expected {
                return Err(ManifestError::Inventory);
            }
        }
        Ok(Self {
            captured_at_ms,
            stores,
        })
    }

    pub fn captured_at_ms(&self) -> u64 {
        self.captured_at_ms
    }

    pub fn stores(&self) -> &[SourceCut] {
        &self.stores
    }

    pub fn store(&self, kind: StoreKind) -> &SourceCut {
        &self.stores[inventory_index(kind)]
    }

    /// Canonical text the mac is taken over. Record bodies are not in it;
    /// their fingerprints are.
    pub fn canonical(&self) -> String {
        let mut text = String::new();
        text.push_str(EXPORT_HEADER);
        text.push('\n');
        text.push_str(&format!("captured-at {}\n", self.captured_at_ms));
        for cut in &self.stores {
            text.push_str(&format!(
                "watermark {} {}\n",
                cut.store().as_str(),
                encode(cut.watermark().as_str())
            ));
            for record in cut.records() {
                text.push_str(&record_line(record));
            }
            for deferred in cut.deferred() {
                text.push_str(&deferred_line(deferred));
            }
            for skipped in cut.skipped() {
                text.push_str(&skip_line(skipped));
            }
        }
        text
    }
}

/// A manifest plus the mac of its canonical text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct AuthenticatedManifest {
    manifest: Manifest,
    mac: Fingerprint,
}

impl AuthenticatedManifest {
    pub fn seal(manifest: Manifest, mac: Fingerprint) -> Self {
        Self { manifest, mac }
    }

    pub fn manifest(&self) -> &Manifest {
        &self.manifest
    }

    pub fn mac(&self) -> &Fingerprint {
        &self.mac
    }

    /// The canonical text with the mac line appended. The mac covers the
    /// text without that line.
    pub fn encoded(&self) -> String {
        let mut text = self.manifest.canonical();
        text.push_str(&format!("mac {}\n", self.mac.hex()));
        text
    }
}

/// Why a manifest was refused.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ManifestError {
    /// A required store was missing, repeated, or out of order.
    Inventory,
}

fn inventory_index(kind: StoreKind) -> usize {
    // `Manifest::new` admits only `WRITER_INVENTORY`, which lists every kind.
    WRITER_INVENTORY
        .iter()
        .position(|stored| *stored == kind)
        .expect("every store kind is in the writer inventory")
}

fn record_line(record: &CapturedRecord) -> String {
    match record.body() {
        RecordBody::Public(_) => format!(
            "record {} {} {}\n",
            record.store().as_str(),
            encode(record.identity()),
            record.fingerprint().hex()
        ),
        RecordBody::Redacted(fingerprint) => format!(
            "redacted {} {} {}\n",
            record.store().as_str(),
            encode(record.identity()),
            fingerprint.hex()
        ),
    }
}

fn deferred_line(entry: &DeferredEntry) -> String {
    format!(
        "deferred {} {} {} {}\n",
        entry.store().as_str(),
        encode(entry.conversation_id()),
        entry.size(),
        entry.digest().hex()
    )
}

fn skip_line(skipped: &SkippedRecord) -> String {
    format!(
        "skipped {} {} {}\n",
        skipped.store().as_str(),
        encode(skipped.identity()),
        skipped.reason().as_str()
    )
}
