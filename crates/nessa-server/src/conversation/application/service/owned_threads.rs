//! Gateway ownership composition; ordinary turn Stop never ends these lifetimes.
//!
//! Ordering: thread admission -> verify exact source -> coordinator reservation
//! -> prepare ordinary conversation -> install existing gate -> Ready. Stop uses
//! the same admission boundary when capturing its exact execution selection.

use super::super::environment::{confirmed_despite, AgentEnd};
use super::super::AgentToolSource;
use super::mutation::SubmittedCommand;
use super::{
    conversation_session, ConversationCaller, ConversationError, ConversationId,
    ConversationService, Inner, RequestedAgent, RequestedConversation, SubmissionMode,
    SubmittedMessage,
};
use async_trait::async_trait;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_sdk::application::agent_execution::agents::OwnedLifetime;
use nessa_sdk::application::agent_execution::commands::{
    CreationFailure, CreationReceipt, CreationStorage, CreationTaskFault, MutationFailure,
};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::application::agent_execution::sessions::CommittedViewState;
use nessa_sdk::application::agent_execution::subagents::{
    BindResourcesRefusal, ChildFactory, ChildResources, CloseCommand, InitialSubmit,
    OwnershipCoordinator, PortFailure, PrepareFailure, PrepareRequest, PreparedChild,
    ResourceReport, SpawnCommand,
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, Initiation};
use nessa_sdk::domain::agent_execution::leases::{LeaseCleanup, LeaseEndCause, LeasePhase};
use nessa_sdk::domain::agent_execution::subagents::{
    AgentLifetimeId, ApprovalPolicy, EvidenceFact, HostActor, Initiator, KnownMilestone,
    LifetimeCause, LifetimeState, OwnershipSnapshot, PhysicalFact, PolicyRead, SpawnBinding,
    SpawnOrigin, SpawnProgress, SpawnRequestId, TaskReceiptId,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Weak};
use std::time::Duration;
pub(super) struct Ownership {
    pub coordinator: OwnershipCoordinator,
    pub cascade: bool,
}

/// The factory's existing transfer slot retains the actual cleanup capability
/// acquired by creation, separately from its not-yet-installed lifetime gate.
pub(super) struct PendingPreparation {
    gate: Arc<dyn OwnedLifetime>,
    resources: Option<PreparationResources>,
}

struct PreparationResources {
    resources: Arc<dyn ChildResources>,
    opening: Arc<super::Slot>,
}

/// The original opening owner confirmed it acquired nothing. Retain its
/// immutable result so later slot removal/reopening cannot supply that fact.
struct UnacquiredPreparation {
    opening: Arc<super::Slot>,
}
#[async_trait]
impl ChildResources for UnacquiredPreparation {
    async fn close(&self, _: &LifetimeCause, _: &Initiator) -> ResourceReport {
        match self.opening.value.get() {
            Some(Err(failure)) if !failure.holds => ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            },
            _ => ResourceReport {
                physical: PhysicalFact::Failed,
                evidence: EvidenceFact::Pending,
            },
        }
    }
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Preparation {
    organization: String,
    principal: String,
    surface: String,
    action: String,
    agent: Option<String>,
    model: Option<String>,
    environment: Option<String>,
}
impl Preparation {
    fn caller(&self) -> Result<ConversationCaller, ConversationError> {
        Ok(ConversationCaller {
            organization_id: OrganizationId::new(&self.organization)
                .map_err(|_| ConversationError::InvalidInput)?,
            principal_id: PrincipalId::new(&self.principal)
                .map_err(|_| ConversationError::InvalidInput)?,
            surface_id: self.surface.clone(),
            action_id: self.action.clone(),
        })
    }
    fn requested(&self) -> RequestedConversation {
        RequestedConversation {
            agent: self
                .agent
                .as_deref()
                .and_then(AgentId::parse)
                .map(RequestedAgent::Known),
            model: self.model.clone(),
            approval_mode: None,
            environment: self.environment.clone(),
        }
    }
}

impl ConversationService {
    /// Prepare-only ordinary creation adapter for the existing SDK coordinator.
    /// It holds no strong service reference, and dispatches no model turn.
    pub fn owned_thread_factory(&self) -> Arc<dyn ChildFactory> {
        Arc::new(Factory {
            service: Arc::downgrade(&self.inner),
        })
    }

