use crate::application::agent_execution::{permissions::ActionContext, sessions::StorageError};
use crate::domain::agent_execution::sessions::SessionId;
use std::{future::Future, pin::Pin, sync::Arc};

/// An asynchronous command boundary with a typed result supplied by its owner.
pub type CreationFuture<'a, T, E = CreationStorageError> =
    Pin<Box<dyn Future<Output = Result<T, E>> + Send + 'a>>;

/// Unexpected completion of a supervised command or control-storage task.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationTaskFault {
    /// The original task unwound; its saved progress remains authoritative.
    Panicked,
    /// The original task was cancelled without reporting its own outcome.
    Cancelled,
}
impl CreationTaskFault {
    /// Translate Tokio's closed task-failure cases without parsing diagnostics.
    pub fn from_join_error(error: tokio::task::JoinError) -> Self {
        if error.is_panic() {
            Self::Panicked
        } else {
            Self::Cancelled
        }
    }
}
/// Control storage preserves backend failures separately from task failures.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum CreationStorageError {
    /// Original storage or restored-evidence refusal.
    Storage(StorageError),
    /// Unexpected task failure, without a fabricated backend diagnostic.
    TaskFault(CreationTaskFault),
    /// This principal request already names a different command family.
    ForeignRequest,
}
impl From<StorageError> for CreationStorageError {
    fn from(error: StorageError) -> Self {
        Self::Storage(error)
    }
}
/// Immutable principal/request binding for one conversation creation.
///
/// The operation is conversation creation, so another command family must use
/// the same principal request namespace rather than quietly reusing its identity.
/// The host owns canonical input encoding; only its fixed SHA-256 fingerprint is
/// retained here. Construction does not authenticate the supplied actor.
///
/// ```
/// use nessa_sdk::application::agent_execution::{commands::{CreationBinding, CreationReceipt, CreationStage}, permissions::ActionContext};
/// use nessa_sdk::domain::agent_execution::sessions::SessionId;
/// let binding = CreationBinding::new(
///     ActionContext::new("verified-principal", "panel", "original-request").unwrap(),
///     SessionId::new("original-conversation").unwrap(),
///     [7; 32],
/// );
/// let accepted = CreationReceipt::accepted(binding);
/// let attempted = accepted.advance(CreationStage::Attempted).unwrap();
/// assert_eq!(attempted.binding(), accepted.binding());
/// ```
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationBinding {
    actor: ActionContext,
    target: SessionId,
    fingerprint: [u8; 32],
}
impl CreationBinding {
    /// Bind verified origin and original target to the canonical-input digest.
    /// No provider or storage work occurs. Raw configuration is not retained.
    pub fn new(actor: ActionContext, target: SessionId, fingerprint: [u8; 32]) -> Self {
        Self {
            actor,
            target,
            fingerprint,
        }
    }
    /// Host-verified principal, logical request identity and original surface.
    pub fn actor(&self) -> &ActionContext {
        &self.actor
    }
    /// Original conversation identity, also returned after interruption.
    pub fn target(&self) -> &SessionId {
        &self.target
    }
    /// Fixed digest of the host's canonical input, not the input itself.
    pub fn fingerprint(&self) -> &[u8; 32] {
        &self.fingerprint
    }
}

/// Durably acknowledged progress of the original initialization attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CreationStage {
    /// Binding saved; no initialization has been authorized by this coordinator.
    Accepted,
    /// Original effect authorized; absence of a terminal does not prove failure.
    Attempted,
    /// Initializer returned success and the terminal receipt was saved.
    Ready,
}

/// Non-content facts retained across retries and target erasure.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct CreationReceipt {
    binding: CreationBinding,
    stage: CreationStage,
}
impl CreationReceipt {
    /// Create the first record of a command, before authorizing initialization.
    pub fn accepted(binding: CreationBinding) -> Self {
        Self {
            binding,
            stage: CreationStage::Accepted,
        }
    }
    /// Original request, target, fingerprint and origin.
    pub fn binding(&self) -> &CreationBinding {
        &self.binding
    }
    /// Acknowledged progress; `Attempted` becomes a truthful interrupted response
    /// on retry, rather than authority to call the initializer again.
    pub fn stage(&self) -> CreationStage {
        self.stage
    }
    /// Make the next immutable progress record.
    ///
    /// # Errors
    /// Returns `StorageError::Corrupt` for a skipped or reversed transition.
    pub fn advance(&self, stage: CreationStage) -> Result<Self, StorageError> {
        if !matches!(
            (self.stage, stage),
            (CreationStage::Accepted, CreationStage::Attempted)
                | (CreationStage::Attempted, CreationStage::Ready)
        ) {
            return Err(StorageError::Corrupt("invalid creation transition".into()));
        }
        Ok(Self {
            binding: self.binding.clone(),
            stage,
        })
    }
}

