//! A native acknowledgement and a correlated metadata commit select one model.
use super::{
    agent_question_entries, agent_review_entries, contain_mode_terminal, lifecycle_view,
    load_conversation, provider_approval_mode, supervised, ActionContext, AgentError,
    AttachmentPhase, Conversation, ConversationCaller, ConversationError, ConversationId,
    ConversationModelId, ConversationService, LiveConversation, ModelBudget, SessionSnapshot,
};
use crate::conversation::application::{
    ConversationModelApplication, ConversationModelRequest, ConversationModelRequestState,
    ConversationModelSelection, ScheduleActor, SelectionChange,
};
use nessa_protocol::conversation::domain::ConversationApprovalMode;
use nessa_protocol::conversation::projection::{bound_view, CommittedCursor};
use nessa_protocol::conversation::view::{
    ConversationModelChangeEffect, ConversationModelChangeStatus, ConversationModelChangeView,
    ConversationSelectionView, ConversationView,
};
use nessa_sdk::application::agent_execution::providers::{
    ApprovalMode, ModelChangeTiming, ProviderOperationStage,
};
use std::sync::Arc;
use uuid::Uuid;

impl ConversationService {
    /// Select a same-provider model on the exact existing native session.
    /// The native harness determines when an active turn sees the choice. The
    /// admitted task finishes after caller loss; retries read its original
    /// request rather than sending a second native control.
    pub async fn set_model(
        &self,
        id: ConversationId,
        caller: ConversationCaller,
        model: ConversationModelId,
    ) -> Result<ConversationModelSelection, ConversationError> {
        let service = self.clone();
        let changed = (self.inner.live_changes.clone(), id.clone());
        supervised(async move {
            let result = service.change_model(&id, &caller, model).await;
            changed.0.publish(&changed.1);
            result
        })
        .await
    }

