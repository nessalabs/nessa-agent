//! Settle the receiver of an enrollment that ended after it was staged.
//!
//! ```text
//! Terminal + cleanup pending --acquire_stage--> StageOwnership (held throughout)
//!   receiver remembered ------------------------> fence --> finish_cleanup
//!   no receiver: lookup only (resolve_terminal_stage)
//!       original pair receipt --> remember_receiver --> fence --> finish_cleanup
//!       no receipt -------------> finish_no_receiver
//! ```
//! Arrows are calls, in order. Nothing here pairs a receiver: cleanup only looks
//! up the original receipt (design row P48). Auth keeps the first terminal
//! cause and initiator; this only completes the physical obligation.
use super::receivers::{PairingReceivers, ReceiverError, ReceiverRequest};
use nessa_auth::{
    application::{
        pairing::{
            resolve_terminal_stage, PairingStore, PairingStoreError, ReceiverOutcome,
            StageOwnership, StageReceiptLookup, StageResolution,
        },
        ports::{Clock, PortFuture},
    },
    domain::pairing::{InvitationId, PairingError, PairingRecord},
};
use std::sync::{Arc, Mutex};

/// Why an ended enrollment's cleanup did not complete. Its record keeps the
/// obligation (`cleanup_pending`) and its first cause either way.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CleanupError {
    /// The registry refused or failed, including `StageOccupied` while the
    /// original dispatch still owns the stage.
    Enrollment(PairingStoreError),
    /// The receiver authority refused or failed.
    Receiver(ReceiverError),
}

/// The cleanup use case. It needs no session: cleanup is the system finishing
/// a physical effect, and the receiver journal records it as such.
pub struct SettleCleanup<'a> {
    /// The registry, which owns the stage lease and the enrollment record.
    pub enrollments: &'a Arc<dyn PairingStore>,
    /// The canonical receiver authority.
    pub receivers: &'a dyn PairingReceivers,
    /// Wall clock for the registry's transitions.
    pub clock: &'a dyn Clock,
}
impl SettleCleanup<'_> {
    /// Settle `id` if it is an ended enrollment with cleanup pending; any other
    /// record is returned as it is.
    pub async fn execute(&self, id: InvitationId) -> Result<PairingRecord, CleanupError> {
        let record = self
            .enrollments
            .read_pairing(id)
            .map_err(CleanupError::Enrollment)?;
        if !record.cleanup_pending() {
            return Ok(record);
        }
        let stage = self
            .enrollments
            .clone()
            .acquire_stage(id)
            .map_err(CleanupError::Enrollment)?;
        self.with_stage(stage).await
    }

    /// Settle under a stage lease the caller already holds; activation hands
    /// over its own when the enrollment ended while it was working.
    pub async fn with_stage(&self, stage: StageOwnership) -> Result<PairingRecord, CleanupError> {
        let current = stage.current().map_err(CleanupError::Enrollment)?;
        if !current.cleanup_pending() {
            return Ok(current);
        }
        let id = current.id();
        let request = ReceiverRequest::for_stage(&current).ok_or(CleanupError::Enrollment(
            PairingStoreError::Domain(PairingError::Conflict),
        ))?;
        // The lease stays held until the fence is recorded.
        let (_stage, receiver, epoch) = match current.receiver_binding() {
            Some((receiver, epoch)) => (stage, receiver.clone(), epoch),
            None => {
                let receipts = Receipts {
                    receivers: self.receivers,
                    request: &request,
                    failed: Mutex::new(None),
                };
                let resolved = resolve_terminal_stage(stage, &receipts).await;
                // A receiver failure is the receiver's, on this path as on the
                // fence below; only Auth's own refusals are the registry's.
                if let Some(error) = receipts.failed.lock().ok().and_then(|failed| *failed) {
                    return Err(CleanupError::Receiver(error));
                }
                match resolved.map_err(CleanupError::Enrollment)? {
                    StageResolution::NoReceiver(proof) => {
                        return self
                            .enrollments
                            .finish_no_receiver(&proof, self.clock)
                            .map_err(CleanupError::Enrollment);
                    }
                    StageResolution::Receiver { ownership, outcome } => {
                        self.enrollments
                            .remember_receiver(id, &outcome, self.clock)
                            .map_err(CleanupError::Enrollment)?;
                        (ownership, outcome.receiver().clone(), outcome.epoch())
                    }
                }
            }
        };
        let fence = self
            .receivers
            .fence(&request, &receiver, epoch)
            .map_err(CleanupError::Receiver)?;
        self.enrollments
            .finish_cleanup(id, &fence, self.clock)
            .map_err(CleanupError::Enrollment)
    }
}

/// Lookup-only receipts for one stage, for Auth's terminal resolution. Auth's
/// port can only answer with its own error, so the receiver's is kept here.
struct Receipts<'a> {
    receivers: &'a dyn PairingReceivers,
    request: &'a ReceiverRequest,
    failed: Mutex<Option<ReceiverError>>,
}
impl StageReceiptLookup for Receipts<'_> {
    fn lookup<'a>(
        &'a self,
        stage: StageOwnership,
    ) -> PortFuture<'a, (StageOwnership, Option<ReceiverOutcome>), PairingStoreError> {
        Box::pin(async move {
            let receipt = self.receivers.paired(self.request).map_err(|error| {
                if let Ok(mut failed) = self.failed.lock() {
                    *failed = Some(error);
                }
                PairingStoreError::Unavailable
            })?;
            Ok((stage, receipt))
        })
    }
}