/// Durable principal control stream, implemented by the existing record runtime.
/// Opening serializes principal commands without a waiting queue. A read-only
/// open must not create an absent stream. The returned lease owns exclusion until
/// drop; its outstanding operations retain that ownership through completion.
pub trait CreationStorage: Send + Sync {
    /// Open the host-verified principal namespace. `create` selects writable
    /// admission or read-only lookup; absent lookup returns `None`.
    fn open_creation(
        &self,
        principal: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>>;
}

/// Exclusive access to one principal's immutable command history.
pub trait CreationStorageLease: Send + Sync {
    /// Return the existing creation binding/progress for `request_id`, without writing.
    ///
    /// # Errors
    /// [`CreationStorageError::ForeignRequest`] when the id belongs to submit or stop.
    fn load(&self, request_id: &str) -> CreationFuture<'_, Option<CreationReceipt>>;
    /// Append one legal creation transition, or acknowledge an exact retained event.
    /// Conflicts and impossible histories are refused before replacement. Errors
    /// may mean an append committed without its answer; they do not prove rollback.
    fn save(&self, receipt: CreationReceipt) -> CreationFuture<'_, ()>;
    /// Return the existing submit or stop receipt for `request_id`, without writing.
    ///
    /// # Errors
    /// [`CreationStorageError::ForeignRequest`] when the id belongs to creation.
    fn load_mutation(
        &self,
        _request_id: &str,
    ) -> CreationFuture<'_, Option<super::mutation::MutationReceipt>> {
        Box::pin(async {
            Err(CreationStorageError::Storage(StorageError::Corrupt(
                "mutation is not on this lease".into(),
            )))
        })
    }
    /// Append one legal submit or stop transition, or acknowledge an exact retained event.
    /// Leases that only store creation refuse the append.
    fn save_mutation(&self, _receipt: super::mutation::MutationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async {
            Err(CreationStorageError::Storage(StorageError::Corrupt(
                "mutation is not on this lease".into(),
            )))
        })
    }
}

/// Current target authority and actual initialization supplied by the host.
/// Authentication has already occurred. Implementations keep target configuration
/// under its own retention owner and report their original typed failure.
pub trait CreationTarget: Send + Sync {
    /// The host's typed access, deletion, initialization and cleanup failures.
    type Error: Send + 'static;
    /// Recheck current target access/deletion against saved progress (`None` for
    /// an absent receipt), including before saved-ready or
    /// interrupted responses. An absent target may be allowed for a new creation.
    fn check(
        &self,
        binding: &CreationBinding,
        stage: Option<CreationStage>,
    ) -> CreationFuture<'_, (), Self::Error>;
    /// Perform the existing target/provider initialization for the original
    /// binding. Called only after acknowledged `Attempted`; it is not a lookup.
    /// Success acknowledges provider initialization and its required publication,
    /// not just admission or preparation. Join the original initialization owner
    /// and preserve its typed task fault separately from the host's refusal.
    fn initialize(
        &self,
        binding: &CreationBinding,
    ) -> CreationFuture<'_, (), CreationInitializationFailure<Self::Error>>;
}

/// Initializer refusal or unexpected completion of its original supervised task.
/// This port cannot manufacture a coordinator receipt or replace its progress.
#[derive(Debug)]
pub enum CreationInitializationFailure<E> {
    /// Original host access, provider, publication or cleanup failure.
    Target(E),
    /// The original initialization supervisor did not return its own outcome.
    TaskFault(CreationTaskFault),
}

/// Failure retains uncertainty separately from the host's own typed cause.
#[derive(Debug)]
pub enum CreationFailure<E> {
    /// Storage refused or did not acknowledge command evidence.
    Storage(CreationStorageError),
    /// Supervised command task did not return its typed result.
    TaskFault(CreationTaskFault),
    /// Existing request identity names different target, bytes or origin.
    Conflict,
    /// The original attempted effect has no saved success. No effect is repeated.
    Interrupted(Box<CreationReceipt>),
    /// Current host refusal or initialization failure; original cause is retained.
    Target(E),
}

/// Orders one durable creation and supervises it after its caller disappears.
///
/// `create` retains the original lease and host initializer in one task. Runtime
/// shutdown can interrupt it; the saved `Attempted` then prevents blind replay.
/// This does not reconcile an external provider whose original outcome is unknown.
pub struct CreationCoordinator {
    storage: Arc<dyn CreationStorage>,
}
impl CreationCoordinator {
    /// Receive the shared control storage from composition or the host entry point.
    pub fn new(storage: Arc<dyn CreationStorage>) -> Self {
        Self { storage }
    }

