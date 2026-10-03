//! Owner approval carried through to an issued device credential.
//!
//! ```text
//! decide(Approve) --> Approved --stage_pairing (fresh admission)--> Staging
//!   --acquire_stage--> StageOwnership, held until the end
//!   --receivers.pair (same stage correlation on every retry)--> remember_receiver
//!   --receivers.current: still the paired receiver, active--> publish_pairing (fresh admission)
//!   --> Active
//! ```
//! Arrows are calls, in order (design rows P19–P25, O5, O6). Auth decides every
//! transition; the receiver authority owns the receiver. A step that does not
//! complete leaves the record where it stands, and approving again continues
//! from there with the same stage, never a second receiver or credential.
use super::cleanup::{CleanupError, SettleCleanup};
use super::owner::{OwnerError, PairingOwner};
use super::receivers::{ReceiverError, ReceiverRequest};
use nessa_auth::{
    application::{
        pairing::{OwnerDecision, PairingStoreError, StageOwnership},
        session::AuthenticatedSession,
    },
    domain::{
        pairing::{AttemptId, DeviceKey, InvitationId, PairingError, PairingPhase, PairingRecord},
        CredentialId,
    },
};

/// Server-minted identities for a stage, used only if the record has none yet.
/// A retry keeps the stage the record already has.
pub struct FreshStage {
    /// The credential identity to reserve.
    pub credential: CredentialId,
    /// The stage correlation every receiver call is found by.
    pub request: AttemptId,
}

/// Why activation stopped before Active. The approval stands; the record says
/// where activation got to.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ActivationError {
    /// Current authorization or the registry refused a step.
    Owner(OwnerError),
    /// The receiver authority refused or failed.
    Receiver(ReceiverError),
    /// The credential's current receiver is no longer the one it was paired
    /// with, active, at that epoch or later (design rows O6, P46).
    ReceiverNotCurrent,
    /// The enrollment ended during activation and its cleanup did not complete.
    Cleanup(CleanupError),
}

/// What an approval produced: the record as it now stands and, if activation
/// stopped short of Active, why.
pub struct Approval {
    /// The canonical record after the approval and any activation steps.
    pub record: PairingRecord,
    /// Why activation stopped, if it did. `None` for Active, or for a record
    /// whose cleanup completed because it ended meanwhile.
    pub stopped: Option<ActivationError>,
}

impl PairingOwner<'_> {
    /// Approve the exact claimed key, then stage, pair the receiver and publish
    /// the credential. A record already Active is the historical receipt and is
    /// returned without a new stage. A step that does not complete is reported
    /// in `stopped`; the decision is not undone and nothing is rolled back.
    pub async fn approve(
        &self,
        session: &AuthenticatedSession,
        id: InvitationId,
        key: DeviceKey,
        fresh: FreshStage,
    ) -> Result<Approval, OwnerError> {
        let decided = self
            .decide(session, id, OwnerDecision::Approve(key))
            .await?;
        if !matches!(
            decided.phase(),
            PairingPhase::Approved | PairingPhase::Staging
        ) {
            return Ok(Approval {
                record: decided,
                stopped: None,
            });
        }
        match self.activate(session, decided, fresh).await {
            Ok(record) => Ok(Approval {
                record,
                stopped: None,
            }),
            Err(stopped) => Ok(Approval {
                record: self
                    .enrollments
                    .read_pairing(id)
                    .map_err(OwnerError::Enrollment)?,
                stopped: Some(stopped),
            }),
        }
    }

    async fn activate(
        &self,
        session: &AuthenticatedSession,
        record: PairingRecord,
        fresh: FreshStage,
    ) -> Result<PairingRecord, ActivationError> {
        let id = record.id();
        if record.phase() == PairingPhase::Approved {
            // The decision advanced the revision, so this admission is fresh.
            let admission = self
                .admit(session, id)
                .await
                .map_err(ActivationError::Owner)?;
            self.enrollments
                .stage_pairing(id, fresh.credential, fresh.request, &admission, self.clock)
                .map_err(store)?;
        }
        // Held until this activation ends, so cleanup or another approval
        // cannot dispatch the same stage meanwhile (design row P48).
        let stage = self.enrollments.clone().acquire_stage(id).map_err(store)?;
        match self.pair_and_publish(session, &stage).await {
            Ok(record) => Ok(record),
            // Ended during activation: a late receiver was remembered above,
            // and is settled here under the same lease (design rows P23, A8).
            Err(stopped) => match stage.current() {
                Ok(now) if now.cleanup_pending() => SettleCleanup {
                    enrollments: self.enrollments,
                    receivers: self.receivers,
                    clock: self.clock,
                }
                .with_stage(stage)
                .await
                .map_err(ActivationError::Cleanup),
                _ => Err(stopped),
            },
        }
    }

    /// Pair the stage's receiver unless it has one, then publish if that
    /// receiver is still the credential's, active, at its paired epoch or later.
    async fn pair_and_publish(
        &self,
        session: &AuthenticatedSession,
        stage: &StageOwnership,
    ) -> Result<PairingRecord, ActivationError> {
        let mut current = stage.current().map_err(store)?;
        let id = current.id();
        let request = ReceiverRequest::for_stage(&current).ok_or_else(conflict)?;
        if current.receiver_binding().is_none() {
            let outcome = self
                .receivers
                .pair(&request)
                .map_err(ActivationError::Receiver)?;
            // Auth correlates the outcome with the stage (design row P21).
            current = self
                .enrollments
                .remember_receiver(id, &outcome, self.clock)
                .map_err(store)?;
        }
        let (receiver, epoch) = current.receiver_binding().ok_or_else(conflict)?;
        let live = self
            .receivers
            .current(request.credential())
            .map_err(ActivationError::Receiver)?;
        let still_paired = live.is_some_and(|live| {
            live.active
                && &live.receiver == receiver
                && live.epoch >= epoch
                && &live.organization == request.organization()
                && &live.owner == request.owner()
        });
        if !still_paired {
            return Err(ActivationError::ReceiverNotCurrent);
        }
        // Stage and receiver advanced the revision: admit again, fresh.
        let admission = self
            .admit(session, id)
            .await
            .map_err(ActivationError::Owner)?;
        self.enrollments
            .publish_pairing(id, &admission, self.clock)
            .map_err(store)
    }
}

fn store(error: PairingStoreError) -> ActivationError {
    ActivationError::Owner(OwnerError::Enrollment(error))
}
fn conflict() -> ActivationError {
    store(PairingStoreError::Domain(PairingError::Conflict))
}
