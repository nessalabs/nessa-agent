//! A confirmed model effect survives the real application-to-product boundary.
use super::{wire_view, ConversationModelChangeEffect, ProductRouteState};
use crate::agents_test_support::StubAgentProbe;
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
    ConversationRuntime, ConversationSelectionView,
};
use nessa_protocol::product::generated::AgentsListResult;
use nessa_protocol::protocol::{OutgoingMessage, ResponseFrame, MAX_PAYLOAD_BYTES};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::domain::agent_execution::questions::MAX_OPEN_QUESTIONS;
use serde_json::json;
use std::sync::Arc;

#[test]
fn subsequent_response_model_effect_survives_the_product_wire_view() {
    confirmed_effect(
        ConversationModelChangeEffect::SubsequentResponses,
        "subsequentResponses",
    );
}

#[test]
fn next_turn_model_effect_survives_the_product_wire_view() {
    confirmed_effect(ConversationModelChangeEffect::NextTurn, "nextTurn");
}

fn confirmed_effect(effect: ConversationModelChangeEffect, expected: &str) {
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
    let wire = wire_view(&state, projection.read()).expect("confirmed model must have a wire view");
    let value = serde_json::to_value(wire).unwrap();
    assert_eq!(value["modelChangeEffect"], expected);
    assert_eq!(value["approvalMode"], "ask");
    assert_eq!(value["runtime"]["model"], "selected-model");
    assert_eq!(value["runtime"]["modelName"], "Selected model");
    assert!(value.get("modelChange").is_none());
    wire_control_budget(&mut state, &mut projection, effect);
}

// The base projection reserves room for catalog enrichment. Test the real
// product payload and public RPC encoder against the transport's own bound.
fn wire_control_budget(
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
    let wire = wire_view(state, bounded).unwrap();
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