    pub(crate) async fn change_model(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
        model: ConversationModelId,
    ) -> Result<ConversationModelSelection, ConversationError> {
        let _admission = self.admit().await?;
        let _selection = self.inner.mode_changes.lock(id).await;
        let actor = caller.actor()?;
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        if let Some(saved) = self
            .inner
            .metadata
            .model_change(id, &caller.action_id)
            .await?
        {
            if saved.conversation_id != *id
                || saved.organization_id != caller.organization_id
                || saved.initiator_principal_id != caller.principal_id
                || saved.initiator_surface_id != caller.surface_id
                || saved.request_id != caller.action_id
                || saved.requested_model != model
            {
                return Err(ConversationError::RequestConflict);
            }
            if saved.state == ConversationModelRequestState::Pending {
                self.recover_model_change(id, caller).await?;
                let terminal = self
                    .inner
                    .metadata
                    .model_change(id, &saved.request_id)
                    .await?
                    .ok_or(ConversationError::ModelChangeUncertain)?;
                if terminal.conversation_id != saved.conversation_id
                    || terminal.organization_id != saved.organization_id
                    || terminal.request_id != saved.request_id
                    || terminal.initiator_principal_id != saved.initiator_principal_id
                    || terminal.initiator_surface_id != saved.initiator_surface_id
                    || terminal.prior_identity != saved.prior_identity
                    || terminal.requested_identity != saved.requested_identity
                    || terminal.prior_model != saved.prior_model
                    || terminal.prior_approval_mode != saved.prior_approval_mode
                    || terminal.requested_model != saved.requested_model
                    || terminal.requested_at_ms != saved.requested_at_ms
                {
                    return Err(ConversationError::ModelChangeUncertain);
                }
                return self
                    .complete_applied_model(id, saved_selection(&terminal))
                    .await;
            }
            return self
                .complete_applied_model(id, saved_selection(&saved))
                .await;
        }
        self.recover_model_change(id, caller).await?;
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        self.refuse_pending_model_change(id, caller).await?;
        if self.inner.metadata.pending_mode_change(id).await?.is_some() {
            return Err(ConversationError::ApprovalModeUncertain);
        }
        if record.model() == &model {
            return Err(ConversationError::ModelChangeNotApplied);
        }
        let configured = self
            .inner
            .agents
            .resolve_model(
                record.agent().ok_or(ConversationError::AgentUnsupported)?,
                model.as_str(),
            )
            .await?;
        let live = self.resolve(id, caller).await?;
        let _ = live.join_attachment_owner().await;
        if live.agent.attachment_status().phase() != AttachmentPhase::Attached
            || live.agent.provider_identity().model_id() != record.model().as_str()
            || live.agent.approval_mode() != Some(provider_approval_mode(record.approval_mode()))
        {
            return Err(ConversationError::ModelChangeUncertain);
        }
        let target = live
            .lease
            .model_provider(configured.provider.clone())
            .map_err(ConversationError::Agent)?;
        if target.identity().model_id() != model.as_str() {
            return Err(ConversationError::ModelChangeNotApplied);
        }
        let intent = ConversationModelRequest {
            conversation_id: id.clone(),
            organization_id: caller.organization_id.clone(),
            request_id: caller.action_id.clone(),
            initiator_principal_id: caller.principal_id.clone(),
            initiator_surface_id: caller.surface_id.clone(),
            prior_identity: live.agent.provider_identity(),
            requested_identity: target.identity(),
            prior_model: record.model().clone(),
            prior_approval_mode: record.approval_mode(),
            requested_model: model,
            state: ConversationModelRequestState::Pending,
            application: None,
            requested_at_ms: self.inner.clock.unix_milliseconds(),
        };
        let begun = match self.inner.metadata.begin_model_change(intent.clone()).await {
            Ok(request) => request,
            Err(error) => {
                return match self
                    .inner
                    .metadata
                    .model_change(id, &intent.request_id)
                    .await
                {
                    Ok(None) => Err(error),
                    Ok(Some(_)) | Err(_) => Err(ConversationError::ModelChangeUncertain),
                };
            }
        };
        if begun != intent {
            return Err(ConversationError::ModelChangeUncertain);
        }
        // Readers may show Changing while this native control is pending. No
        // requested model is published as the committed selection here.
        self.inner.live_changes.publish(id);
        let application = match live.agent.set_model(target, actor).await {
            Ok(effect) => {
                let mode = match effect.approval_mode() {
                    Some(ApprovalMode::Ask) => Some(ConversationApprovalMode::Ask),
                    Some(ApprovalMode::Auto) => Some(ConversationApprovalMode::Auto),
                    Some(ApprovalMode::Full) => Some(ConversationApprovalMode::Full),
                    None => None,
                };
                if effect.identity() != &intent.requested_identity
                    || effect.capabilities() != &live.agent.capabilities()
                    || effect.identity() != &live.agent.provider_identity()
                {
                    ConversationModelApplication::Uncertain
                } else if let Some(approval_mode) = mode {
                    ConversationModelApplication::Applied(ConversationModelSelection {
                        model: intent.requested_model.clone(),
                        approval_mode,
                        effect: match effect.timing() {
                            ModelChangeTiming::SubsequentResponses => {
                                ConversationModelChangeEffect::SubsequentResponses
                            }
                            ModelChangeTiming::NextTurn => ConversationModelChangeEffect::NextTurn,
                        },
                    })
                } else {
                    ConversationModelApplication::Uncertain
                }
            }
            // Addressable is not a no-effect proof for a failed setting call.
            Err(failure) => match failure.stage() {
                ProviderOperationStage::BeforeEffect => ConversationModelApplication::Refused,
                ProviderOperationStage::EffectUncertain => ConversationModelApplication::Uncertain,
            },
        };
        let observed = ConversationModelRequest {
            application: Some(application.clone()),
            ..intent
        };
        let observation = contain_mode_terminal(async {
            self.inner
                .metadata
                .observe_model_application(id, &observed.request_id, application.clone())
                .await
        })
        .await;
        if !matches!(observation, Ok(Ok(ref request)) if request == &observed) {
            return Err(ConversationError::ModelChangeUncertain);
        }
        let state = match &application {
            ConversationModelApplication::Applied(_) => ConversationModelRequestState::Applied,
            ConversationModelApplication::Refused => ConversationModelRequestState::NotApplied,
            ConversationModelApplication::Uncertain => {
                let _ = self.recover_model_change(id, caller).await;
                return Err(ConversationError::ModelChangeUncertain);
            }
        };
        if state == ConversationModelRequestState::Applied {
            self.invalidate_selection_usage_limit(
                SelectionChange::Model,
                id,
                &observed.request_id,
                ScheduleActor::model_request(&observed),
            )
            .await?;
        }
        let expected = ConversationModelRequest { state, ..observed };
        let write = contain_mode_terminal(async {
            self.inner
                .metadata
                .finish_model_change(id, &expected.request_id, state)
                .await
        })
        .await;
        let terminal = match write {
            Ok(Ok(request)) => Some(request),
            Ok(Err(_)) | Err(_) => contain_mode_terminal(async {
                self.inner
                    .metadata
                    .model_change(id, &expected.request_id)
                    .await
            })
            .await
            .ok()
            .and_then(Result::ok)
            .flatten(),
        };
        if terminal.as_ref() != Some(&expected) {
            let pending = ConversationModelRequest {
                state: ConversationModelRequestState::Pending,
                ..expected.clone()
            };
            if terminal.as_ref() != Some(&pending) {
                let _ = self.retire_uncertain_model(id).await;
            }
            return Err(ConversationError::ModelChangeUncertain);
        }
        let ConversationModelApplication::Applied(selection) = application else {
            return Err(ConversationError::ModelChangeNotApplied);
        };
        // Submission holds this same selection lock. Publish the matching
        // admission reservation before allowing the next message through.
        *live.model_budget.lock().await = ModelBudget {
            identity: live.agent.provider_identity(),
            reserved_output_tokens: configured.reserved_output_tokens,
        };
        live.projection.lock().await.model_selection(
            selection.model.as_str().into(),
            selection.approval_mode,
            selection.effect,
        );
        // Metadata already holds this model. The selection is not complete
        // until the open continuation is reconciled: changing away and back
        // before the due time would otherwise hide the mismatch until
        // delivery, when the model matches again.
        self.complete_applied_model(id, Ok(selection)).await
    }

