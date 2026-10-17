//! Append fences that have already committed, and drop a share-grant fence
//! whose latest journal row is a grant again. A failed append does not roll
//! the fence back and does not rewrite a partial log.

use std::collections::HashSet;

use super::super::domain::{DeletionFence, DeletionLog, FenceKind};
use super::ports::{
    DeletionLogStore, FenceSource, InventoryCapturePolicyEvaluator, InventoryDecision,
    InventoryDecisionAudit, InventoryDenial, InventoryFacts, LogStoreError,
};

/// How many times a writer reconciles again after another writer stored first.
const REPLACE_ATTEMPTS: u32 = 8;

/// Why the log was not updated. The fences themselves are unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum InventoryFailure {
    /// The log could not be read, so nothing was appended.
    Unreadable,
    /// This attempt did not replace the log.
    NotStored,
}

/// Why a capture was not written.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CaptureError {
    /// Cedar refused. The decision was stored. The log was not read.
    Denied(InventoryDenial),
    /// The decision could not be stored. The log was not read.
    NotAudited,
    /// The fences or the log could not be written. The decision stays.
    Inventory(InventoryFailure),
}

/// Store Cedar's decision, then reconcile `fences` into `store`.
///
/// The decision is stored before the log is read, on allow and on deny. An
/// audit failure returns [`CaptureError::NotAudited`] and does not read the
/// log. A denial returns after the decision is stored, still without reading
/// the log. [`CaptureError::Inventory`] leaves the log as [`capture_fences`]
/// left it: the previous bytes, or nothing new. The stored decision stays.
pub async fn capture_inventory(
    facts: &InventoryFacts<'_>,
    policy: &dyn InventoryCapturePolicyEvaluator,
    audit: &dyn InventoryDecisionAudit,
    fences: &[&dyn FenceSource],
    store: &dyn DeletionLogStore,
) -> Result<DeletionLog, CaptureError> {
    let decision = policy.evaluate(facts);
    if audit.record(facts, &decision).is_err() {
        return Err(CaptureError::NotAudited);
    }
    match decision {
        InventoryDecision::Allow { .. } => capture_fences(fences, store)
            .await
            .map_err(CaptureError::Inventory),
        InventoryDecision::Deny(reason) => {
            tracing::warn!(
                ?reason,
                kind = facts.kind,
                cause = facts.cause.as_str(),
                action = facts.action.as_str(),
                "the deletion inventory was not written"
            );
            Err(CaptureError::Denied(reason))
        }
    }
}

/// Read `fences` and append any that are not already in the log.
///
/// Captures hold the store's reconciliation lease through the source reads
/// and the final no-op or replacement, so an older read cannot land after
/// a later capture has returned
/// (`a_noop_regrant_capture_waits_for_an_older_revoke_read`).
/// Each attempt loads the log and then reads
/// every source. A source that
/// fails leaves the stored log untouched. The fences from that read are the
/// share-grant revocations current after the load: a grant fence already in
/// the loaded log is removed when this read does not name it, which is a
/// revoke whose latest journal row is a grant again. A revoke another writer
/// stored before this load stays when the read still names it. Conversation
/// and credential fences stay. Survivors are written back at sequences 1, 2,
/// 3, … and a fence that remains is still found by its key.
///
/// Returns the log after the attempt. On [`InventoryFailure::NotStored`] this
/// attempt did not replace the log. A writer that finds another writer's log
/// loads it and reads the fences again, up to `REPLACE_ATTEMPTS`, so that
/// other log's fences stay.
pub async fn capture_fences(
    fences: &[&dyn FenceSource],
    store: &dyn DeletionLogStore,
) -> Result<DeletionLog, InventoryFailure> {
    let _lease = store
        .hold_reconciliation()
        .await
        .map_err(|()| InventoryFailure::NotStored)?;
    for _ in 0..REPLACE_ATTEMPTS {
        let loaded = store
            .load()
            .await
            .map_err(|()| InventoryFailure::Unreadable)?;
        let mut pending = Vec::new();
        for source in fences {
            match source.fences().await {
                Ok(found) => pending.extend(found),
                Err(()) => return Err(InventoryFailure::Unreadable),
            }
        }
        let next = reconcile(&loaded, pending);
        if next.canonical() == loaded.canonical() {
            return Ok(loaded);
        }
        match store.store(&loaded, &next).await {
            Ok(()) => return Ok(next),
            Err(LogStoreError::Stale) => continue,
            Err(LogStoreError::Failed) => return Err(InventoryFailure::NotStored),
        }
    }
    Err(InventoryFailure::NotStored)
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
