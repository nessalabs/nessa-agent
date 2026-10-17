//! Exact recursive turn Stop policy, using only the existing command ledger.
//!
//! | Order | Durable fact / effect |
//! |---|---|
//! | admission held | check root and capture graph descendants / exact queued + active turns |
//! | admission held | Accepted child Stops, then root Accepted binding their ordered digest |
//! | release admission | supervised exact Stops, preserving each target's existing authorization |
//! | retry/restart | verify the root digest and reuse the original captured bindings |
//!
//! A partial selection with no root marker never authorizes effects. Attempted
//! mutations keep the SDK's uncertainty contract; a lost ACK is not cancelled
//! again. No lifetime is sealed and no second graph or progress ledger is added.
use super::mutation::{ensure_live, stop_binding, StopTarget};
use super::{
    conversation_session, ConversationCaller, ConversationError, ConversationId,
    ConversationService,
};
use nessa_sdk::application::agent_execution::{
    agents::{turn_continuation, TurnContinuation},
    commands::{
        CreationStorage, CreationTaskFault, MutationBinding, MutationCoordinator, MutationFailure,
        MutationOperation, MutationPlan, MutationReceipt, MutationTarget,
    },
    permissions::ActionContext,
};
use nessa_sdk::domain::agent_execution::subagents::{LifetimeState, MAX_READ_PAGE};
use sha2::{Digest, Sha256};
use std::{collections::BTreeSet, sync::Arc};

type Failure = MutationFailure<ConversationError>;
struct Selection {
    root: MutationBinding,
    children: Vec<MutationBinding>,
}
fn unavailable() -> Failure {
    MutationFailure::Target(ConversationError::Unavailable)
}
fn prefix(root: &MutationBinding) -> String {
    let bytes = serde_json::to_vec(&(
        root.actor().principal_id(),
        root.actor().surface_id(),
        root.actor().request_id(),
        root.target().as_str(),
        root.turn_id().as_str(),
    ))
    .expect("strings serialize");
    format!("parent-stop-{:x}-", Sha256::digest(bytes))
}
fn plan_binding(
    root: &MutationBinding,
    children: &[MutationBinding],
) -> Result<MutationBinding, Failure> {
    let mut hash = Sha256::new();
    hash.update(b"nessa.gateway.parent-stop\0");
    hash.update(root.fingerprint());
    for child in children {
        hash.update(
            serde_json::to_vec(&(
                child.actor().principal_id(),
                child.actor().surface_id(),
                child.actor().request_id(),
                child.target().as_str(),
                child.turn_id().as_str(),
                child.fingerprint(),
            ))
            .expect("bindings serialize"),
        );
    }
    MutationBinding::new(
        root.actor().clone(),
        MutationOperation::Stop,
        root.target().clone(),
        root.turn_id().as_str(),
        hash.finalize().into(),
    )
    .map_err(|_| unavailable())
}
fn restore(
    root: &MutationBinding,
    saved: &MutationReceipt,
    receipts: &[MutationReceipt],
) -> Result<Selection, Failure> {
    let held = saved.binding();
    if held.actor() != root.actor()
        || held.operation() != root.operation()
        || held.target() != root.target()
        || held.turn_id() != root.turn_id()
    {
        return Err(MutationFailure::Conflict);
    }
    // A single-turn Stop retains its canonical digest and has no child marker.
    if held == root {
        return Ok(Selection {
            root: held.clone(),
            children: Vec::new(),
        });
    }
    let prefix = prefix(root);
    let mut children: Vec<_> = receipts
        .iter()
        .filter(|receipt| receipt.binding().actor().request_id().starts_with(&prefix))
        .map(|receipt| receipt.binding().clone())
        .collect();
    children.sort_by(|a, b| a.actor().request_id().cmp(b.actor().request_id()));
    for (index, child) in children.iter().enumerate() {
        if child.operation() != MutationOperation::Stop
            || child.actor().principal_id() != root.actor().principal_id()
            || child.actor().surface_id() != root.actor().surface_id()
            || child.actor().request_id() != format!("{prefix}{index:04}")
        {
            return Err(MutationFailure::Conflict);
        }
    }
    if plan_binding(root, &children)? != *held {
        return Err(MutationFailure::Conflict);
    }
    Ok(Selection {
        root: held.clone(),
        children,
    })
}
impl ConversationService {
    pub(super) async fn stop_owned_command(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: ConversationId,
        caller: ConversationCaller,
        execution: String,
    ) -> Result<MutationReceipt, Failure> {
        let base = stop_binding(&id, &caller, &execution).map_err(MutationFailure::Target)?;
        ensure_live(self, &id, &caller)
            .await
            .map_err(MutationFailure::Target)?;
        let service = self.clone();
        tokio::spawn(async move {
            let _service_admission = service.admit().await.map_err(|_| unavailable())?;
            let admission = service.inner.thread_admission.clone().lock_owned().await;
            let selection = service
                .select_stop(storage.clone(), &id, &caller, base)
                .await?;
            drop(admission);
            let mut root_result = None;
            let mut failure = None;
            // Withdraw report-created parent inputs before cancelling its active
            // turn, so normal queue advancement cannot immediately undo Stop.
            let root_binding = selection.root;
            let mut ordered = selection.children;
            let mut rest = Vec::new();
            ordered.retain(|binding| {
                if binding.target() == root_binding.target() {
                    true
                } else {
                    rest.push(binding.clone());
                    false
                }
            });
            ordered.push(root_binding.clone());
            ordered.extend(rest);
            for binding in ordered {
                let is_root = binding == root_binding;
                let result = service
                    .execute_selected_stop(storage.clone(), binding, &caller)
                    .await;
                match result {
                    Ok(receipt) if is_root => root_result = Some(receipt),
                    Ok(_) => {}
                    Err(error) => {
                        if failure.is_none() {
                            failure = Some(error);
                        }
                    }
                }
            }
            if let Some(error) = failure {
                return Err(error);
            }
            root_result.ok_or_else(unavailable)
        })
        .await
        .map_err(|error| MutationFailure::TaskFault(CreationTaskFault::from_join_error(error)))?
    }

