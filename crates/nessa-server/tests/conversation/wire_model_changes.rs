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
use nessa_protocol::conversation::domain::ConversationApprovalMode;
use nessa_protocol::conversation::projection::Projection;
use nessa_protocol::conversation::view::{
    ConversationCapabilities, ConversationRuntime, ConversationSelectionView,
};
use nessa_protocol::product::generated::AgentsListResult;
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
