//! A confirmed model effect survives the real application-to-product boundary.
use super::{wire_view, ConversationModelChangeEffect, ProductRouteState};
use crate::agents_test_support::StubAgentProbe;
use crate::conversation::application::{
    ConversationError, ConversationFuture, PlanToken, PreferenceReading, RecordedPreference,
    ScheduleActor, ScheduleDraft, ScheduleSettlement, ScheduledMessage, ScheduledMessageStore,
    SelectionRequest,
};
use crate::conversation::domain::ContinuationCause;
use crate::conversation_test_support::mode_fixture;
use crate::product::ProductDependencies;
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::pairing::{OwnerPhonePurpose, OwnerPhonePurposeSource};
use nessa_auth::application::ports::{
    AccessReader, AccessSnapshot, Clock, CredentialEvidence, CredentialVerifier, PortFuture,
    VerifiedCredential,
};
use nessa_auth::application::session::AuthenticatedSession;
use nessa_auth::domain::{AudienceId, CredentialId, OrganizationId, ResourceId};
use nessa_protocol::agents::AgentId;
use nessa_protocol::clock::Clock as UptimeClock;
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationModelId};
use nessa_protocol::conversation::projection::{
    bound_view, question_entry, review_entry, Projection, MAX_LISTED_REVIEWS, MAX_VIEW_BYTES,
};
use nessa_protocol::conversation::view::{
    ConversationCapabilities, ConversationMessage, ConversationMessageStatus,
    ConversationModelChangeStatus, ConversationModelChangeView, ConversationReviewKind,
    ConversationRuntime, ConversationSelectionView, ConversationUsageLimit,
    ConversationUsageLimitProvider, ConversationUsageLimitRecovery,
};
use nessa_protocol::product::generated::AgentsListResult;
use nessa_protocol::protocol::{OutgoingMessage, ResponseFrame, MAX_PAYLOAD_BYTES};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::questions::MAX_OPEN_QUESTIONS;
use serde_json::json;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::Arc;

#[tokio::test]
async fn subsequent_response_model_effect_survives_the_product_wire_view() {
    confirmed_effect(
        ConversationModelChangeEffect::SubsequentResponses,
        "subsequentResponses",
    )
    .await;
}

#[tokio::test]
async fn next_turn_model_effect_survives_the_product_wire_view() {
    confirmed_effect(ConversationModelChangeEffect::NextTurn, "nextTurn").await;
}

#[tokio::test]
async fn an_unopened_view_carries_the_conversations_model() {
    let unused = Arc::new(UnusedAuthority);
    let mut state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: unused.clone(),
            owner_phone_purpose: unused.clone(),
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new().unwrap(),
            ),
            owner_phone_receivers: None,
            access: unused.clone(),
            clock: unused.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: unused,
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    );
    let catalog: AgentsListResult = serde_json::from_value(json!({
        "agents":[{"agent":"claude","defaultModel":"selected-model","models":[{
            "modelId":"selected-model","displayName":"Selected model",
            "maxContextWindowTokens":100000,"reasoning":false,"imageInput":false,
            "approvalModes":[{"id":"ask","name":"Ask","description":"Ask first"}]
        }]}],"environments":[],"commandHosts":[],
        "nativeHooks":{"claude":"off","codex":"off"}
    }))
    .unwrap();
    state.agents_catalog = Some(Arc::new(catalog));
    let mut projection = Projection::new(
        "11111111-1111-4111-8111-111111111111".into(),
        ConversationCapabilities::read_only(),
        None,
    );
    projection.selection(ConversationSelectionView {
        agent: AgentId::Claude,
        model: "selected-model".into(),
        approval_mode: ConversationApprovalMode::Ask,
    });
    let wire = wire_view(&state, projection.read())
        .await
        .expect("selection is enough for a wire view");
    let value = serde_json::to_value(wire).unwrap();
    assert!(value.get("runtime").is_none());
    assert_eq!(value["model"], "selected-model");
}