    async fn select_stop(
        &self,
        storage: Arc<dyn CreationStorage>,
        id: &ConversationId,
        caller: &ConversationCaller,
        root: MutationBinding,
    ) -> Result<Selection, Failure> {
        // Exclusive principal leases must be released before nested ordinary
        // mutations, including source/child creation and physical cancellation.
        let lease = storage
            .open_creation(caller.principal_id.as_str().into(), true)
            .await
            .map_err(MutationFailure::Storage)?
            .ok_or_else(unavailable)?;
        let receipts = lease.mutations().await.map_err(MutationFailure::Storage)?;
        if let Some(saved) = lease
            .load_mutation(caller.action_id.as_str())
            .await
            .map_err(MutationFailure::Storage)?
        {
            return restore(&root, &saved, &receipts);
        }
        // A failed partial publication cannot authorize a new selection under
        // the same logical request. No complete root marker means no effects.
        if receipts.iter().any(|receipt| {
            receipt
                .binding()
                .actor()
                .request_id()
                .starts_with(&prefix(&root))
        }) {
            return Err(MutationFailure::Conflict);
        }
        drop(lease);
        let target = StopTarget {
            service: self.clone(),
            id: id.clone(),
            caller: caller.clone(),
            execution_id: root.turn_id().as_str().into(),
        };
        target
            .check(&root, None)
            .await
            .map_err(MutationFailure::Target)?;
        let effect = target
            .classify(&root)
            .await
            .map_err(MutationFailure::Target)?
            == MutationPlan::Effect;
        if !effect {
            return Ok(Selection {
                root,
                children: Vec::new(),
            });
        }
        let mut targets = BTreeSet::new();
        if effect {
            let ownership = self.inner.ownership.get().ok_or_else(unavailable)?;
            if ownership.cascade {
                let mut pending: Vec<_> = ownership
                    .coordinator
                    .active_lifetime_for_session(&conversation_session(id))
                    .into_iter()
                    .collect();
                while let Some(parent) = pending.pop() {
                    let mut cursor = None;
                    loop {
                        let page = ownership
                            .coordinator
                            .children(&parent, cursor.as_ref(), MAX_READ_PAGE)
                            .map_err(|_| unavailable())?;
                        for child in page.children {
                            if child.state == LifetimeState::Closed {
                                continue;
                            }
                            pending.push(child.lifetime);
                            let child_id = ConversationId::new(child.session.as_str())
                                .map_err(|_| unavailable())?;
                            if super::load_conversation(self.inner.metadata.as_ref(), &child_id)
                                .await
                                .map_err(MutationFailure::Target)?
                                .is_some_and(|record| {
                                    record.deletion().is_some_and(|deletion| deletion.erased())
                                })
                            {
                                continue;
                            }
                            // Read committed identities without opening a provider or
                            // admitting automatic resumption during selection. Each
                            // physical target still receives its ordinary Cedar check.
                            let (committed, _) = self
                                .committed_with_order(&child_id)
                                .await
                                .map_err(MutationFailure::Target)?;
                            if let Some(history) = committed.snapshot() {
                                for record in &history.invocations {
                                    if turn_continuation(
                                        &history.invocations,
                                        &record.request.execution_id,
                                    ) == TurnContinuation::Pending
                                    {
                                        targets.insert((
                                            child_id.to_string(),
                                            record.request.execution_id.as_str().to_owned(),
                                        ));
                                    }
                                }
                            }
                        }
                        cursor = page.next;
                        if cursor.is_none() {
                            break;
                        }
                    }
                }
            }
        }
        if effect {
            if let Some(store) = self.inner.thread_reports.store.get() {
                let pending = store.pending().await.map_err(MutationFailure::Target)?;
                let live = self
                    .resolve(id, caller)
                    .await
                    .map_err(MutationFailure::Target)?;
                let queued = live.agent.queued_ids().await;
                let history = live
                    .agent
                    .session_manager()
                    .snapshot()
                    .await
                    .ok_or_else(unavailable)?;
                let lease = storage
                    .open_creation(caller.principal_id.as_str().into(), false)
                    .await
                    .map_err(MutationFailure::Storage)?
                    .ok_or_else(unavailable)?;
                let commands = lease.mutations().await.map_err(MutationFailure::Storage)?;
                drop(lease);
                for report in pending.into_iter().filter(|row| row.report.parent == *id) {
                    let report_ids = super::thread_report_admission::report_execution_ids(
                        &report.report.execution,
                        &history.invocations,
                    );
                    for turn in &queued {
                        let is_report = history
                            .invocations
                            .iter()
                            .find(|record| &record.request.execution_id == turn)
                            .is_some_and(|record| {
                                super::thread_report_admission::is_report_record(&report, record)
                            });
                        let is_command = commands.iter().any(|receipt| {
                            receipt.binding().operation() == MutationOperation::Submit
                                && receipt.binding().target() == &conversation_session(id)
                                && receipt.binding().turn_id() == turn
                        });
                        if report_ids.contains(turn.as_str()) && is_report && !is_command {
                            targets.insert((id.to_string(), turn.as_str().to_owned()));
                        }
                    }
                }
            }
        }
        let prefix = prefix(&root);
        let mut children = Vec::new();
        for (index, (target, turn)) in targets.into_iter().enumerate() {
            let mut child_caller = caller.clone();
            child_caller.action_id = format!("{prefix}{index:04}");
            children.push(
                stop_binding(
                    &ConversationId::new(&target).map_err(|_| unavailable())?,
                    &child_caller,
                    &turn,
                )
                .map_err(MutationFailure::Target)?,
            );
        }
        let selected_root = plan_binding(&root, &children)?;
        let lease = storage
            .open_creation(caller.principal_id.as_str().into(), true)
            .await
            .map_err(MutationFailure::Storage)?
            .ok_or_else(unavailable)?;
        let previous = lease.mutations().await.map_err(MutationFailure::Storage)?;
        // A prior partial write cannot be replaced by a newly selected target.
        for saved in previous
            .iter()
            .filter(|receipt| receipt.binding().actor().request_id().starts_with(&prefix))
        {
            if !children.contains(saved.binding()) {
                return Err(MutationFailure::Conflict);
            }
        }
        for child in &children {
            lease
                .save_mutation(MutationReceipt::accepted(child.clone()))
                .await
                .map_err(MutationFailure::Storage)?;
        }
        lease
            .save_mutation(MutationReceipt::accepted(selected_root.clone()))
            .await
            .map_err(MutationFailure::Storage)?;
        Ok(Selection {
            root: selected_root,
            children,
        })
    }
}