    /// Save the binding and original attempt before initialization, then its result.
    /// Exact ready retries return the saved receipt after a current target check.
    /// Once this future has been polled and spawned its operation, dropping the
    /// waiting future leaves that same operation running. An unpolled future
    /// admits no work.
    ///
    /// # Errors
    /// A conflicting binding, storage failure, current target refusal, or attempted
    /// original effect without a saved terminal returns its distinct typed variant.
    pub async fn create<T: CreationTarget + 'static>(
        &self,
        binding: CreationBinding,
        target: Arc<T>,
    ) -> Result<CreationReceipt, CreationFailure<T::Error>> {
        let storage = self.storage.clone();
        tokio::spawn(async move {
            let lease = storage
                .open_creation(binding.actor().principal_id().into(), true)
                .await
                .map_err(CreationFailure::Storage)?
                .ok_or_else(|| {
                    CreationFailure::Storage(CreationStorageError::Storage(StorageError::Corrupt(
                        "creation stream absent".into(),
                    )))
                })?;
            let saved = match lease.load(binding.actor().request_id()).await {
                Ok(saved) => saved,
                Err(CreationStorageError::ForeignRequest) => return Err(CreationFailure::Conflict),
                Err(error) => return Err(CreationFailure::Storage(error)),
            };
            let receipt = match saved {
                Some(saved) if saved.binding() != &binding => {
                    return Err(CreationFailure::Conflict)
                }
                Some(saved) => saved,
                None => {
                    let accepted = CreationReceipt::accepted(binding.clone());
                    lease.save(accepted.clone()).await.map_err(stored)?;
                    accepted
                }
            };
            target
                .check(&binding, Some(receipt.stage()))
                .await
                .map_err(CreationFailure::Target)?;
            match receipt.stage() {
                CreationStage::Ready => Ok(receipt),
                CreationStage::Attempted => Err(CreationFailure::Interrupted(Box::new(receipt))),
                CreationStage::Accepted => {
                    let attempted = receipt
                        .advance(CreationStage::Attempted)
                        .map_err(|error| CreationFailure::Storage(error.into()))?;
                    lease.save(attempted.clone()).await.map_err(stored)?;
                    target
                        .initialize(&binding)
                        .await
                        .map_err(|failure| match failure {
                            CreationInitializationFailure::Target(error) => {
                                CreationFailure::Target(error)
                            }
                            CreationInitializationFailure::TaskFault(fault) => {
                                CreationFailure::TaskFault(fault)
                            }
                        })?;
                    let ready = attempted
                        .advance(CreationStage::Ready)
                        .map_err(|error| CreationFailure::Storage(error.into()))?;
                    lease.save(ready.clone()).await.map_err(stored)?;
                    target
                        .check(&binding, Some(ready.stage()))
                        .await
                        .map_err(CreationFailure::Target)?;
                    Ok(ready)
                }
            }
        })
        .await
        .map_err(|error| CreationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }

    /// Read original progress without admitting initialization or appending records.
    /// The host checks current deletion before any saved receipt is returned.
    ///
    /// # Errors
    /// Same typed conflict, storage or host refusal as [`Self::create`]. An absent
    /// principal/request returns `None`; an attempted record is returned as evidence
    /// with `CreationStage::Attempted`, without performing its external action.
    pub async fn lookup<T: CreationTarget>(
        &self,
        binding: &CreationBinding,
        target: &T,
    ) -> Result<Option<CreationReceipt>, CreationFailure<T::Error>> {
        let lease = self
            .storage
            .open_creation(binding.actor().principal_id().into(), false)
            .await
            .map_err(CreationFailure::Storage)?;
        let saved = match lease {
            Some(lease) => match lease.load(binding.actor().request_id()).await {
                Ok(saved) => saved,
                Err(CreationStorageError::ForeignRequest) => return Err(CreationFailure::Conflict),
                Err(error) => return Err(CreationFailure::Storage(error)),
            },
            None => None,
        };
        if saved
            .as_ref()
            .is_some_and(|saved| saved.binding() != binding)
        {
            return Err(CreationFailure::Conflict);
        }
        target
            .check(binding, saved.as_ref().map(CreationReceipt::stage))
            .await
            .map_err(CreationFailure::Target)?;
        Ok(saved)
    }
}

fn stored<E>(error: CreationStorageError) -> CreationFailure<E> {
    match error {
        CreationStorageError::ForeignRequest => CreationFailure::Conflict,
        other => CreationFailure::Storage(other),
    }
}
