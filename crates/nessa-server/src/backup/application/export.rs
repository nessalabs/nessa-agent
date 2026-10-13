//! One cut. Admission is held only while stores are pinned, then released
//! before records are read and the bundle is published.

use std::collections::BTreeMap;

use super::super::domain::{
    AuthenticatedManifest, Manifest, SkipReason, SkippedRecord, SourceCut, StoreKind, Watermark,
    WRITER_INVENTORY,
};
use super::deletion_inventory::capture_fences;
use super::ports::{
    AdmissionFailure, AdmissionLease, Authenticator, CopiedStore, DeletionLogStore, ExportClock,
    ExportSource, FenceSource, ManifestPublisher, MutationAdmission, PinnedStore, PublishedRecord,
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

/// Pin every store, append fences, release admission, then read and publish.
///
/// The write hold joins in-flight commands and covers only the pin: each
/// store's watermark, an open SQLite read snapshot or a copy of a small
/// file, and the deletion-inventory append. Records, the mac, and publish
/// run after the hold is dropped, from those snapshots, so a later write is
/// not in the cut and a command can proceed while they run. The hold is
/// dropped before this returns, including on failure and panic.
pub async fn capture_export(
    request: ExportRequest<'_>,
) -> Result<AuthenticatedManifest, ExportFailure> {
    if !inventory_complete(request.sources) {
        return Err(ExportFailure::Inventory);
    }
    let (mut pinned, append_failed) = {
        let hold = request
            .admission
            .hold()
            .await
            .map_err(|AdmissionFailure::Retired| ExportFailure::Retired)?;
        let prepared = pin_held(hold.as_ref(), &request).await;
        drop(hold);
        prepared?
    };
    let mut cuts = BTreeMap::new();
    for (source, pinned) in request.sources.iter().zip(pinned.iter_mut()) {
        let kind = source.kind();
        let cut = match pinned.read().await {
            Ok(cut) if cut.store() == kind => cut,
            Ok(_) | Err(()) => skipped_store(kind)?,
        };
        cuts.insert(kind, cut);
    }
    if append_failed {
        let inventory = cuts
            .get_mut(&StoreKind::DeletionInventory)
            .ok_or(ExportFailure::Inventory)?;
        push_skip(inventory, "append", SkipReason::Unavailable)?;
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

/// Snapshots and the fence append. Admission is still held by the caller.
async fn pin_held(
    hold: &dyn AdmissionLease,
    request: &ExportRequest<'_>,
) -> Result<(Vec<Box<dyn PinnedStore>>, bool), ExportFailure> {
    if hold.retirement_signaled() {
        return Err(ExportFailure::Retired);
    }
    let mut pinned = Vec::with_capacity(request.sources.len());
    let mut inventory_at = None;
    for (index, source) in request.sources.iter().enumerate() {
        if source.kind() == StoreKind::DeletionInventory {
            inventory_at = Some(index);
            pinned.push(pin_or_skip_placeholder(source.kind())?);
            continue;
        }
        pinned.push(pin_or_skip(*source).await?);
    }
    // The append is inside the hold, after the snapshots it reads, so the
    // inventory pin below sees the log those fences produced.
    let appended = capture_fences(request.fences, request.inventory).await;
    if let Some(index) = inventory_at {
        pinned[index] = pin_or_skip(request.sources[index]).await?;
    }
    if hold.retirement_signaled() {
        return Err(ExportFailure::Retired);
    }
    Ok((pinned, appended.is_err()))
}

async fn pin_or_skip(source: &dyn ExportSource) -> Result<Box<dyn PinnedStore>, ExportFailure> {
    match source.pin().await {
        Ok(pinned) => Ok(pinned),
        Err(()) => Ok(Box::new(CopiedStore::new(skipped_store(source.kind())?))),
    }
}

fn pin_or_skip_placeholder(kind: StoreKind) -> Result<Box<dyn PinnedStore>, ExportFailure> {
    Ok(Box::new(CopiedStore::new(skipped_store(kind)?)))
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