async fn confirmed_effect(effect: ConversationModelChangeEffect, expected: &str) {
    let unused = Arc::new(UnusedAuthority);
    let mut state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: unused.clone(),
            owner_phone_purpose: unused.clone(),
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new().unwrap(),
            ),
            owner_phone_receivers: None,
            access: unused.clone(),
            clock: unused.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: unused,
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    );
    let catalog: AgentsListResult = serde_json::from_value(json!({
        "agents":[{"agent":"claude","defaultModel":"selected-model","models":[{
            "modelId":"selected-model","displayName":"Selected model",
            "maxContextWindowTokens":100000,"reasoning":false,"imageInput":false,
            "approvalModes":[{"id":"ask","name":"Ask","description":"Ask first"}]
        }]}],"environments":[],"commandHosts":[],
        "nativeHooks":{"claude":"off","codex":"off"}
    }))
    .unwrap();
    state.agents_catalog = Some(Arc::new(catalog));
    let mut projection = Projection::new(
        "11111111-1111-4111-8111-111111111111".into(),
        ConversationCapabilities::read_only(),
        None,
    );
    projection.selection(ConversationSelectionView {
        agent: AgentId::Claude,
        model: "prior-model".into(),
        approval_mode: ConversationApprovalMode::Auto,
    });
    projection.runtime(ConversationRuntime {
        provider: "anthropic".into(),
        model: "prior-model".into(),
        workspace: "/fixture".into(),
    });
    projection.model_selection(
        "selected-model".into(),
        ConversationApprovalMode::Ask,
        effect,
    );
    let wire = wire_view(&state, projection.read())
        .await
        .expect("confirmed model must have a wire view");
    let value = serde_json::to_value(wire).unwrap();
    assert_eq!(value["modelChangeEffect"], expected);
    assert_eq!(value["approvalMode"], "ask");
    assert_eq!(value["model"], "selected-model");
    assert_eq!(value["runtime"]["model"], "selected-model");
    assert_eq!(value["runtime"]["modelName"], "Selected model");
    assert!(value.get("modelChange").is_none());
    wire_control_budget(&mut state, &mut projection, effect).await;
}

// The base projection reserves room for catalog enrichment. Test the real
// product payload and public RPC encoder against the transport's own bound.
async fn wire_control_budget(
    state: &mut ProductRouteState,
    projection: &mut Projection,
    effect: ConversationModelChangeEffect,
) {
    let model = ConversationModelId::new("\"".repeat(ConversationModelId::MAX_BYTES)).unwrap();
    let requested = ConversationModelId::new("\\".repeat(ConversationModelId::MAX_BYTES)).unwrap();
    let actor = ActionContext::new(
        "person",
        "panel",
        "\u{1}".repeat(ActionContext::MAX_IDENTITY_BYTES),
    )
    .unwrap();
    Arc::get_mut(state.agents_catalog.as_mut().unwrap())
        .unwrap()
        .agents[0]
        .models[0]
        .model_id = model.as_str().to_owned();
    projection.model_selection(
        model.as_str().to_owned(),
        ConversationApprovalMode::Ask,
        effect,
    );
    let mut view = projection.read_with_model_change(Some(ConversationModelChangeView {
        request_id: actor.request_id().to_owned(),
        requested_model: requested.as_str().to_owned(),
        status: ConversationModelChangeStatus::RecoveryRequired,
    }));
    view.messages = (0..40)
        .map(|index| ConversationMessage {
            authentication_required: None,
            usage_limit: None,
            required_work_failed: None,
            parts: Vec::new(),
            steering_offset: None,
            event_count: 0,
            retained_text: 0,
            execution_id: format!("turn-{index}"),
            user_text: "y".repeat(8_000),
            attachments: Vec::new(),
            files: Vec::new(),
            app: None,
            steering_target: None,
            status: ConversationMessageStatus::Completed,
            error: None,
        })
        .collect();
    view.permissions = (0..MAX_LISTED_REVIEWS)
        .map(|index| {
            review_entry(
                &format!("{index:032x}"),
                ConversationReviewKind::App,
                &"\u{1}".repeat(1_000),
            )
        })
        .collect();
    view.questions = (0..MAX_OPEN_QUESTIONS)
        .map(|index| question_entry(&format!("{index:032x}"), &"\u{1}".repeat(1_000)))
        .collect();
    let permissions = serde_json::to_value(&view.permissions).unwrap();
    let questions = serde_json::to_value(&view.questions).unwrap();
    let pending = serde_json::to_value(&view.model_change).unwrap();
    assert!(serde_json::to_vec(&view).unwrap().len() > MAX_VIEW_BYTES);
    let bounded = bound_view(view);
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= MAX_VIEW_BYTES);
    let wire = wire_view(state, bounded).await.unwrap();
    let value = serde_json::to_value(&wire).unwrap();
    assert_eq!(value["permissions"], permissions);
    assert_eq!(value["questions"], questions);
    assert_eq!(value["modelChange"], pending);
    assert_eq!(value["runtime"]["model"], model.as_str());
    assert_eq!(value["runtime"]["modelName"], "Selected model");
    assert_eq!(value["approvalMode"], "ask");
    assert_eq!(
        value["modelChangeEffect"],
        serde_json::to_value(effect).unwrap()
    );
    let response =
        OutgoingMessage::Response(ResponseFrame::success(actor.request_id(), &wire).unwrap());
    let text = response.to_wire_text().unwrap();
    assert!(
        text.len() <= MAX_PAYLOAD_BYTES as usize,
        "catalog-enriched response {} exceeds {} bytes",
        text.len(),
        MAX_PAYLOAD_BYTES
    );
}