    /// An applied selection, including one read back on retry, is returned
    /// only after schedule reconciliation commits. A refusal is not reconciled.
    async fn complete_applied_model(
        &self,
        id: &ConversationId,
        selection: Result<ConversationModelSelection, ConversationError>,
    ) -> Result<ConversationModelSelection, ConversationError> {
        let selection = selection?;
        self.commit_applied_model_plan(id).await?;
        Ok(selection)
    }

    /// The continuation plan for an applied model. A failure stays owed, and
    /// the next recovery — what an open read and a submit both call — runs it
    /// again even when the request is no longer pending.
    async fn commit_applied_model_plan(
        &self,
        id: &ConversationId,
    ) -> Result<(), ConversationError> {
        self.note_model_reconcile(id);
        self.reconcile_usage_limit_schedule(id).await?;
        self.clear_model_reconcile(id);
        Ok(())
    }

    /// Run a plan a previous applied transition left unfinished. Nothing is
    /// owed on the ordinary path, and that path does not touch the schedule.
    async fn retry_applied_model_plan(&self, id: &ConversationId) -> Result<(), ConversationError> {
        if !self.model_reconcile_owed(id) {
            return Ok(());
        }
        self.commit_applied_model_plan(id).await
    }

    fn note_model_reconcile(&self, id: &ConversationId) {
        self.inner
            .model_reconcile_owed
            .lock()
            .expect("model reconciliation")
            .insert(id.clone());
    }

    fn clear_model_reconcile(&self, id: &ConversationId) {
        self.inner
            .model_reconcile_owed
            .lock()
            .expect("model reconciliation")
            .remove(id);
    }

    fn model_reconcile_owed(&self, id: &ConversationId) -> bool {
        self.inner
            .model_reconcile_owed
            .lock()
            .expect("model reconciliation")
            .contains(id)
    }

    async fn retire_uncertain_model(&self, id: &ConversationId) -> Result<(), ConversationError> {
        let actor = ActionContext::new(
            "gateway",
            "model_recovery",
            format!("recovery-{}", Uuid::new_v4()),
        )
        .map_err(|_| ConversationError::ModelChangeUncertain)?;
        self.retire_selection_session(id, &actor)
            .await
            .map_err(|_| ConversationError::ModelChangeUncertain)
    }

