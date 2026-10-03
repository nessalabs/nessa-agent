use super::{
    ConfirmedClaim, DeviceConnectionProof, GatewayKeyStore, NoReceiverCleanupProof,
    PairingAdmission, PrivateKeyMaterial, PrivateStateError, ReceiverOutcome, StageOwnership,
};
use crate::domain::CredentialId;
use crate::{
    application::ports::{Clock, PortFuture},
    domain::pairing::{
        AttemptFailure, AttemptId, DeviceKey, InvitationId, PairingError, PairingRecord,
    },
};
use std::{fmt, sync::Arc};
/// Unexpected physical pairing worker termination, without secret diagnostics.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingWorkerFault {
    /// The owned closure panicked before returning verified evidence.
    Panic,
    /// The runtime cancelled its physical job before completion.
    Cancelled,
}
/// Enrollment storage failures without secret or provider payloads.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PairingStoreError {
    /// Original physical stage/cleanup ownership has not yet drained.
    StageOccupied,
    /// Missing native gateway key cannot be recreated after any canonical enrollment history.
    GatewayKeyHistoryExists,
    /// Private key publication retains its typed external effect/audit meaning.
    PrivateState(PrivateStateError),
    /// Physical lookup failed unexpectedly; no absent-effect proof exists.
    WorkerFault(PairingWorkerFault),
    /// An authoritative decision no longer matches the committed revision.
    StaleRevision,
    /// No matching enrollment exists.
    NotFound,
    /// Storage or validation is unavailable.
    Unavailable,
    /// Enrollment transition or identity is ineligible.
    Domain(PairingError),
}
impl fmt::Display for PairingStoreError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{self:?}")
    }
}
impl std::error::Error for PairingStoreError {}
/// Receipt of charging an attempt; exact retries do not restart physical crypto work.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum AttemptReservation {
    /// Newly charged and admitted original handshake.
    Admitted(PairingRecord),
    /// Previously admitted identical input; return its status without a new worker.
    Existing(PairingRecord),
}
/// Explicit owner consent targets the exact claimed device key.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwnerDecision {
    /// Approve this key for the admission's exact intent.
    Approve(DeviceKey),
    /// Deny the enrollment, preserving this first terminal cause.
    Deny,
    /// Cancel or revoke the enrollment.
    Cancel,
}
/// Runtime-only termination; the canonical domain checks deadline/phase.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RuntimeEnd {
    /// The injected clock reached the invitation's exclusive deadline.
    Expired,
    /// Gateway startup lost volatile PAKE state for an Available invitation.
    Restarted,
}
/// Enrollment records share the credential authority's transaction and revision.
pub trait PairingStore: Send + Sync {
    /// Admit first native key publication while canonical all-history remains empty.
    /// The mutation owner retains its guard through injected private publication.
    fn publish_first_gateway_key(
        &self,
        keys: &dyn GatewayKeyStore,
        key: &PrivateKeyMaterial,
        clock: &dyn Clock,
    ) -> Result<(), PairingStoreError>;
    /// Acquire exclusive physical dispatch/lookup ownership for an existing stage.
    /// The lease retains the registry handle and its process lock through physical work.
    fn acquire_stage(
        self: Arc<Self>,
        id: InvitationId,
    ) -> Result<StageOwnership, PairingStoreError>;
    /// Complete an exact terminal stage whose original physical receipt is absent.
    /// The private proof holds drained-dispatch exclusion through this commit.
    fn finish_no_receiver(
        &self,
        proof: &NoReceiverCleanupProof,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;

    /// Read bounded canonical unfinished records for startup/expiry reconciliation.
    /// Registry receipt limits bound the returned collection; this is not a wire API.
    fn pending_pairings(&self) -> Result<Box<[PairingRecord]>, PairingStoreError>;
    /// Conditionally expire against the current locked record and clock.
    /// A valid Active, Terminal or not-due record is returned unchanged.
    fn expire_pairing_if_due(
        &self,
        id: InvitationId,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;
    /// End eligible runtime work under the mutation owner. An existing terminal
    /// cause is retained; Active credentials cannot expire/restart by this method.
    fn end_pairing(
        &self,
        id: InvitationId,
        cause: RuntimeEnd,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;

    /// Read an immutable validated enrollment; this is an internal owner port, not a public status route.
    fn read_pairing(&self, id: InvitationId) -> Result<PairingRecord, PairingStoreError>;
    /// Create an invitation only if this exact current policy decision still holds.
    /// Sample the injected clock after acquiring the mutation lock. Failure publishes nothing.
    fn create_pairing<'a>(
        &'a self,
        record: &'a PairingRecord,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError>;
    /// Commit owner consent only against the current exact-intent admission.
    /// A duplicate decision returns its committed outcome; a competing cause cannot overwrite it.
    fn decide_pairing<'a>(
        &'a self,
        id: InvitationId,
        decision: OwnerDecision,
        admission: &'a PairingAdmission,
        clock: &'a dyn Clock,
    ) -> PortFuture<'a, PairingRecord, PairingStoreError>;
    /// Charge one exact KE1 input under a proved device key before crypto dispatch.
    fn reserve_attempt(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
        input: [u8; 32],
        clock: &dyn Clock,
    ) -> Result<AttemptReservation, PairingStoreError>;
    /// Commit a mutually confirmed claim before exposing its protected receipt.
    fn confirm_claim(
        &self,
        proof: &ConfirmedClaim,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;
    /// Settle an original admitted physical handshake with its typed outcome.
    fn fail_attempt(
        &self,
        id: InvitationId,
        attempt: AttemptId,
        device: &DeviceConnectionProof,
        cause: AttemptFailure,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;
    /// Reserve a unique credential and durable receiver dispatch before any receiver effect.
    fn stage_pairing(
        &self,
        id: InvitationId,
        credential: CredentialId,
        request: AttemptId,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;
    /// Retain a physically confirmed canonical receiver outcome, including after cancellation.
    fn remember_receiver(
        &self,
        id: InvitationId,
        outcome: &ReceiverOutcome,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;
    /// Publish the exact key credential and Active transition in one fresh-authority commit.
    fn publish_pairing(
        &self,
        id: InvitationId,
        admission: &PairingAdmission,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;
    /// Record an exact physically confirmed fence after a terminal stage.
    fn finish_cleanup(
        &self,
        id: InvitationId,
        outcome: &ReceiverOutcome,
        clock: &dyn Clock,
    ) -> Result<PairingRecord, PairingStoreError>;
}