// Wire enrichment must not consult credential authorities or open providers.
struct UnusedAuthority;
impl CredentialVerifier for UnusedAuthority {
    fn verify<'a>(
        &'a self,
        _: &'a CredentialEvidence,
        _: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        panic!("wire enrichment must not authenticate")
    }
}
impl AccessReader for UnusedAuthority {
    fn read<'a>(&'a self, _: &'a CredentialId) -> PortFuture<'a, AccessSnapshot> {
        panic!("wire enrichment must not read credentials")
    }
}
impl Clock for UnusedAuthority {
    fn unix_milliseconds(&self) -> u64 {
        1000
    }
}
impl UptimeClock for UnusedAuthority {
    fn elapsed_ms(&self) -> u64 {
        0
    }
}

impl OwnerPhonePurposeSource for UnusedAuthority {
    fn owner_phone_purpose<'a>(
        &'a self,
        _: &'a AuthenticatedSession,
        _: &'a dyn Clock,
    ) -> PortFuture<'a, Option<OwnerPhonePurpose>> {
        panic!("wire enrichment must not classify credentials")
    }
}

#[tokio::test]
async fn a_schedule_store_failure_fails_the_conversation_view() {
    let mut state = reading_state();
    let projection = reading_projection();

    let (unbound, _, _, _, _) = mode_fixture();
    state.conversations = Some(Arc::new(unbound));
    let wire = wire_view(&state, projection.read())
        .await
        .expect("no schedule database is preference off");
    assert_eq!(
        serde_json::to_value(wire).unwrap()["usageLimitAutoContinue"],
        false
    );
    state
        .conversations
        .take()
        .unwrap()
        .shutdown()
        .await
        .unwrap();

    let (off, _, _, _, _) = mode_fixture();
    off.bind_scheduled_messages(Arc::new(ScheduleReads::new(false, false, false)));
    state.conversations = Some(Arc::new(off));
    let wire = wire_view(&state, projection.read())
        .await
        .expect("a stored off preference is off");
    assert_eq!(
        serde_json::to_value(wire).unwrap()["usageLimitAutoContinue"],
        false
    );
    state
        .conversations
        .take()
        .unwrap()
        .shutdown()
        .await
        .unwrap();

    let (preference, _, _, _, _) = mode_fixture();
    preference.bind_scheduled_messages(Arc::new(ScheduleReads::new(true, false, true)));
    state.conversations = Some(Arc::new(preference));
    assert!(
        wire_view(&state, projection.read()).await.is_err(),
        "a preference read failure is not preference off"
    );
    state
        .conversations
        .take()
        .unwrap()
        .shutdown()
        .await
        .unwrap();

    let (open, _, _, _, _) = mode_fixture();
    open.bind_scheduled_messages(Arc::new(ScheduleReads::new(false, true, true)));
    state.conversations = Some(Arc::new(open));
    assert!(
        wire_view(&state, projection.read()).await.is_err(),
        "an open-row read failure does not drop auto-resuming"
    );
    state
        .conversations
        .take()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

/// Two separate reads would disagree. The first preference read is `first`.
/// The next preference read is the opposite, and an open row is waiting.
/// The view follows the one snapshot: off publishes no auto-resuming row,
/// and on publishes the row from that same read.
#[tokio::test]
async fn a_conversation_view_reads_the_preference_and_the_schedule_from_one_snapshot() {
    view_snapshot(false, false, false).await;
    view_snapshot(true, true, true).await;
}

async fn view_snapshot(first: bool, expect_enabled: bool, expect_resuming: bool) {
    let mut state = reading_state();
    let mut view = reading_projection().read();
    view.messages.push(limit_message("turn-1"));
    let (service, _, _, _, _) = mode_fixture();
    let mut reads = ScheduleReads::new(false, false, !first);
    reads.flip_first = Some(first);
    reads.open_row = Some(open_row("turn-1"));
    service.bind_scheduled_messages(Arc::new(reads));
    state.conversations = Some(Arc::new(service));
    let wire = wire_view(&state, view)
        .await
        .expect("one snapshot is readable");
    let json = serde_json::to_value(&wire).unwrap();
    assert_eq!(json["usageLimitAutoContinue"], expect_enabled);
    let resuming = json["messages"][0]["usageLimit"].get("autoResuming");
    if expect_resuming {
        assert_eq!(resuming, Some(&json!(true)));
    } else {
        assert!(resuming.is_none(), "{resuming:?}");
    }
    state
        .conversations
        .take()
        .unwrap()
        .shutdown()
        .await
        .unwrap();
}

fn limit_message(execution_id: &str) -> ConversationMessage {
    ConversationMessage {
        authentication_required: None,
        required_work_failed: None,
        usage_limit: Some(ConversationUsageLimit {
            provider: ConversationUsageLimitProvider::Claude,
            recovery: ConversationUsageLimitRecovery::TimedReset,
            resets_at_ms: Some(1_700_000_000_000),
            auto_resuming: None,
        }),
        parts: Vec::new(),
        steering_offset: None,
        event_count: 0,
        retained_text: 0,
        execution_id: execution_id.into(),
        user_text: String::new(),
        attachments: Vec::new(),
        files: Vec::new(),
        app: None,
        steering_target: None,
        status: ConversationMessageStatus::Failed,
        error: None,
    }
}

fn open_row(causal: &str) -> ScheduledMessage {
    ScheduledMessage {
        command_evidence: None,
        conversation: "11111111-1111-4111-8111-111111111111".into(),
        causal_execution: causal.into(),
        execution_id: "sched-1".into(),
        due_at_ms: 50,
        delivering: false,
        agent: "claude".into(),
        model: "opus".into(),
        prompt: "continue".into(),
        organization_id: "org".into(),
        principal_id: "person".into(),
        surface_id: "desktop".into(),
        action_id: "action".into(),
        initiation: "owner".into(),
        grant_id: None,
        policy_revision: None,
        tools_checked: false,
        retry_failures: 0,
    }
}

fn reading_state() -> ProductRouteState {
    let unused = Arc::new(UnusedAuthority);
    let mut state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("organization").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: unused.clone(),
            owner_phone_purpose: unused.clone(),
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new().unwrap(),
            ),
            owner_phone_receivers: None,
            access: unused.clone(),
            clock: unused.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            uptime_clock: unused,
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    );
    let catalog: AgentsListResult = serde_json::from_value(json!({
        "agents":[{"agent":"claude","defaultModel":"selected-model","models":[{
            "modelId":"selected-model","displayName":"Selected model",
            "maxContextWindowTokens":100000,"reasoning":false,"imageInput":false,
            "approvalModes":[{"id":"ask","name":"Ask","description":"Ask first"}]
        }]}],"environments":[],"commandHosts":[],
        "nativeHooks":{"claude":"off","codex":"off"}
    }))
    .unwrap();
    state.agents_catalog = Some(Arc::new(catalog));
    state
}