    /// Settle a stopped request from its attributed SDK fact. No setting call
    /// is replayed. An unrecorded native result requires retiring that generation
    /// and verifying the committed prior configuration on a fresh attachment.
    pub(super) async fn recover_model_change(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(), ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        let Some(request) = self.inner.metadata.pending_model_change(id).await? else {
            return self.retry_applied_model_plan(id).await;
        };
        pending_model_for(id, &record, &request)?;
        self.finish_stopped_selection(id)
            .await
            .map_err(|_| ConversationError::ModelChangeUncertain)?;
        let agent = record.agent().ok_or(ConversationError::AgentUnsupported)?;
        let prior = self
            .inner
            .agents
            .resolve_for(agent, record.model().as_str(), record.approval_mode())
            .await?;
        let (committed, _) = self.committed_with_order(id).await?;
        let matching = committed
            .snapshot()
            .map(|snapshot| acknowledged_model_selection(&request, snapshot))
            .transpose()?
            .flatten();
        let (application, target, identity) = if let Some(selection) = matching {
            let target = self
                .inner
                .agents
                .resolve_for(
                    agent,
                    request.requested_model.as_str(),
                    selection.approval_mode,
                )
                .await?;
            (
                ConversationModelApplication::Applied(selection),
                target,
                request.requested_identity.clone(),
            )
        } else {
            if committed
                .snapshot()
                .is_some_and(|snapshot| snapshot.provider != request.prior_identity)
            {
                return Err(ConversationError::ModelChangeUncertain);
            }
            self.retire_uncertain_model(id).await?;
            let restored = self.resolve_unchecked(id, caller).await?;
            let _ = restored.join_attachment_owner().await;
            if restored.agent.attachment_status().phase() != AttachmentPhase::Attached
                || restored.agent.provider_identity() != request.prior_identity
                || restored.agent.approval_mode()
                    != Some(provider_approval_mode(request.prior_approval_mode))
            {
                return Err(ConversationError::ModelChangeUncertain);
            }
            (
                ConversationModelApplication::Refused,
                prior,
                request.prior_identity.clone(),
            )
        };
        if let Some(live) = self.live_now(id, caller).await? {
            if live.agent.provider_identity() != identity
                || live.agent.attachment_status().phase() != AttachmentPhase::Attached
            {
                self.retire_uncertain_model(id).await?;
            }
        }
        let observed = ConversationModelRequest {
            application: Some(application.clone()),
            ..request
        };
        let saved = self
            .inner
            .metadata
            .observe_model_application(id, &observed.request_id, application.clone())
            .await?;
        if saved != observed {
            return Err(ConversationError::ModelChangeUncertain);
        }
        let state = match &application {
            ConversationModelApplication::Applied(_) => ConversationModelRequestState::Applied,
            ConversationModelApplication::Refused => ConversationModelRequestState::NotApplied,
            ConversationModelApplication::Uncertain => {
                return Err(ConversationError::ModelChangeUncertain)
            }
        };
        if state == ConversationModelRequestState::Applied {
            self.invalidate_selection_usage_limit(
                SelectionChange::Model,
                id,
                &observed.request_id,
                ScheduleActor::model_request(&observed),
            )
            .await?;
        }
        let expected = ConversationModelRequest { state, ..observed };
        let written = contain_mode_terminal(async {
            self.inner
                .metadata
                .finish_model_change(id, &expected.request_id, state)
                .await
        })
        .await;
        let terminal = match written {
            Ok(Ok(request)) => Some(request),
            _ => contain_mode_terminal(async {
                self.inner
                    .metadata
                    .model_change(id, &expected.request_id)
                    .await
            })
            .await
            .ok()
            .and_then(Result::ok)
            .flatten(),
        };
        if terminal.as_ref() != Some(&expected) {
            let pending = ConversationModelRequest {
                state: ConversationModelRequestState::Pending,
                ..expected.clone()
            };
            if terminal.as_ref() != Some(&pending) {
                let _ = self.retire_uncertain_model(id).await;
            }
            return Err(ConversationError::ModelChangeUncertain);
        }
        // Metadata already holds an applied model. The recovery is not
        // finished until the open continuation is reconciled, same as a
        // command that returns the selection. A failed plan stays owed.
        let applied = match &application {
            ConversationModelApplication::Applied(selection) => Some(selection.clone()),
            ConversationModelApplication::Refused | ConversationModelApplication::Uncertain => None,
        };
        if applied.is_some() {
            self.note_model_reconcile(id);
        }
        if let Some(live) = self.live_now(id, caller).await? {
            if live.agent.provider_identity() != identity {
                return Err(ConversationError::ModelChangeUncertain);
            }
            *live.model_budget.lock().await = ModelBudget {
                identity,
                reserved_output_tokens: target.reserved_output_tokens,
            };
            if let Some(selection) = applied.clone() {
                live.projection.lock().await.model_selection(
                    selection.model.as_str().into(),
                    selection.approval_mode,
                    selection.effect,
                );
            }
        }
        if applied.is_some() {
            self.commit_applied_model_plan(id).await?;
        }
        self.inner.live_changes.publish(id);
        Ok(())
    }

