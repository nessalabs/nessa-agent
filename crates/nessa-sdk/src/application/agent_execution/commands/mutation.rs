//! Durable submit and exact-turn Stop on the principal control stream.
use super::creation::{
    CreationFuture, CreationInitializationFailure, CreationStorage, CreationStorageError,
    CreationTaskFault,
};
use crate::application::agent_execution::{permissions::ActionContext, sessions::StorageError};
use crate::domain::agent_execution::{executions::ExecutionId, sessions::SessionId};
use std::sync::Arc;

/// Command family stored beside creation in one principal request namespace.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationOperation {
    /// Queue one turn. The effect is a single enqueue.
    Submit,
    /// Stop one captured turn. The effect is withdraw or cancel, never session close.
    Stop,
}

/// Durably acknowledged progress of one submit or stop.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationStage {
    /// Binding saved. No withdraw, cancel, or enqueue has been authorized.
    Accepted,
    /// The original effect was authorized. Absence of a terminal does not prove it failed.
    Attempted,
    /// The effect finished, or the turn was already final and nothing was sent.
    Settled,
}

/// What a settled submit or stop actually did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationOutcome {
    /// Submit enqueued the original turn once.
    Dispatched,
    /// Stop removed the named turn before dispatch.
    Withdrawn,
    /// Stop delivered cancellation for the named active turn.
    Cancelled,
    /// The named turn was already final. No withdraw or cancel was sent.
    AlreadyFinal,
}

/// Immutable principal/request binding for one submit or stop.
///
/// The same principal request namespace also holds creation. Another operation,
/// target, turn, origin, or fingerprint is a conflict. The host owns canonical
/// input encoding; only its fixed SHA-256 fingerprint is retained.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationBinding {
    actor: ActionContext,
    operation: MutationOperation,
    target: SessionId,
    turn_id: ExecutionId,
    fingerprint: [u8; 32],
}
impl MutationBinding {
    /// Bind verified origin, operation, target and turn to the canonical digest.
    ///
    /// # Errors
    /// Returns [`StorageError::Corrupt`] when `turn_id` is not a valid execution id.
    pub fn new(
        actor: ActionContext,
        operation: MutationOperation,
        target: SessionId,
        turn_id: &str,
        fingerprint: [u8; 32],
    ) -> Result<Self, StorageError> {
        let turn_id = ExecutionId::new(turn_id)
            .map_err(|_| StorageError::Corrupt("invalid command turn".into()))?;
        Ok(Self {
            actor,
            operation,
            target,
            turn_id,
            fingerprint,
        })
    }
    /// Host-verified principal, surface and request identity.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Submit or stop. A different family under this request is a conflict.
    pub fn operation(&self) -> MutationOperation {
        self.operation
    }
    /// Conversation that owns the turn.
    pub fn target(&self) -> &SessionId {
        &self.target
    }
    /// Captured turn. Stop never substitutes a newer one.
    pub fn turn_id(&self) -> &ExecutionId {
        &self.turn_id
    }
    /// Fixed digest of the host's canonical input, not the input itself.
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }
}

/// Non-content submit or stop facts retained across retries and target erasure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct MutationReceipt {
    binding: MutationBinding,
    stage: MutationStage,
    outcome: Option<MutationOutcome>,
}
impl MutationReceipt {
    /// First record of a command, before any effect is authorized.
    pub fn accepted(binding: MutationBinding) -> Self {
        Self {
            binding,
            stage: MutationStage::Accepted,
            outcome: None,
        }
    }
    /// Original request, operation, target, turn, fingerprint and origin.
    pub fn binding(&self) -> &MutationBinding {
        &self.binding
    }
    /// Acknowledged progress. `Attempted` becomes an interrupted response on retry.
    pub fn stage(&self) -> MutationStage {
        self.stage
    }
    /// Settled outcome. Absent until [`MutationStage::Settled`].
    pub fn outcome(&self) -> Option<MutationOutcome> {
        self.outcome
    }
    /// Make the next immutable progress record.
    ///
    /// `Accepted` may become `Attempted` with no outcome, or `Settled` with
    /// [`MutationOutcome::AlreadyFinal`] when classification proved nothing
    /// would be sent. `Attempted` may become `Settled` with the effect's outcome.
    ///
    /// # Errors
    /// Returns [`StorageError::Corrupt`] for a skipped, reversed, or outcome-less transition.
    pub fn advance(
        &self,
        stage: MutationStage,
        outcome: Option<MutationOutcome>,
    ) -> Result<Self, StorageError> {
        let legal = matches!(
            (self.stage, stage, outcome),
            (MutationStage::Accepted, MutationStage::Attempted, None)
                | (
                    MutationStage::Attempted,
                    MutationStage::Settled,
                    Some(
                        MutationOutcome::Dispatched
                            | MutationOutcome::Withdrawn
                            | MutationOutcome::Cancelled
                            | MutationOutcome::AlreadyFinal
                    )
                )
                | (
                    MutationStage::Accepted,
                    MutationStage::Settled,
                    Some(MutationOutcome::AlreadyFinal)
                )
        );
        if !legal {
            return Err(StorageError::Corrupt("invalid command transition".into()));
        }
        Ok(Self {
            binding: self.binding.clone(),
            stage,
            outcome,
        })
    }
}

