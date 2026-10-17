//! Gateway ownership composition; ordinary turn Stop never ends these lifetimes.
//!
//! Ordering: thread admission -> verify exact source -> coordinator reservation
//! -> prepare ordinary conversation -> install existing gate -> Ready. Stop uses
//! the same admission boundary when capturing its exact execution selection.
use super::super::environment::{confirmed_despite, AgentEnd};
use super::super::AgentToolSource;
use super::{
    conversation_session, ConversationCaller, ConversationError, ConversationId,
    ConversationService, Inner, RequestedAgent, RequestedConversation, SubmissionMode,
    SubmittedMessage,
};
use async_trait::async_trait;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::agents::AgentId;
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::application::agent_execution::{
    agents::OwnedLifetime,
    commands::{CreationFailure, CreationReceipt},
    subagents::{
        ChildFactory, ChildResources, InitialSubmit, OwnershipCoordinator, PortFailure,
        PrepareFailure, PrepareRequest, PreparedChild, ResourceReport, SpawnCommand,
    },
};
use nessa_sdk::domain::agent_execution::subagents::{
    ApprovalPolicy, EvidenceFact, HostActor, Initiator, LifetimeCause, PhysicalFact, PolicyRead,
    SpawnOrigin, SpawnRequestId, TaskReceiptId,
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, Initiation},
    leases::LeaseEndCause,
};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::sync::{Arc, Weak};

pub(super) struct Ownership {
    pub coordinator: OwnershipCoordinator,
    pub cascade: bool,
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
            service.prepare_owned_thread(source, id, caller, preparation).await
        })
        .await
        .map_err(|error| CreationFailure::TaskFault(
            nessa_sdk::application::agent_execution::commands::CreationTaskFault::from_join_error(error)
        ))?
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
        let request = format!(
            "create-{:x}",
            Sha256::digest(
                serde_json::to_vec(&(
                    caller.principal_id.as_str(),
                    &caller.surface_id,
                    &caller.action_id
                ))
                .expect("strings serialize")
            )
        );
        // This is the ordinary creation policy, not inherited authority.
        let policy = creation_policy();
        ownership
            .coordinator
            .prepare_child(
                SpawnCommand {
                    parent: lifetime,
                    request_id: SpawnRequestId::new(request).expect("digest identity"),
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
        let storage = self
            .commands()
            .ok_or(CreationFailure::Target(ConversationError::Unavailable))?;
        self.lookup_creation(storage, id, caller, preparation.requested())
            .await?
            .ok_or(CreationFailure::Target(ConversationError::Unavailable))
    }

    pub(super) async fn ownership_gate(
        &self,
        id: &ConversationId,
        actor: &ActionContext,
    ) -> Result<Option<Arc<dyn OwnedLifetime>>, ConversationError> {
        let Some(ownership) = self.inner.ownership.get() else {
            return Ok(None);
        };
        let session = conversation_session(id);
        if let Some(gate) = self
            .inner
            .ownership_preparations
            .lock()
            .await
            .get(&session)
            .cloned()
        {
            return Ok(Some(gate));
        }
        let coordinator = &ownership.coordinator;
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
        let resources = Arc::new(Resources {
            service: Arc::downgrade(&self.inner),
            id: id.clone(),
        });
        match coordinator.bind_resources(lifetime.clone(), resources) {
            Ok(()) => {},
            Err(failure) if failure.reason == nessa_sdk::application::agent_execution::subagents::BindResourcesRefusal::AlreadyBound => {},
            Err(_) => return Err(ConversationError::Unavailable),
        }
        coordinator
            .participation(&lifetime)
            .map(Some)
            .ok_or(ConversationError::Unavailable)
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
        service
            .inner
            .ownership_preparations
            .lock()
            .await
            .insert(request.session.clone(), request.owned_lifetime);
        let result = service
            .create_command(storage, id.clone(), caller, preparation.requested())
            .await;
        service
            .inner
            .ownership_preparations
            .lock()
            .await
            .remove(&request.session);
        let resources: Arc<dyn ChildResources> = Arc::new(Resources {
            service: self.service.clone(),
            id: id.clone(),
        });
        if result.is_err() {
            let no_owner = service.inner.conversations.lock().await.get(&id).is_none();
            if no_owner
                && !matches!(
                    result,
                    Err(CreationFailure::TaskFault(_) | CreationFailure::Interrupted(_))
                )
            {
                return Err(PrepareFailure {
                    failure: PortFailure::Rejected,
                    cleanup: None,
                });
            }
            return Err(PrepareFailure {
                failure: PortFailure::Uncertain,
                cleanup: Some(resources),
            });
        }
        Ok(PreparedChild {
            resources,
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
        let Some(live) = slot
            .as_ref()
            .and_then(|slot| slot.value.get())
            .and_then(|live| live.as_ref().ok())
        else {
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
        storage: Arc<dyn nessa_sdk::application::agent_execution::commands::CreationStorage>,
        target: ConversationId,
        source: AgentToolSource,
        execution: String,
        message: SubmittedMessage,
        mode: SubmissionMode,
    ) -> Result<
        super::mutation::SubmittedCommand,
        nessa_sdk::application::agent_execution::commands::MutationFailure<ConversationError>,
    > {
        use nessa_sdk::application::agent_execution::commands::{
            CreationTaskFault, MutationFailure,
        };
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