    /// Restore presentation only from the same native/request relationship
    /// recovery uses, plus a matching terminal metadata commit. The preset in
    /// that historical application may precede an independent preset choice.
    pub(super) async fn confirmed_model_effect(
        &self,
        record: &Conversation,
        snapshot: Option<&SessionSnapshot>,
    ) -> Result<Option<ConversationModelChangeEffect>, ConversationError> {
        let Some(snapshot) = snapshot else {
            return Ok(None);
        };
        let Some(saved) = snapshot.model_change.as_ref() else {
            return Ok(None);
        };
        let Some(request) = self
            .inner
            .metadata
            .model_change(record.id(), saved.actor().request_id())
            .await?
        else {
            return Ok(None);
        };
        let Ok(committed) = saved_selection(&request) else {
            return Ok(None);
        };
        if request.conversation_id != *record.id()
            || request.organization_id != *record.organization()
            || committed.model != *record.model()
        {
            return Ok(None);
        }
        let Ok(Some(acknowledged)) = acknowledged_model_selection(&request, snapshot) else {
            return Ok(None);
        };
        Ok((acknowledged == committed).then_some(committed.effect))
    }

    pub(super) async fn read_pending_model_change(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<Option<(ConversationView, Option<CommittedCursor>)>, ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        let Some(request) = self.inner.metadata.pending_model_change(id).await? else {
            return Ok(None);
        };
        pending_model_for(id, &record, &request)?;
        let change = ConversationModelChangeView {
            request_id: request.request_id,
            requested_model: request.requested_model.as_str().into(),
            status: if request.application.is_none() {
                ConversationModelChangeStatus::Changing
            } else {
                ConversationModelChangeStatus::RecoveryRequired
            },
        };
        let slot = self.readable_slot(id).await?;
        let live = slot
            .as_ref()
            .and_then(|slot| slot.value.get())
            .and_then(|value| value.as_ref().ok())
            .cloned();
        let (committed, order) = self.committed_with_order(id).await?;
        let (view, cursor) = if let Some(live) = &live {
            let active = live.agent.active_execution_id();
            let mut projection = live.projection.lock().await;
            if projection.replace_committed(&committed, &order, active.as_ref()) {
                projection.transcript_state(committed.state().into());
            }
            projection.lifecycle(lifecycle_view(&live.agent));
            (
                projection.read_with_model_change(Some(change)),
                projection.committed_cursor(),
            )
        } else {
            let projection = self.detached(id, &committed, &order);
            (
                projection.read_with_model_change(Some(change)),
                projection.committed_cursor(),
            )
        };
        let read = self
            .pending_selection_read(
                (view, cursor),
                &record,
                id,
                caller,
                live.as_ref(),
                committed.snapshot(),
            )
            .await?;
        Ok(Some(read))
    }

    /// A pending selection disables new work, while reviews keep the exact
    /// live invocation that still owns them. A detached record answers none.
    pub(super) async fn pending_selection_read(
        &self,
        (mut view, cursor): (ConversationView, Option<CommittedCursor>),
        record: &Conversation,
        id: &ConversationId,
        caller: &ConversationCaller,
        live: Option<&Arc<LiveConversation>>,
        snapshot: Option<&SessionSnapshot>,
    ) -> Result<(ConversationView, Option<CommittedCursor>), ConversationError> {
        view.selection = Some(ConversationSelectionView {
            agent: record.agent().ok_or(ConversationError::AgentUnsupported)?,
            model: record.model().as_str().into(),
            approval_mode: record.approval_mode(),
        });
        if view.approval_mode_change.is_some() || view.model_change.is_some() {
            view.capabilities.queue = false;
            view.capabilities.steer = false;
            view.capabilities.resume = false;
        }
        if live.is_none() {
            view.capabilities.permissions = false;
            view.permissions.clear();
            view.questions.clear();
        }
        view.title = self.title(id).await;
        self.check_view_access(id, caller).await?;
        if let Some(live) = live {
            let current = self.readable_slot(id).await?;
            if !current
                .as_ref()
                .and_then(|slot| slot.value.get())
                .and_then(|value| value.as_ref().ok())
                .is_some_and(|known| Arc::ptr_eq(known, live))
            {
                return Err(ConversationError::Agent(AgentError::Closed));
            }
        }

        let view = if let Some(live) = live {
            super::app_calls::with_reviews(
                view,
                agent_review_entries(&live.agent, snapshot),
                live.app_reviews.entries(),
                agent_question_entries(&live.agent, snapshot),
            )
        } else {
            bound_view(view)
        };
        Ok((view, cursor))
    }

