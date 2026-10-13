//! Append fences that have already committed. A failed append does not
//! roll the fence back and does not rewrite a partial log.

use super::super::domain::{DeletionFence, DeletionLog};
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
    let next = append_all(&loaded, pending);
    if next.entries().len() == loaded.entries().len() {
        return Ok(loaded);
    }
    store
        .store(&next)
        .await
        .map_err(|()| InventoryFailure::NotStored)?;
    Ok(next)
}

fn append_all(loaded: &DeletionLog, fences: Vec<DeletionFence>) -> DeletionLog {
    let mut log = loaded.clone();
    for fence in fences {
        let (next, _append) = log.append(fence);
        log = next;
    }
    log
}