impl ConversationService {
    /// Read complete acknowledged selections. Prefixes only locate candidate
    /// child receipts; the root's digest, never a prefix, binds their authority.
    pub(super) async fn captured_stops(
        &self,
        caller: &ConversationCaller,
    ) -> Result<Vec<(MutationBinding, MutationBinding)>, ConversationError> {
        let Some(storage) = self.commands() else {
            return Ok(Vec::new());
        };
        let Some(lease) = storage
            .open_creation(caller.principal_id.as_str().into(), false)
            .await
            .map_err(|_| ConversationError::Unavailable)?
        else {
            return Ok(Vec::new());
        };
        let receipts = lease
            .mutations()
            .await
            .map_err(|_| ConversationError::Unavailable)?;
        let mut captured = Vec::new();
        for receipt in &receipts {
            let binding = receipt.binding();
            if binding.operation() != MutationOperation::Stop {
                continue;
            }
            let id = ConversationId::new(binding.target().as_str())
                .map_err(|_| ConversationError::Unavailable)?;
            let mut origin = caller.clone();
            origin.surface_id = binding.actor().surface_id().into();
            origin.action_id = binding.actor().request_id().into();
            let base = stop_binding(&id, &origin, binding.turn_id().as_str())?;
            if binding == &base {
                if matches!(receipt.stage(), nessa_sdk::application::agent_execution::commands::MutationStage::Attempted)
                    || matches!(receipt.outcome(), Some(nessa_sdk::application::agent_execution::commands::MutationOutcome::Cancelled | nessa_sdk::application::agent_execution::commands::MutationOutcome::Withdrawn)) {
                    captured.push((binding.clone(), binding.clone()));
                }
                continue;
            }
            let selection =
                restore(&base, receipt, &receipts).map_err(|_| ConversationError::Unavailable)?;
            for selected in std::iter::once(selection.root).chain(selection.children) {
                captured.push((base.clone(), selected));
            }
        }
        Ok(captured)
    }

