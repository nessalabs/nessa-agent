//! Read-only verification of supplied export bytes. No gateway or restore ports.

use std::collections::{BTreeMap, BTreeSet};
use std::io::Read;

use serde::Serialize;
use sha2::{Digest, Sha256};

use crate::backup::domain::{
    DeletionLog, DeletionReadError, Fingerprint, ListedStore, ManifestListing, ManifestReadError,
    StoreKind, EXPORT_HEADER,
};

/// Reads are supplied by the caller; the verifier has no profile locator.
pub trait VerificationSource {
    fn manifest(&self) -> Result<Box<dyn Read + '_>, VerificationFailure>;
    fn record(
        &self,
        store: StoreKind,
        encoded_identity: &str,
    ) -> Result<Box<dyn Read + '_>, VerificationFailure>;

    /// Enumerate only regular bodies in the fixed bundle layout. Stop when
    /// the visitor refuses a name; do not collect an unbounded directory.
    fn visit_records(
        &self,
        visitor: &mut dyn FnMut(StoreKind, &str) -> Result<(), VerificationFailure>,
    ) -> Result<(), VerificationFailure>;
}

/// The trusted verification capability is injected, not recovered from a cut.
pub trait ManifestMacVerifier {
    fn verify(&self, canonical: &str, mac: Fingerprint) -> Result<(), VerificationFailure>;
}

/// Work limits, independent of the format's compatibility and restore policy.
#[derive(Clone, Copy)]
pub struct VerificationLimits {
    pub manifest_bytes: usize,
    pub named_entries: usize,
    pub inventory_bytes: usize,
    pub total_body_bytes: u64,
}

