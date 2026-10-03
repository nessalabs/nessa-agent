//! Physical stage exclusion and lookup-only terminal settlement.
//!
//! The canonical registry owns the bounded gate. A lease retains that registry's
//! lifetime lock until actual physical work drops it, independently of the caller.
use super::{PairingStore, PairingStoreError, ReceiverOutcome};
use crate::{
    application::ports::PortFuture,
    domain::pairing::{PairingError, PairingPhase, PairingRecord},
};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

pub(crate) struct StageGate(AtomicBool);
impl StageGate {
    pub(crate) fn new() -> Self {
        Self(AtomicBool::new(false))
    }
    pub(crate) fn acquire(self: &Arc<Self>) -> Result<StagePermit, PairingStoreError> {
        self.0
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .map_err(|_| PairingStoreError::StageOccupied)?;
        Ok(StagePermit(self.clone()))
    }
}
pub(crate) struct StagePermit(Arc<StageGate>);
impl Drop for StagePermit {
    fn drop(&mut self) {
        self.0 .0.store(false, Ordering::Release);
    }
}
/// Exclusive physical stage ownership, not an enrollment approval or wire proof.
///
/// Move this value into the actual physical worker. Dropping an awaiting caller
/// must not drop a lease still used by that worker. No clone or deserialization
/// can mint another owner; the registry's bounded gate admits one at a time.
pub struct StageOwnership {
    record: PairingRecord,
    store: Arc<dyn PairingStore>,
    _permit: StagePermit,
}
impl StageOwnership {
    pub(crate) fn new(
        record: PairingRecord,
        store: Arc<dyn PairingStore>,
        permit: StagePermit,
    ) -> Self {
        Self {
            record,
            store,
            _permit: permit,
        }
    }
    /// Canonical snapshot correlated with this physical reservation.
    pub fn record(&self) -> &PairingRecord {
        &self.record
    }
    /// Re-read the canonical owner, retaining the physical exclusion lease.
    pub fn current(&self) -> Result<PairingRecord, PairingStoreError> {
        self.store.read_pairing(self.record.id())
    }
}
/// Trusted canonical receiver lookup; it must never dispatch a new Pair.
pub trait StageReceiptLookup: Send + Sync {
    /// Read the original exact Pair correlation under the held physical lease.
    /// None means a coherent absent receipt, not unavailable or uncertain storage.
    fn lookup<'a>(
        &'a self,
        stage: StageOwnership,
    ) -> PortFuture<'a, (StageOwnership, Option<ReceiverOutcome>), PairingStoreError>;
}
/// Private no-effect admission retains exclusion through canonical completion.
pub struct NoReceiverCleanupProof {
    ownership: StageOwnership,
}
impl NoReceiverCleanupProof {
    pub(crate) fn record(&self) -> &PairingRecord {
        self.ownership.record()
    }
    pub(crate) fn belongs_to(&self, gate: &Arc<StageGate>) -> bool {
        Arc::ptr_eq(&self.ownership._permit.0, gate)
    }
}
/// Original receiver evidence or a privately established absent physical effect.
pub enum StageResolution {
    /// Original canonical effect; retain it before requesting physical fencing.
    Receiver {
        /// Keep exclusion while retaining the exact original receiver evidence.
        ownership: StageOwnership,
        /// Original canonical Pair result.
        outcome: ReceiverOutcome,
    },
    /// No original receipt exists while dispatch is excluded.
    NoReceiver(NoReceiverCleanupProof),
}
/// Resolve only a terminal stage after its original dispatch owner has drained.
///
/// Lookup failure preserves the terminal obligation. This operation never starts
/// a Pair. The non-cloneable lease excludes a later runner until resolution ends.
pub async fn resolve_terminal_stage(
    ownership: StageOwnership,
    receipts: &dyn StageReceiptLookup,
) -> Result<StageResolution, PairingStoreError> {
    let current = ownership.current()?;
    if current.phase() != PairingPhase::Terminal || current.receiver_binding().is_some() {
        return Err(PairingStoreError::Domain(PairingError::Conflict));
    }
    let gate = ownership._permit.0.clone();
    let (ownership, receipt) = receipts.lookup(ownership).await?;
    if !Arc::ptr_eq(&ownership._permit.0, &gate) {
        return Err(PairingStoreError::Domain(PairingError::Conflict));
    }
    match receipt {
        Some(outcome) => {
            let (credential, request) = current
                .stage_binding()
                .ok_or(PairingStoreError::Domain(PairingError::Conflict))?;
            if outcome.credential() != credential
                || outcome.request() != request
                || outcome.generation() != current.intent().generation()
            {
                return Err(PairingStoreError::Domain(PairingError::Conflict));
            }
            Ok(StageResolution::Receiver { ownership, outcome })
        }
        None => Ok(StageResolution::NoReceiver(NoReceiverCleanupProof {
            ownership,
        })),
    }
}
