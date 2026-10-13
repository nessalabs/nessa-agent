//! One cut. Admission is held across the reads and released on every path.

use std::collections::BTreeMap;

use super::super::domain::{
    AuthenticatedManifest, Manifest, SkipReason, SkippedRecord, SourceCut, StoreKind, Watermark,
    WRITER_INVENTORY,
};
use super::deletion_inventory::capture_fences;
use super::ports::{
    AdmissionFailure, Authenticator, DeletionLogStore, ExportClock, ExportSource, FenceSource,
    ManifestPublisher, MutationAdmission, PublishedRecord,
};

/// Everything one capture needs. The caller supplies the stores; this
/// module does not open them.
pub struct ExportRequest<'a> {
    pub admission: &'a dyn MutationAdmission,
    pub sources: &'a [&'a dyn ExportSource],
    pub fences: &'a [&'a dyn FenceSource],
    pub inventory: &'a dyn DeletionLogStore,
    pub publisher: &'a dyn ManifestPublisher,
    pub clock: &'a dyn ExportClock,
    pub authenticator: &'a dyn Authenticator,
}

/// Why a cut was not published.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ExportFailure {
    /// A required store had no source. Admission was not taken.
    Inventory,
    /// The gateway is retiring.
    Retired,
    /// The mac could not be produced. Nothing was published.
    Authenticator,
    /// The bundle could not be written. Nothing is claimed as published.
    Publish,
}

/// Hold admission, read every store, append fences, and publish.
///
/// The hold is dropped before this returns, including on failure.
pub async fn capture_export(
    request: ExportRequest<'_>,
) -> Result<AuthenticatedManifest, ExportFailure> {
    if !inventory_complete(request.sources) {
        return Err(ExportFailure::Inventory);
    }
    let hold = request
        .admission
        .hold()
        .await
        .map_err(|AdmissionFailure::Retired| ExportFailure::Retired)?;
    let outcome = async {
        if hold.retirement_signaled() {
            return Err(ExportFailure::Retired);
        }
        let appended = capture_fences(request.fences, request.inventory).await;
        let mut cuts = BTreeMap::new();
        for source in request.sources {
            let kind = source.kind();
            let cut = match source.capture().await {
                Ok(cut) if cut.store() == kind => cut,
                Ok(_) | Err(()) => skipped_store(kind)?,
            };
            cuts.insert(kind, cut);
        }
        if appended.is_err() {
            let inventory = cuts
                .get_mut(&StoreKind::DeletionInventory)
                .ok_or(ExportFailure::Inventory)?;
            push_skip(inventory, "append", SkipReason::Unavailable)?;
        }
        if hold.retirement_signaled() {
            return Err(ExportFailure::Retired);
        }
        let stores = WRITER_INVENTORY
            .iter()
            .filter_map(|kind| cuts.remove(kind))
            .collect();
        let manifest =
            Manifest::new(request.clock.unix_ms(), stores).map_err(|_| ExportFailure::Inventory)?;
        let mac = request
            .authenticator
            .mac(&manifest.canonical())
            .map_err(|()| ExportFailure::Authenticator)?;
        let sealed = AuthenticatedManifest::seal(manifest, mac);
        let records = published(&sealed);
        request
            .publisher
            .put(&sealed.encoded(), &records)
            .await
            .map_err(|()| ExportFailure::Publish)?;
        Ok(sealed)
    }
    .await;
    drop(hold);
    outcome
}

fn inventory_complete(sources: &[&dyn ExportSource]) -> bool {
    if sources.len() != WRITER_INVENTORY.len() {
        return false;
    }
    WRITER_INVENTORY
        .iter()
        .zip(sources.iter())
        .all(|(kind, source)| source.kind() == *kind)
}

fn skipped_store(kind: StoreKind) -> Result<SourceCut, ExportFailure> {
    let watermark = Watermark::new("unavailable").map_err(|_| ExportFailure::Inventory)?;
    let skipped = SkippedRecord::new(kind, "store", SkipReason::Unavailable)
        .map_err(|_| ExportFailure::Inventory)?;
    SourceCut::new(kind, watermark, Vec::new(), vec![skipped]).map_err(|_| ExportFailure::Inventory)
}

fn push_skip(cut: &mut SourceCut, identity: &str, reason: SkipReason) -> Result<(), ExportFailure> {
    let skipped =
        SkippedRecord::new(cut.store(), identity, reason).map_err(|_| ExportFailure::Inventory)?;
    let records = cut.records().to_vec();
    let mut skipped_records = cut.skipped().to_vec();
    skipped_records.push(skipped);
    *cut = SourceCut::new(
        cut.store(),
        cut.watermark().clone(),
        records,
        skipped_records,
    )
    .map_err(|_| ExportFailure::Inventory)?;
    Ok(())
}

fn published(sealed: &AuthenticatedManifest) -> Vec<PublishedRecord> {
    let mut records = Vec::new();
    for cut in sealed.manifest().stores() {
        for record in cut.records() {
            if let Some(bytes) = record.public_bytes() {
                records.push(PublishedRecord {
                    store: record.store(),
                    identity: record.identity().to_owned(),
                    bytes: bytes.to_vec(),
                });
            }
        }
    }
    records
}