impl Default for VerificationLimits {
    fn default() -> Self {
        Self {
            manifest_bytes: 4 * 1024 * 1024,
            named_entries: 16_384,
            inventory_bytes: 4 * 1024 * 1024,
            total_body_bytes: 1024 * 1024 * 1024,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum VerificationFailure {
    Unreadable,
    UnsafeObject,
    InvalidLayout,
    MalformedManifest,
    UnsupportedFormat,
    ResourceLimit,
    Authentication,
    InvalidKey,
    MissingRecord,
    UnexpectedRecord,
    BodyFingerprint,
    MalformedInventory,
    UnsupportedInventory,
    InventoryWatermark,
}

#[derive(Debug, Serialize)]
pub struct VerificationReport {
    pub format: &'static str,
    pub captured_at_ms: String,
    pub public_records_verified: usize,
    pub body_bytes_verified: u64,
    pub inventory: InventoryVerification,
    pub omissions: Vec<VerifiedOmission>,
    pub inventory_freshness: &'static str,
    pub restore_readiness: &'static str,
    pub payload_compatibility: &'static str,
}

#[derive(Debug, Serialize)]
#[serde(tag = "status", rename_all = "snake_case")]
pub enum InventoryVerification {
    NotIncluded,
    Verified { entries: usize },
}

#[derive(Debug, Serialize)]
pub struct VerifiedOmission {
    pub store: &'static str,
    pub identity: String,
    pub kind: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<&'static str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub size: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub fingerprint: Option<String>,
}

/// Authenticate the manifest before any payload access, match its public
/// layout, then stream each body and check the exported inventory (V1–V11).
pub fn verify_bundle(
    source: &dyn VerificationSource,
    verifier: &dyn ManifestMacVerifier,
    limits: VerificationLimits,
) -> Result<VerificationReport, VerificationFailure> {
    let bytes = read_bounded(source.manifest()?, limits.manifest_bytes)?;
    let text = std::str::from_utf8(&bytes).map_err(|_| VerificationFailure::MalformedManifest)?;
    let listing =
        ManifestListing::parse(text, limits.named_entries).map_err(|error| match error {
            ManifestReadError::UnsupportedVersion => VerificationFailure::UnsupportedFormat,
            ManifestReadError::Malformed => VerificationFailure::MalformedManifest,
            ManifestReadError::EntryLimit => VerificationFailure::ResourceLimit,
        })?;
    verifier.verify(&text[..listing.canonical_len()], listing.mac())?;

    let mut expected: BTreeMap<StoreKind, BTreeSet<String>> = BTreeMap::new();
    for store in listing.stores() {
        for record in store.records().iter().filter(|record| record.is_public()) {
            expected
                .entry(store.store())
                .or_default()
                .insert(record.encoded_identity());
        }
    }
    source.visit_records(&mut |store, identity| {
        if expected
            .get_mut(&store)
            .is_some_and(|names| names.remove(identity))
        {
            Ok(())
        } else {
            Err(VerificationFailure::UnexpectedRecord)
        }
    })?;
    if expected.values().any(|names| !names.is_empty()) {
        return Err(VerificationFailure::MissingRecord);
    }

    let mut report = VerificationReport {
        format: EXPORT_HEADER,
        captured_at_ms: listing.captured_at_ms().to_string(),
        public_records_verified: 0,
        body_bytes_verified: 0,
        inventory: InventoryVerification::NotIncluded,
        omissions: Vec::new(),
        inventory_freshness: "not_assessed",
        restore_readiness: "not_assessed",
        payload_compatibility: "not_assessed",
    };
    for store in listing.stores() {
        for record in store.records() {
            if !record.is_public() {
                report.omissions.push(VerifiedOmission {
                    store: store.store().as_str(),
                    identity: record.identity().to_owned(),
                    kind: "redacted",
                    reason: None,
                    size: None,
                    fingerprint: Some(record.fingerprint().hex()),
                });
                continue;
            }
            let is_inventory = store.store() == StoreKind::DeletionInventory;
            if is_inventory && (record.identity() != "log" || store.records().len() != 1) {
                return Err(VerificationFailure::MalformedInventory);
            }
            let mut reader = source.record(store.store(), &record.encoded_identity())?;
            let (fingerprint, inventory) = hash_body(
                &mut reader,
                &mut report.body_bytes_verified,
                limits,
                is_inventory,
            )?;
            if fingerprint != record.fingerprint() {
                return Err(VerificationFailure::BodyFingerprint);
            }
            if is_inventory {
                report.inventory = verify_inventory(&inventory, store, limits.named_entries)?;
            }
            report.public_records_verified += 1;
        }
        for skipped in store.skipped() {
            report.omissions.push(VerifiedOmission {
                store: store.store().as_str(),
                identity: skipped.identity().to_owned(),
                kind: "skipped",
                reason: Some(skipped.reason().as_str()),
                size: None,
                fingerprint: None,
            });
        }
        for deferred in store.deferred() {
            report.omissions.push(VerifiedOmission {
                store: store.store().as_str(),
                identity: deferred.conversation_id().to_owned(),
                kind: "deferred",
                reason: None,
                size: Some(deferred.size()),
                fingerprint: Some(deferred.digest().hex()),
            });
        }
    }
    Ok(report)
}

fn read_bounded(
    mut reader: Box<dyn Read + '_>,
    limit: usize,
) -> Result<Vec<u8>, VerificationFailure> {
    let mut bytes = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let remaining = limit.saturating_sub(bytes.len());
        let request = remaining.saturating_add(1).min(buffer.len());
        let count = reader
            .read(&mut buffer[..request])
            .map_err(|_| VerificationFailure::Unreadable)?;
        if count > remaining {
            return Err(VerificationFailure::ResourceLimit);
        }
        if count == 0 {
            return Ok(bytes);
        }
        bytes.extend_from_slice(&buffer[..count]);
    }
}

fn hash_body(
    reader: &mut dyn Read,
    total: &mut u64,
    limits: VerificationLimits,
    keep_inventory: bool,
) -> Result<(Fingerprint, Vec<u8>), VerificationFailure> {
    let mut hash = Sha256::new();
    let mut inventory = Vec::new();
    let mut buffer = [0; 8192];
    loop {
        let remaining = limits.total_body_bytes.saturating_sub(*total);
        let mut request = remaining.saturating_add(1).min(buffer.len() as u64) as usize;
        if keep_inventory {
            request = request.min(
                limits
                    .inventory_bytes
                    .saturating_sub(inventory.len())
                    .saturating_add(1),
            );
        }
        let count = reader
            .read(&mut buffer[..request])
            .map_err(|_| VerificationFailure::Unreadable)?;
        if count as u64 > remaining {
            return Err(VerificationFailure::ResourceLimit);
        }
        if count == 0 {
            break;
        }
        *total += count as u64;
        if keep_inventory {
            if count > limits.inventory_bytes.saturating_sub(inventory.len()) {
                return Err(VerificationFailure::ResourceLimit);
            }
            inventory.extend_from_slice(&buffer[..count]);
        }
        hash.update(&buffer[..count]);
    }
    Ok((Fingerprint::from_digest(hash.finalize().into()), inventory))
}

fn verify_inventory(
    bytes: &[u8],
    store: &ListedStore,
    max_entries: usize,
) -> Result<InventoryVerification, VerificationFailure> {
    let text = std::str::from_utf8(bytes).map_err(|_| VerificationFailure::MalformedInventory)?;
    if text.lines().count().saturating_sub(1) > max_entries {
        return Err(VerificationFailure::ResourceLimit);
    }
    let log = DeletionLog::decode(text).map_err(|error| match error {
        DeletionReadError::UnsupportedVersion => VerificationFailure::UnsupportedInventory,
        DeletionReadError::Malformed => VerificationFailure::MalformedInventory,
    })?;
    if log.canonical() != text {
        return Err(VerificationFailure::MalformedInventory);
    }
    let mut fences = BTreeSet::new();
    if log
        .entries()
        .iter()
        .any(|entry| !fences.insert(entry.fence().key()))
    {
        return Err(VerificationFailure::MalformedInventory);
    }
    if log.watermark() != store.watermark().as_str() {
        return Err(VerificationFailure::InventoryWatermark);
    }
    Ok(InventoryVerification::Verified {
        entries: log.entries().len(),
    })
}