/// Whether classification found an external effect to authorize.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum MutationPlan {
    /// Withdraw, cancel, or enqueue after `Attempted` is durable.
    Effect,
    /// The turn is already final. Save `Settled(AlreadyFinal)` and send nothing.
    AlreadyFinal,
}

/// Failure retains uncertainty separately from the host's typed cause.
#[derive(Debug)]
pub enum MutationFailure<E> {
    /// Storage refused or did not acknowledge command evidence.
    Storage(CreationStorageError),
    /// Supervised command task did not return its typed result.
    TaskFault(CreationTaskFault),
    /// Existing request identity names a different operation, target, turn, bytes, or origin.
    Conflict,
    /// The original attempted effect has no saved success. No effect is repeated.
    Interrupted(Box<MutationReceipt>),
    /// Current host refusal or effect failure. Original cause is retained.
    Target(E),
}

/// Current target authority and the one effect supplied by the host.
pub trait MutationTarget: Send + Sync {
    /// The host's typed access, deletion, and effect failures.
    type Error: Send + 'static;
    /// Recheck current target access and deletion before an effect and before
    /// a saved receipt is returned.
    fn check(
        &self,
        binding: &MutationBinding,
        stage: Option<MutationStage>,
    ) -> CreationFuture<'_, (), Self::Error>;
    /// Classify the named turn before any effect. `Err` means nothing was sent,
    /// including a provider that cannot cancel a turn.
    fn classify(&self, binding: &MutationBinding) -> CreationFuture<'_, MutationPlan, Self::Error>;
    /// Perform the original effect. Called only after acknowledged `Attempted`.
    fn effect(
        &self,
        binding: &MutationBinding,
    ) -> CreationFuture<'_, MutationOutcome, CreationInitializationFailure<Self::Error>>;
}

/// Orders one durable submit or stop and supervises it after its caller disappears.
pub struct MutationCoordinator {
    storage: Arc<dyn CreationStorage>,
}
impl MutationCoordinator {
    /// Receive the shared principal control storage.
    pub fn new(storage: Arc<dyn CreationStorage>) -> Self {
        Self { storage }
    }