    /// Install the namespace's acknowledged graph and immutable Stop policy.
    /// Composition restores the coordinator before admitting conversation tools.
    pub fn bind_owned_threads(
        &self,
        coordinator: OwnershipCoordinator,
        cascade: bool,
    ) -> Result<(), ConversationError> {
        self.inner
            .ownership
            .set(Ownership {
                coordinator,
                cascade,
            })
            .map_err(|_| ConversationError::RequestConflict)
    }

    /// Create an empty ordinary child under a verified exact source turn.
    /// Approval selection stays with ordinary creation; no parent's permission
    /// answers or effective approval configuration are copied into the child.
    pub async fn create_owned_thread(
        &self,
        source: AgentToolSource,
        id: ConversationId,
        requested: RequestedConversation,
    ) -> Result<CreationReceipt, CreationFailure<ConversationError>> {
        let caller = source.caller();
        if requested.approval_mode.is_some() {
            return Err(CreationFailure::Target(ConversationError::InvalidInput));
        }
        let agent = match requested.agent {
            None => None,
            Some(RequestedAgent::Known(agent)) => Some(agent.name().to_owned()),
            Some(RequestedAgent::Unknown) => {
                return Err(CreationFailure::Target(ConversationError::AgentUnsupported))
            }
        };
        let preparation = Preparation {
            organization: caller.organization_id.as_str().into(),
            principal: caller.principal_id.as_str().into(),
            surface: caller.surface_id.clone(),
            action: caller.action_id.clone(),
            agent,
            model: requested.model,
            environment: requested.environment,
        };
        let service = self.clone();
        tokio::spawn(async move {
            service
                .prepare_owned_thread(source, id, caller, preparation)
                .await
        })
        .await
        .map_err(|error| CreationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }

    async fn prepare_owned_thread(
        &self,
        source: AgentToolSource,
        id: ConversationId,
        caller: ConversationCaller,
        preparation: Preparation,
    ) -> Result<CreationReceipt, CreationFailure<ConversationError>> {
        let _admission = self.inner.thread_admission.clone().lock_owned().await;
        let ownership = self
            .inner
            .ownership
            .get()
            .ok_or(CreationFailure::Target(ConversationError::Unavailable))?;
        let parent = self
            .resolve(source.conversation(), &caller)
            .await
            .map_err(CreationFailure::Target)?;
        if parent
            .agent
            .active_execution_id()
            .as_ref()
            .map(ExecutionId::as_str)
            != Some(source.execution())
        {
            return Err(CreationFailure::Target(ConversationError::Unavailable));
        }
        if self
            .captured_stop_actor(source.conversation(), source.execution(), &caller)
            .await
            .map_err(CreationFailure::Target)?
            .is_some()
        {
            return Err(CreationFailure::Target(ConversationError::Unavailable));
        }
        let session = conversation_session(source.conversation());
        let lifetime = ownership
            .coordinator
            .active_lifetime_for_session(&session)
            .ok_or(CreationFailure::Target(ConversationError::Unavailable))?;
        let description = serde_json::to_string(&preparation)
            .map_err(|_| CreationFailure::Target(ConversationError::InvalidInput))?;
        let request = creation_request(
            caller.principal_id.as_str(),
            &caller.surface_id,
            &caller.action_id,
        );
        // This is the ordinary creation policy, not inherited authority.
        let policy = creation_policy();
        let prepared = ownership
            .coordinator
            .prepare_child(
                SpawnCommand {
                    parent: lifetime,
                    request_id: request,
                    task: description,
                    policy: PolicyRead::Committed(policy),
                    child_supports_policy: true,
                    model: None,
                    origin: SpawnOrigin::Execution {
                        execution: ExecutionId::new(source.execution()).map_err(|_| {
                            CreationFailure::Target(ConversationError::InvalidInput)
                        })?,
                        tool: None,
                    },
                },
                conversation_session(&id),
            )
            .await
            .map_err(|_| CreationFailure::Target(ConversationError::Unavailable))?;
        if prepared.progress != SpawnProgress::Ready
            || ownership.coordinator.lifetime_state(&prepared.child) != Some(LifetimeState::Open)
        {
            return Err(CreationFailure::Target(ConversationError::Unavailable));
        }
        let storage = self
            .commands()
            .ok_or(CreationFailure::Target(ConversationError::Unavailable))?;
        let binding = ownership
            .coordinator
            .spawn_binding_for_lifetime(&prepared.child)
            .ok_or(CreationFailure::Target(ConversationError::Unavailable))?;
        self.lookup_child_creation(storage, id, caller, binding.task_digest)
            .await?
            .ok_or(CreationFailure::Target(ConversationError::Unavailable))
    }

    /// Rebind acknowledged cleanup adapters, then retry retained close owners.
    /// Resume may have completed an empty-adapter generation before this bind.
    /// Every retry joins the saved operation, cause and initiator.
    pub async fn recover_owned_threads(
        &self,
        snapshot: &OwnershipSnapshot,
    ) -> Result<usize, ConversationError> {
        let Some(ownership) = self.inner.ownership.get() else {
            return Ok(0);
        };
        for row in &snapshot.lifetimes {
            let id = ConversationId::new(row.session_id.as_str())
                .map_err(|_| ConversationError::InvalidInput)?;
            match self.confirm_child_creation(&id, &row.lifetime_id).await {
                Ok(true) => {}
                Ok(false) => {
                    if let Some(binding) = ownership
                        .coordinator
                        .spawn_binding_for_lifetime(&row.lifetime_id)
                    {
                        // A completed absence already belongs to the graph.
                        // Only an unsettled reservation needs command discovery.
                        if !matches!(
                            ownership.coordinator.spawn_progress(&binding.request_id),
                            Some(
                                SpawnProgress::Unconfirmed {
                                    known: KnownMilestone::Reserved
                                } | SpawnProgress::Draining {
                                    known: KnownMilestone::Reserved
                                }
                            )
                        ) {
                            continue;
                        }
                        if let Some(original_command) =
                            self.unstarted_child_creation(&id, &binding).await?
                        {
                            ownership
                                .coordinator
                                .reject_unstarted_preparation(&binding.request_id, &row.lifetime_id)
                                .await
                                .map_err(|_| ConversationError::Unavailable)?;
                            drop(original_command);
                        }
                    }
                    continue;
                }
                Err(ConversationError::Unavailable) => {
                    // Attempted does not prove acquisition. Keep the original
                    // lifetime unsettled without binding an inferred owner or
                    // preventing unrelated conversations from starting.
                    tracing::warn!(conversation_id = %id, "restored child creation remains unconfirmed; cleanup owner was not rebound");
                    continue;
                }
                Err(error) => return Err(error),
            }
            let _ = ownership.coordinator.bind_resources(
                row.lifetime_id.clone(),
                Arc::new(Resources {
                    service: Arc::downgrade(&self.inner),
                    id,
                    acquisition: None,
                }),
            );
        }
        let mut attempted = 0;
        for row in &snapshot.lifetimes {
            if row.state == LifetimeState::Closing && row.cascaded_from.is_none() {
                let (Some(cause), Some(initiator)) = (row.cause.clone(), row.initiator.clone())
                else {
                    return Err(ConversationError::Unavailable);
                };
                attempted += 1;
                if let Err(error) = ownership
                    .coordinator
                    .end_lifetime(CloseCommand {
                        lifetime: row.lifetime_id.clone(),
                        cause,
                        initiator,
                        external_attachment: false,
                        timeout: Some(Duration::from_secs(5)),
                    })
                    .await
                {
                    tracing::warn!(lifetime_id = %row.lifetime_id.as_str(), %error, "restored ownership cleanup remains unconfirmed");
                }
                continue;
            }
            let id = ConversationId::new(row.session_id.as_str())
                .map_err(|_| ConversationError::InvalidInput)?;
            let Some(record) = super::load_conversation(self.inner.metadata.as_ref(), &id).await?
            else {
                continue;
            };
            if let Some(deletion) = record.deletion().filter(|deletion| deletion.erased()) {
                attempted += 1;
                let actor = super::deletion_actor(deletion)?;
                if let Err(error) = self.end_deleted_ownership(&id, &actor).await {
                    tracing::warn!(conversation_id = %id, %error, "deleted ownership cleanup remains unconfirmed");
                }
            }
        }
        Ok(attempted)
    }

    pub(super) async fn end_deleted_ownership(
        &self,
        id: &ConversationId,
        actor: &ActionContext,
    ) -> Result<(), ConversationError> {
        let Some(ownership) = self.inner.ownership.get() else {
            return Ok(());
        };
        let Some(lifetime) = ownership
            .coordinator
            .active_lifetime_for_session(&conversation_session(id))
        else {
            return Ok(());
        };
        if !self.confirm_child_creation(id, &lifetime).await? {
            return Err(ConversationError::Unavailable);
        }
        let resources = Arc::new(Resources {
            service: Arc::downgrade(&self.inner),
            id: id.clone(),
            acquisition: None,
        });
        let _ = ownership
            .coordinator
            .bind_resources(lifetime.clone(), resources);
        ownership
            .coordinator
            .end_lifetime(CloseCommand {
                lifetime,
                cause: LifetimeCause::Deletion,
                initiator: Initiator::Host(
                    HostActor::new(actor.principal_id(), actor.surface_id(), actor.request_id())
                        .map_err(|_| ConversationError::InvalidInput)?,
                ),
                external_attachment: false,
                timeout: Some(Duration::from_secs(5)),
            })
            .await
            .map_err(|_| ConversationError::Unavailable)
    }

    async fn cold_owned_resources(
        &self,
        id: &ConversationId,
        acquisition: Option<&std::sync::OnceLock<bool>>,
    ) -> ResourceReport {
        let unknown = ResourceReport {
            physical: PhysicalFact::Failed,
            evidence: EvidenceFact::Pending,
        };
        let Ok(committed) = self.committed_if_present_with_order(id).await else {
            return unknown;
        };
        let Some((committed, _)) = committed else {
            // No persisted attachment history exists. Only this exact completed
            // opening can establish non-acquisition; a missing Slot cannot.
            return if acquisition.and_then(std::sync::OnceLock::get) == Some(&false) {
                ResourceReport {
                    physical: PhysicalFact::Released,
                    evidence: EvidenceFact::Acknowledged,
                }
            } else {
                unknown
            };
        };
        if matches!(
            committed.state(),
            CommittedViewState::Complete | CommittedViewState::CompleteEmpty
        ) && committed
            .snapshot()
            .is_none_or(|snapshot| snapshot.lease.is_none())
            && acquisition.and_then(std::sync::OnceLock::get) == Some(&false)
        {
            return ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            };
        }
        let Some(lease) = committed
            .snapshot()
            .and_then(|snapshot| snapshot.lease.as_ref())
            .and_then(|current| current.held())
        else {
            return unknown;
        };

        let known = match lease.phase() {
            LeasePhase::Ended { cleanup, .. } => Some(cleanup),
            LeasePhase::Interrupted {
                late_cleanup: Some(cleanup),
                ..
            } => Some(cleanup),
            _ => {
                let Ok(environment) = self.inner.environment.of(id).await else {
                    return unknown;
                };
                if environment.declaration().environment != lease.terms().environment {
                    return unknown;
                }
                environment.account(lease.id()).await
            }
        };
        if matches!(known, Some(LeaseCleanup::Confirmed { .. })) {
            // The existing environment/lease owner confirms this exact process
            // tree. The coordinator durably records the resource observation.
            ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            }
        } else {
            // NotHeld only says the environment currently holds nothing; it is
            // not proof that a process from an earlier connection was reaped.
            unknown
        }
    }

    pub(super) async fn ownership_gate(
        &self,
        id: &ConversationId,
        actor: &ActionContext,
        acquisition: Option<Arc<std::sync::OnceLock<bool>>>,
    ) -> Result<Option<Arc<dyn OwnedLifetime>>, ConversationError> {
        let Some(ownership) = self.inner.ownership.get() else {
            return Ok(None);
        };
        let session = conversation_session(id);
        let coordinator = &ownership.coordinator;
        // A pending gate belongs to the creation's original metadata, not to
        // another opener that happens to name the same reserved session.
        if let Some(lifetime) = coordinator
            .active_lifetime_for_session(&session)
            .or_else(|| coordinator.child_lifetime_for_session(&session))
        {
            if let Some(binding) = coordinator.spawn_binding_for_lifetime(&lifetime) {
                if self.child_creation_caller(id, &binding).await?.is_none() {
                    return Err(ConversationError::Unavailable);
                }
            }
        }
        if coordinator.active_lifetime_for_session(&session).is_none()
            && coordinator.child_lifetime_for_session(&session).is_some()
        {
            return Err(ConversationError::Unavailable);
        }
        let lifetime = match coordinator.active_lifetime_for_session(&session) {
            Some(id) => id,
            None => coordinator
                .open_root(
                    session,
                    Initiator::Host(
                        HostActor::new(
                            actor.principal_id(),
                            actor.surface_id(),
                            actor.request_id(),
                        )
                        .map_err(|_| ConversationError::InvalidInput)?,
                    ),
                )
                .await
                .map_err(|_| ConversationError::Unavailable)?,
        };
        // A restored lifetime's cleanup adapter must be rebound before attachment.
        // Reopening does not replay its child factory or initial submission.
        if coordinator.spawn_binding_for_lifetime(&lifetime).is_some() {
            // Factory success or composition recovery supplied this adapter
            // from the original child receipt. A cold submit may already hold
            // the principal command lease, so consume that prior transfer.
            if !coordinator.has_bound_resources(&lifetime) {
                return Err(ConversationError::Unavailable);
            }
            return coordinator
                .participation(&lifetime)
                .map(Some)
                .ok_or(ConversationError::Unavailable);
        }
        let resources = Arc::new(Resources {
            service: Arc::downgrade(&self.inner),
            id: id.clone(),
            acquisition,
        });
        match coordinator.bind_resources(lifetime.clone(), resources) {
            Ok(()) => {}
            Err(failure) if failure.reason == BindResourcesRefusal::AlreadyBound => {}
            Err(_) => return Err(ConversationError::Unavailable),
        }
        coordinator
            .participation(&lifetime)
            .map(Some)
            .ok_or(ConversationError::Unavailable)
    }

    /// Called only by the strict new-target creation owner after acknowledged
    /// Created, audit and capacity checks, while inserting its vacant live slot.
    /// Reads only the existing graph and pending transfer, without repository
    /// or audit I/O. The factory retains the actual acquisition through its
    /// existing PrepareFailure/PreparedChild resource transfer.
    pub(super) async fn claim_owned_preparation(
        &self,
        id: &ConversationId,
        actor: &ActionContext,
        opening: Arc<super::Slot>,
    ) -> Result<Option<Arc<dyn OwnedLifetime>>, ConversationError> {
        let Some(ownership) = self.inner.ownership.get() else {
            return Ok(None);
        };
        let session = conversation_session(id);
        let lifetime = ownership
            .coordinator
            .active_lifetime_for_session(&session)
            .ok_or(ConversationError::Unavailable)?;
        let binding = ownership
            .coordinator
            .spawn_binding_for_lifetime(&lifetime)
            .ok_or(ConversationError::Unavailable)?;
        if binding.request_id
            != creation_request(actor.principal_id(), actor.surface_id(), actor.request_id())
        {
            return Err(ConversationError::Unavailable);
        }
        let mut preparations = self.inner.ownership_preparations.lock().await;
        let preparation = preparations
            .get_mut(&session)
            .ok_or(ConversationError::Unavailable)?;
        if preparation.resources.is_some() {
            return Err(ConversationError::RequestConflict);
        }
        preparation.resources = Some(PreparationResources {
            resources: Arc::new(Resources {
                service: Arc::downgrade(&self.inner),
                id: id.clone(),
                acquisition: None,
            }),
            opening,
        });
        Ok(Some(preparation.gate.clone()))
    }

    async fn child_creation_caller(
        &self,
        id: &ConversationId,
        binding: &SpawnBinding,
    ) -> Result<Option<ConversationCaller>, ConversationError> {
        let Some(record) = super::load_conversation(self.inner.metadata.as_ref(), id).await? else {
            return Ok(None);
        };
        if creation_request(
            record.owner().as_str(),
            record.creator_surface(),
            record.creation_action(),
        ) != binding.request_id
        {
            return Ok(None);
        }
        let parent = ConversationId::new(binding.parent_session.as_str())
            .map_err(|_| ConversationError::InvalidInput)?;
        let Some(parent) = super::load_conversation(self.inner.metadata.as_ref(), &parent).await?
        else {
            return Ok(None);
        };
        if record.organization() != parent.organization() {
            return Ok(None);
        }
        Ok(Some(ConversationCaller {
            organization_id: record.organization().clone(),
            principal_id: record.owner().clone(),
            surface_id: record.creator_surface().into(),
            action_id: record.creation_action().into(),
        }))
    }

    /// Resource adapters are bound only from the original child command and
    /// immutable metadata origin. A cached slot alone belongs to no new attempt.
    async fn confirm_child_creation(
        &self,
        id: &ConversationId,
        lifetime: &AgentLifetimeId,
    ) -> Result<bool, ConversationError> {
        let Some(ownership) = self.inner.ownership.get() else {
            return Ok(false);
        };
        let Some(binding) = ownership.coordinator.spawn_binding_for_lifetime(lifetime) else {
            return Ok(true); // An existing root has no child creation command.
        };
        let Some(caller) = self.child_creation_caller(id, &binding).await? else {
            return Ok(false);
        };
        let storage = self.commands().ok_or(ConversationError::Unavailable)?;
        self.has_child_creation_receipt(storage, id, &caller, &binding.task_digest)
            .await
    }
}