fn reading_projection() -> Projection {
    let mut projection = Projection::new(
        "11111111-1111-4111-8111-111111111111".into(),
        ConversationCapabilities::read_only(),
        None,
    );
    projection.selection(ConversationSelectionView {
        agent: AgentId::Claude,
        model: "selected-model".into(),
        approval_mode: ConversationApprovalMode::Ask,
    });
    projection
}

/// A schedule store whose reads fail, or return a stored preference.
///
/// `flip_first` makes successive preference reads disagree. The open row is
/// still there. A view that reads both through one snapshot does not publish
/// that disagreement.
struct ScheduleReads {
    fail_preference: bool,
    fail_open: bool,
    enabled: bool,
    flip_first: Option<bool>,
    open_row: Option<ScheduledMessage>,
    reads: AtomicUsize,
}

impl ScheduleReads {
    fn new(fail_preference: bool, fail_open: bool, enabled: bool) -> Self {
        Self {
            fail_preference,
            fail_open,
            enabled,
            flip_first: None,
            open_row: None,
            reads: AtomicUsize::new(0),
        }
    }
}

fn schedule_unavailable<'a, T: Send + 'static>() -> ConversationFuture<'a, T> {
    Box::pin(async { Err(ConversationError::Unavailable) })
}

impl ScheduledMessageStore for ScheduleReads {
    // This double exercises unrelated store behavior; model invalidation has
    // dedicated tests against LocalScheduledMessages.
    fn invalidate_usage_limit(
        &self,
        _: SelectionRequest<'_>,
        _: &str,
        _: Option<&str>,
        _: &crate::conversation::application::PlanToken,
        _: crate::conversation::application::ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        Box::pin(async { Ok(true) })
    }
    fn preference(&self) -> ConversationFuture<'_, PreferenceReading> {
        if self.fail_preference {
            return schedule_unavailable();
        }
        let enabled = if let Some(first) = self.flip_first {
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            if read == 0 {
                first
            } else {
                !first
            }
        } else {
            self.enabled
        };
        Box::pin(async move {
            Ok(PreferenceReading {
                enabled,
                revision: 1,
            })
        })
    }
    fn set_preference(&self, _: bool, _: ScheduleActor, _: u64) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn open(&self, _: &str) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        if self.fail_open {
            return schedule_unavailable();
        }
        let row = self.open_row.clone();
        Box::pin(async move { Ok(row) })
    }
    fn accept(&self, _: ScheduleDraft, _: u64) -> ConversationFuture<'_, Option<ScheduledMessage>> {
        schedule_unavailable()
    }
    fn move_due(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn defer_claim(
        &self,
        _: &str,
        _: &str,
        _: u64,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn cancel_open(
        &self,
        _: &str,
        _: ContinuationCause,
        _: ScheduleActor,
        _: &PlanToken,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn claim_due(&self, _: u64, _: usize) -> ConversationFuture<'_, Vec<ScheduledMessage>> {
        schedule_unavailable()
    }
    fn next_due(&self, _: u64) -> ConversationFuture<'_, Option<u64>> {
        schedule_unavailable()
    }
    fn begin_submit(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn renew_fence(&self, _: &str, _: u64) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn release_submit(&self, _: &str) -> ConversationFuture<'_, ()> {
        schedule_unavailable()
    }
    fn record_preference_decision(
        &self,
        _: bool,
        _: bool,
        _: &[String],
        _: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, RecordedPreference> {
        schedule_unavailable()
    }
    fn settle_installed_preference(&self, _: u64) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn revert_installed_preference(
        &self,
        _: u64,
        _: ScheduleActor,
        _: u64,
    ) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn status(&self, _: &str) -> ConversationFuture<'_, Option<String>> {
        schedule_unavailable()
    }
    fn record_outcome(
        &self,
        _: &str,
        _: ScheduleSettlement,
        _: &str,
        _: u64,
    ) -> ConversationFuture<'_, ()> {
        schedule_unavailable()
    }
    fn erase_conversation(&self, _: &str, _: u64) -> ConversationFuture<'_, ()> {
        schedule_unavailable()
    }
    fn defer_submit(&self, _: &str, _: u32, _: u64, _: u64) -> ConversationFuture<'_, bool> {
        schedule_unavailable()
    }
    fn note_reconcile_retry(&self, _: &str, _: u32, _: u64) -> ConversationFuture<'_, ()> {
        schedule_unavailable()
    }
    fn clear_reconcile_retry(&self, _: &str) -> ConversationFuture<'_, ()> {
        schedule_unavailable()
    }
    fn reconcile_retries(
        &self,
    ) -> ConversationFuture<'_, Vec<crate::conversation::application::ReconcileRetry>> {
        schedule_unavailable()
    }
}