    pub(super) async fn captured_stop_actor(
        &self,
        id: &ConversationId,
        execution: &str,
        caller: &ConversationCaller,
    ) -> Result<Option<ActionContext>, ConversationError> {
        if self.inner.ownership.get().is_none() {
            return Ok(None);
        }
        Ok(self
            .captured_stops(caller)
            .await?
            .into_iter()
            .find_map(|(root, target)| {
                (target.target() == &conversation_session(id)
                    && target.turn_id().as_str() == execution)
                    .then(|| root.actor().clone())
            }))
    }

    pub(super) async fn selected_stop_lookup(
        &self,
        storage: Arc<dyn CreationStorage>,
        requested: &MutationBinding,
    ) -> Result<MutationBinding, Failure> {
        let Some(lease) = storage
            .open_creation(requested.actor().principal_id().into(), false)
            .await
            .map_err(MutationFailure::Storage)?
        else {
            return Ok(requested.clone());
        };
        let Some(saved) = lease
            .load_mutation(requested.actor().request_id())
            .await
            .map_err(MutationFailure::Storage)?
        else {
            return Ok(requested.clone());
        };
        let receipts = lease.mutations().await.map_err(MutationFailure::Storage)?;
        let selection = restore(requested, &saved, &receipts)?;
        for child in &selection.children {
            let receipt = receipts
                .iter()
                .find(|receipt| receipt.binding() == child)
                .ok_or_else(unavailable)?;
            if receipt.stage()
                != nessa_sdk::application::agent_execution::commands::MutationStage::Settled
            {
                return Err(MutationFailure::Interrupted(Box::new(receipt.clone())));
            }
        }
        Ok(selection.root)
    }
}

fn selection_actor(caller: &ConversationCaller) -> Result<ActionContext, Failure> {
    caller.actor().map_err(MutationFailure::Target)
}