struct Factory {
    service: Weak<Inner>,
}
#[async_trait]
impl ChildFactory for Factory {
    async fn prepare(&self, request: PrepareRequest) -> Result<PreparedChild, PrepareFailure> {
        let refused = || PrepareFailure {
            failure: PortFailure::Rejected,
            cleanup: None,
        };
        let inner = self.service.upgrade().ok_or_else(refused)?;
        let service = ConversationService { inner };
        let Some(ownership) = service.inner.ownership.get() else {
            return Err(refused());
        };
        if ownership
            .coordinator
            .lifetime_state(&request.child)
            .is_none()
            || request.policy != creation_policy()
            || request.model.is_some()
        {
            return Err(refused());
        }
        let preparation: Preparation =
            serde_json::from_str(&request.task).map_err(|_| refused())?;
        if preparation
            .agent
            .as_deref()
            .is_some_and(|name| AgentId::parse(name).is_none())
        {
            return Err(refused());
        }
        let caller = preparation.caller().map_err(|_| refused())?;
        let id = ConversationId::new(request.session.as_str()).map_err(|_| refused())?;
        let storage = service.commands().ok_or_else(refused)?;
        let binding = ownership
            .coordinator
            .spawn_binding_for_lifetime(&request.child)
            .filter(|binding| binding.request_id == request.request)
            .ok_or_else(refused)?;
        service.inner.ownership_preparations.lock().await.insert(
            request.session.clone(),
            PendingPreparation {
                gate: request.owned_lifetime,
                resources: None,
            },
        );
        let result = service
            .create_child_command(
                storage,
                id.clone(),
                caller,
                preparation.requested(),
                binding.task_digest,
            )
            .await;
        let resources = service
            .inner
            .ownership_preparations
            .lock()
            .await
            .remove(&request.session)
            .and_then(|preparation| preparation.resources);
        if result.is_err() {
            return Err(match resources {
                Some(held)
                    if held.opening.value.get().is_some_and(|result| {
                        result.as_ref().is_err_and(|failure| !failure.holds)
                    }) =>
                {
                    PrepareFailure {
                        failure: PortFailure::Rejected,
                        cleanup: Some(Arc::new(UnacquiredPreparation {
                            opening: held.opening,
                        })),
                    }
                }
                Some(held) => PrepareFailure {
                    failure: PortFailure::Uncertain,
                    cleanup: Some(held.resources),
                },
                None => refused(),
            });
        }
        let resources = resources.ok_or(PrepareFailure {
            failure: PortFailure::Uncertain,
            cleanup: None,
        })?;
        Ok(PreparedChild {
            resources: resources.resources,
            submit: Arc::new(NoInitialTurn),
        })
    }
}
struct NoInitialTurn;
#[async_trait]
impl InitialSubmit for NoInitialTurn {
    async fn submit(&self, _: &str, _: &SpawnRequestId) -> Result<TaskReceiptId, PortFailure> {
        Err(PortFailure::Rejected)
    }
}
struct Resources {
    service: Weak<Inner>,
    id: ConversationId,
    acquisition: Option<Arc<std::sync::OnceLock<bool>>>,
}
#[async_trait]
impl ChildResources for Resources {
    async fn close(&self, _: &LifetimeCause, initiator: &Initiator) -> ResourceReport {
        let failed = ResourceReport {
            physical: PhysicalFact::Failed,
            evidence: EvidenceFact::Pending,
        };
        let Some(inner) = self.service.upgrade() else {
            return failed;
        };
        let service = ConversationService { inner };
        // Successful deletion already confirmed physical cleanup and persisted
        // its erasure tombstone before asking ownership to release capacity.
        if super::load_conversation(service.inner.metadata.as_ref(), &self.id)
            .await
            .is_ok_and(|record| {
                record.is_some_and(|record| {
                    record.deletion().is_some_and(|deletion| deletion.erased())
                })
            })
        {
            return ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            };
        }
        let actor = match initiator {
            Initiator::Host(actor) => {
                ActionContext::new(actor.principal_id(), actor.surface_id(), actor.request_id())
            }
            Initiator::Runtime => {
                ActionContext::new("gateway", "owned_cleanup", self.id.to_string())
            }
        };
        let Ok(actor) = actor else {
            return failed;
        };
        let slot = service
            .inner
            .conversations
            .lock()
            .await
            .get(&self.id)
            .cloned();
        let Some(slot) = slot else {
            return service
                .cold_owned_resources(&self.id, self.acquisition.as_deref())
                .await;
        };
        // A pending or failed opening may retain a newer physical hold before
        // its lease is published. Historical cleanup cannot release that owner.
        let Some(live) = slot.value.get().and_then(|live| live.as_ref().ok()) else {
            return failed;
        };
        let close = live
            .lease
            .close(
                &live.agent,
                LeaseEndCause::Closed,
                actor,
                service.inner.deletion_budgets.stop,
                AgentEnd::Close,
            )
            .await;
        match close {
            Ok(_) => ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Acknowledged,
            },
            Err(error) if confirmed_despite(&error).is_some() => ResourceReport {
                physical: PhysicalFact::Released,
                evidence: EvidenceFact::Failed,
            },
            Err(_) => failed,
        }
    }
}

