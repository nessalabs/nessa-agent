//! Append fences that have already committed, and drop a share-grant fence
//! whose latest journal row is a grant again. A failed append does not roll
//! the fence back and does not rewrite a partial log.

use std::collections::HashSet;

use super::super::domain::{DeletionFence, DeletionLog, FenceKind};
use super::ports::{DeletionLogStore, FenceSource};

/// Why the log was not updated. The fences themselves are unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryFailure {
    /// The log could not be read, so nothing was appended.
    Unreadable,
    /// The replacement could not be stored. The previous log remains.
    NotStored,
}

/// Read `fences` and append any that are not already in the log.
///
/// Every source has to succeed. A failure leaves the stored log untouched.
/// The fences from a successful read are the current share-grant revocations:
/// a grant fence already in the log is removed when this read does not name
/// it, which is a revoke whose latest journal row is a grant again. Conversation
/// and credential fences stay. Survivors are written back at sequences 1, 2, 3,
/// … and a fence that remains is still found by its key.
///
/// Returns the log after the attempt. On [`InventoryFailure`] the previous
/// log is still what `store` holds.
pub async fn capture_fences(
    fences: &[&dyn FenceSource],
    store: &dyn DeletionLogStore,
) -> Result<DeletionLog, InventoryFailure> {
    let mut pending = Vec::new();
    for source in fences {
        match source.fences().await {
            Ok(found) => pending.extend(found),
            Err(()) => return Err(InventoryFailure::Unreadable),
        }
    }
    let loaded = store
        .load()
        .await
        .map_err(|()| InventoryFailure::Unreadable)?;
    let next = reconcile(&loaded, pending);
    if next.canonical() == loaded.canonical() {
        return Ok(loaded);
    }
    store
        .store(&next)
        .await
        .map_err(|()| InventoryFailure::NotStored)?;
    Ok(next)
}

/// Keep every non-grant fence, keep a grant fence only when `fences` still
/// names it, then append `fences`.
fn reconcile(loaded: &DeletionLog, fences: Vec<DeletionFence>) -> DeletionLog {
    let live_grants: HashSet<&str> = fences
        .iter()
        .filter(|fence| is_share_grant(fence))
        .map(|fence| fence.key())
        .collect();
    let mut kept = Vec::new();
    for entry in loaded.entries() {
        if is_share_grant(entry.fence()) && !live_grants.contains(entry.fence().key()) {
            continue;
        }
        kept.push(entry.fence().clone());
    }
    let mut log = DeletionLog::default();
    for fence in kept.into_iter().chain(fences) {
        let (next, _append) = log.append(fence);
        log = next;
    }
    log
}

fn is_share_grant(fence: &DeletionFence) -> bool {
    matches!(fence.kind(), FenceKind::ShareGrant { .. })
}