impl ConversationService {
    async fn execute_selected_stop(
        &self,
        storage: Arc<dyn CreationStorage>,
        binding: MutationBinding,
        caller: &ConversationCaller,
    ) -> Result<MutationReceipt, Failure> {
        let id = ConversationId::new(binding.target().as_str()).map_err(|_| unavailable())?;
        let mut target_caller = caller.clone();
        target_caller.action_id = binding.actor().request_id().into();
        ensure_live(self, &id, &target_caller)
            .await
            .map_err(MutationFailure::Target)?;
        let live = self
            .resolve(&id, &target_caller)
            .await
            .map_err(MutationFailure::Target)?;
        loop {
            if live.agent.active_execution_id().as_ref() == Some(binding.turn_id())
                || live.agent.queued_ids().await.contains(binding.turn_id())
            {
                break;
            }
            let mut declined = false;
            for owed in live.agent.owed_turns().await {
                if owed.execution_id() == binding.turn_id() {
                    live.agent
                        .decline_turn(&owed, selection_actor(caller)?)
                        .await
                        .map_err(|error| {
                            MutationFailure::Target(ConversationError::Agent(error))
                        })?;
                    declined = true;
                }
            }
            if declined
                || !live
                    .agent
                    .unsettled_execution_ids()
                    .await
                    .contains(binding.turn_id())
            {
                break;
            }
            // A locally owned dequeue-to-dispatch handoff is still the exact
            // selected execution. Wait for its existing owner; never target the
            // next active execution or replay an uncertain provider effect.
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        let target = Arc::new(StopTarget {
            service: self.clone(),
            id,
            caller: target_caller,
            execution_id: binding.turn_id().as_str().into(),
        });
        MutationCoordinator::new(storage)
            .execute(binding, target)
            .await
    }
}

impl ConversationService {
    /// One startup pass over durable exact selections, after ownership and
    /// command storage are bound. This does not select current work again.
    pub async fn recover_parent_stops(
        &self,
        snapshot: &nessa_sdk::domain::agent_execution::subagents::OwnershipSnapshot,
    ) -> Result<usize, ConversationError> {
        let Some(storage) = self.commands() else {
            return Ok(0);
        };
        let mut owners = std::collections::BTreeMap::new();
        for row in &snapshot.lifetimes {
            let id = ConversationId::new(row.session_id.as_str())
                .map_err(|_| ConversationError::Unavailable)?;
            if let Some(record) =
                super::load_conversation(self.inner.metadata.as_ref(), &id).await?
            {
                owners.insert(
                    record.owner().as_str().to_owned(),
                    record.organization().clone(),
                );
            }
        }
        let mut retry = Vec::new();
        for (principal, organization_id) in owners {
            let Some(lease) = storage
                .open_creation(principal.clone(), false)
                .await
                .map_err(|_| ConversationError::Unavailable)?
            else {
                continue;
            };
            let receipts = lease
                .mutations()
                .await
                .map_err(|_| ConversationError::Unavailable)?;
            for receipt in &receipts {
                let binding = receipt.binding();
                if binding.operation() != MutationOperation::Stop {
                    continue;
                }
                let id = ConversationId::new(binding.target().as_str())
                    .map_err(|_| ConversationError::Unavailable)?;
                let caller = ConversationCaller {
                    organization_id: organization_id.clone(),
                    principal_id: nessa_auth::domain::PrincipalId::new(&principal)
                        .map_err(|_| ConversationError::Unavailable)?,
                    surface_id: binding.actor().surface_id().into(),
                    action_id: binding.actor().request_id().into(),
                };
                let base = stop_binding(&id, &caller, binding.turn_id().as_str())?;
                if binding == &base {
                    continue;
                }
                let selection = restore(&base, receipt, &receipts)
                    .map_err(|_| ConversationError::Unavailable)?;
                let complete = receipt.stage() == nessa_sdk::application::agent_execution::commands::MutationStage::Settled
                    && selection.children.iter().all(|child| receipts.iter().any(|receipt| receipt.binding() == child && receipt.stage() == nessa_sdk::application::agent_execution::commands::MutationStage::Settled));
                if !complete {
                    retry.push((id, caller, binding.turn_id().as_str().to_owned()));
                }
            }
        }
        let count = retry.len();
        let service = self.clone();
        tokio::spawn(async move {
            for (id, caller, execution) in retry {
                if service.inner.retirement.get().is_some() {
                    break;
                }
                loop {
                    let result = service
                        .stop_command(
                            storage.clone(),
                            id.clone(),
                            caller.clone(),
                            execution.clone(),
                        )
                        .await;
                    match result {
                        Err(MutationFailure::Storage(nessa_sdk::application::agent_execution::commands::CreationStorageError::Storage(nessa_sdk::application::agent_execution::sessions::StorageError::Busy)))
                            if service.inner.retirement.get().is_none() => {
                                // Exclusive command leases can be occupied by a
                                // read-only lookup. Busy admits no new effect;
                                // retry the same durable selection only.
                                tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                        }
                        Err(error) => {
                            tracing::warn!(conversation_id=%id, ?error, "restored exact parent Stop remains unconfirmed");
                            break;
                        }
                        Ok(_) => break,
                    }
                }
            }
        });
        Ok(count)
    }
}