    /// Save the binding and original attempt before the effect, then its result.
    /// Exact settled retries return the saved receipt after a current target check.
    /// A restored attempt returns [`MutationFailure::Interrupted`] and does not
    /// repeat the effect. Dropping a polled future leaves the operation running.
    ///
    /// # Errors
    /// A conflicting binding, storage failure, current target refusal, or attempted
    /// original effect without a saved terminal returns its distinct typed variant.
    pub async fn execute<T: MutationTarget + 'static>(
        &self,
        binding: MutationBinding,
        target: Arc<T>,
    ) -> Result<MutationReceipt, MutationFailure<T::Error>> {
        let storage = self.storage.clone();
        tokio::spawn(async move {
            let lease = storage
                .open_creation(binding.actor().principal_id().into(), true)
                .await
                .map_err(MutationFailure::Storage)?
                .ok_or_else(|| {
                    MutationFailure::Storage(CreationStorageError::Storage(StorageError::Corrupt(
                        "command stream absent".into(),
                    )))
                })?;
            let saved = match lease.load_mutation(binding.actor().request_id()).await {
                Ok(saved) => saved,
                Err(CreationStorageError::ForeignRequest) => return Err(MutationFailure::Conflict),
                Err(error) => return Err(MutationFailure::Storage(error)),
            };
            let receipt = match saved {
                Some(saved) if saved.binding() != &binding => {
                    return Err(MutationFailure::Conflict)
                }
                Some(saved) => saved,
                None => {
                    let accepted = MutationReceipt::accepted(binding.clone());
                    lease
                        .save_mutation(accepted.clone())
                        .await
                        .map_err(stored_mutation)?;
                    accepted
                }
            };
            target
                .check(&binding, Some(receipt.stage()))
                .await
                .map_err(MutationFailure::Target)?;
            match receipt.stage() {
                MutationStage::Settled => Ok(receipt),
                MutationStage::Attempted => Err(MutationFailure::Interrupted(Box::new(receipt))),
                MutationStage::Accepted => {
                    match target
                        .classify(&binding)
                        .await
                        .map_err(MutationFailure::Target)?
                    {
                        MutationPlan::AlreadyFinal => {
                            let settled = receipt
                                .advance(
                                    MutationStage::Settled,
                                    Some(MutationOutcome::AlreadyFinal),
                                )
                                .map_err(|error| MutationFailure::Storage(error.into()))?;
                            lease
                                .save_mutation(settled.clone())
                                .await
                                .map_err(stored_mutation)?;
                            target
                                .check(&binding, Some(settled.stage()))
                                .await
                                .map_err(MutationFailure::Target)?;
                            Ok(settled)
                        }
                        MutationPlan::Effect => {
                            let attempted = receipt
                                .advance(MutationStage::Attempted, None)
                                .map_err(|error| MutationFailure::Storage(error.into()))?;
                            lease
                                .save_mutation(attempted.clone())
                                .await
                                .map_err(stored_mutation)?;
                            let outcome =
                                target
                                    .effect(&binding)
                                    .await
                                    .map_err(|failure| match failure {
                                        CreationInitializationFailure::Target(error) => {
                                            MutationFailure::Target(error)
                                        }
                                        CreationInitializationFailure::TaskFault(fault) => {
                                            MutationFailure::TaskFault(fault)
                                        }
                                    })?;
                            let settled = attempted
                                .advance(MutationStage::Settled, Some(outcome))
                                .map_err(|error| MutationFailure::Storage(error.into()))?;
                            lease
                                .save_mutation(settled.clone())
                                .await
                                .map_err(stored_mutation)?;
                            target
                                .check(&binding, Some(settled.stage()))
                                .await
                                .map_err(MutationFailure::Target)?;
                            Ok(settled)
                        }
                    }
                }
            }
        })
        .await
        .map_err(|error| MutationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }

    /// Read original progress without admitting an effect or appending records.
    /// The host checks current deletion before any saved receipt is returned.
    ///
    /// # Errors
    /// Same typed conflict, storage, or host refusal as [`Self::execute`].
    /// An absent principal or request returns `None`. An attempted record is
    /// returned as evidence and its effect is not performed.
    pub async fn lookup<T: MutationTarget>(
        &self,
        binding: &MutationBinding,
        target: &T,
    ) -> Result<Option<MutationReceipt>, MutationFailure<T::Error>> {
        let lease = self
            .storage
            .open_creation(binding.actor().principal_id().into(), false)
            .await
            .map_err(MutationFailure::Storage)?;
        let saved = match lease {
            Some(lease) => match lease.load_mutation(binding.actor().request_id()).await {
                Ok(saved) => saved,
                Err(CreationStorageError::ForeignRequest) => return Err(MutationFailure::Conflict),
                Err(error) => return Err(MutationFailure::Storage(error)),
            },
            None => None,
        };
        if saved
            .as_ref()
            .is_some_and(|saved| saved.binding() != binding)
        {
            return Err(MutationFailure::Conflict);
        }
        target
            .check(binding, saved.as_ref().map(MutationReceipt::stage))
            .await
            .map_err(MutationFailure::Target)?;
        Ok(saved)
    }
}

fn stored_mutation<E>(error: CreationStorageError) -> MutationFailure<E> {
    match error {
        CreationStorageError::ForeignRequest => MutationFailure::Conflict,
        other => MutationFailure::Storage(other),
    }
}