impl ConversationService {
    /// Source-directed admission and submission share Stop's capture boundary.
    /// The supervisor retains admission through command/queue acknowledgement
    /// even when the tool's response waiter disappears.
    pub async fn submit_agent_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        target: ConversationId,
        source: AgentToolSource,
        execution: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
    ) -> Result<SubmittedCommand, MutationFailure<ConversationError>> {
        let service = self.clone();
        tokio::spawn(async move {
            let _admission = service.inner.thread_admission.clone().lock_owned().await;
            let caller = source.caller();
            let live = service
                .resolve(source.conversation(), &caller)
                .await
                .map_err(MutationFailure::Target)?;
            if live
                .agent
                .active_execution_id()
                .as_ref()
                .map(ExecutionId::as_str)
                != Some(source.execution())
                || service
                    .captured_stop_actor(source.conversation(), source.execution(), &caller)
                    .await
                    .map_err(MutationFailure::Target)?
                    .is_some()
            {
                return Err(MutationFailure::Target(ConversationError::Unavailable));
            }
            service
                .submit_command_origin(
                    storage,
                    target,
                    caller,
                    Initiation::owner(),
                    false,
                    execution,
                    message,
                    mode,
                    true,
                )
                .await
        })
        .await
        .map_err(|error| MutationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }
}

fn creation_policy() -> ApprovalPolicy {
    ApprovalPolicy::new("ask", "ordinary-creation", "gateway-creation")
        .expect("static ordinary creation policy")
}

pub(super) fn creation_request(principal: &str, surface: &str, action: &str) -> SpawnRequestId {
    let bytes = serde_json::to_vec(&(principal, surface, action)).expect("strings serialize");
    SpawnRequestId::new(format!("create-{:x}", Sha256::digest(bytes))).expect("digest identity")
}

#[cfg(test)]
#[path = "../../../../tests/conversation/owned_preparation.rs"]
mod tests;