    pub(super) async fn refuse_pending_model_change(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<(), ConversationError> {
        let record = load_conversation(self.inner.metadata.as_ref(), id)
            .await?
            .ok_or(ConversationError::NotFound)?;
        self.access().admit(&record, caller)?;
        if self
            .inner
            .metadata
            .pending_model_change(id)
            .await?
            .is_some()
        {
            Err(ConversationError::ModelChangeUncertain)
        } else {
            Ok(())
        }
    }
}

/// The SDK owns the validated native fact; the metadata request owns its
/// original actor and intent. Recovery and publication join them here.
fn acknowledged_model_selection(
    request: &ConversationModelRequest,
    snapshot: &SessionSnapshot,
) -> Result<Option<ConversationModelSelection>, ConversationError> {
    let Some(saved) = snapshot.model_change.as_ref().filter(|saved| {
        saved.actor().principal_id() == request.initiator_principal_id.as_str()
            && saved.actor().surface_id() == request.initiator_surface_id
            && saved.actor().request_id() == request.request_id
            && saved.change().before() == &request.prior_identity
            && saved.change().after() == &request.requested_identity
    }) else {
        return Ok(None);
    };
    if &snapshot.provider != saved.change().after() {
        return Err(ConversationError::ModelChangeUncertain);
    }
    let approval_mode = match saved.configuration().approval_mode() {
        Some(ApprovalMode::Ask) => ConversationApprovalMode::Ask,
        Some(ApprovalMode::Auto) => ConversationApprovalMode::Auto,
        Some(ApprovalMode::Full) => ConversationApprovalMode::Full,
        None => return Err(ConversationError::ModelChangeUncertain),
    };
    let effect = match saved.timing() {
        Some(ModelChangeTiming::SubsequentResponses) => {
            ConversationModelChangeEffect::SubsequentResponses
        }
        Some(ModelChangeTiming::NextTurn) => ConversationModelChangeEffect::NextTurn,
        None => return Err(ConversationError::ModelChangeUncertain),
    };
    Ok(Some(ConversationModelSelection {
        model: request.requested_model.clone(),
        approval_mode,
        effect,
    }))
}

fn saved_selection(
    request: &ConversationModelRequest,
) -> Result<ConversationModelSelection, ConversationError> {
    match (&request.state, &request.application) {
        (
            ConversationModelRequestState::Applied,
            Some(ConversationModelApplication::Applied(selection)),
        ) => Ok(selection.clone()),
        (
            ConversationModelRequestState::NotApplied,
            Some(ConversationModelApplication::Refused),
        ) => Err(ConversationError::ModelChangeNotApplied),
        _ => Err(ConversationError::ModelChangeUncertain),
    }
}

/// Correlate adapter-returned pending intent before it authorizes retirement
/// or is joined into a publication. Ownership is the loaded record's.
fn pending_model_for(
    id: &ConversationId,
    record: &Conversation,
    request: &ConversationModelRequest,
) -> Result<(), ConversationError> {
    if request.conversation_id != *id
        || request.organization_id != *record.organization()
        || request.initiator_principal_id != *record.owner()
        || request.prior_model != *record.model()
        || request.prior_approval_mode != record.approval_mode()
        || request.prior_identity.model_id() != request.prior_model.as_str()
        || request.requested_identity.model_id() != request.requested_model.as_str()
        || request.prior_identity.name() != request.requested_identity.name()
        || request.state != ConversationModelRequestState::Pending
        || request.prior_model == request.requested_model
    {
        return Err(ConversationError::ModelChangeUncertain);
    }
    ActionContext::new(
        request.initiator_principal_id.as_str(),
        request.initiator_surface_id.as_str(),
        request.request_id.as_str(),
    )
    .map_err(|_| ConversationError::ModelChangeUncertain)?;
    Ok(())
}
