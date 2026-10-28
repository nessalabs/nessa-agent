//! An owner phone may start a conversation only in Ask, and a channel's
//! preset counts: naming a channel whose new conversations start in Auto,
//! with no preset of its own, is refused before anything is created.
use super::{fixture, ActualHolding};
use crate::agents_test_support::StubAgentProbe;
use crate::conversation::application::ConversationLimits;
use crate::conversation::domain::{
    ChannelDefaults, ChannelName, ChannelNotes, ChannelPath, ChannelRequest, ChannelResources,
};
use crate::conversation_test_support::absolute;
use crate::conversation_test_support::{self, EveryConversationGranted};
use crate::product::{dispatch_conversation, ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::application::ports::Clock;
use nessa_auth::domain::{AudienceId, OrganizationId, ResourceId};
use nessa_protocol::protocol::{OutgoingMessage, RequestFrame, ResponseFrame};
use nessa_sdk::domain::agent_execution::executions::Initiation;
use serde_json::{json, Value};
use std::sync::Arc;

struct FixedClock;
impl Clock for FixedClock {
    fn unix_milliseconds(&self) -> u64 {
        110_001
    }
}
impl nessa_protocol::clock::Clock for FixedClock {
    fn elapsed_ms(&self) -> u64 {
        1
    }
}

fn bind_command_store(service: &crate::conversation::application::ConversationService) {
    let directory = tempfile::tempdir().expect("command store");
    let storage = Arc::new(
        nessa_sdk::infrastructure::session_storage::RecordStorage::new(
            directory.path().join("records"),
        )
        .expect("command store"),
    );
    service.bind_commands(storage);
    std::mem::forget(directory);
}

fn response(message: OutgoingMessage) -> ResponseFrame {
    match message {
        OutgoingMessage::Response(frame) => frame,
        OutgoingMessage::Event(frame) => panic!("unexpected event {}", frame.event),
    }
}

async fn call(
    state: &ProductRouteState,
    session: &nessa_auth::application::session::AuthenticatedSession,
    id: &str,
    method: &str,
    params: Value,
) -> ResponseFrame {
    response(
        dispatch_conversation(
            state,
            session,
            RequestFrame::new(id, method, &params).unwrap(),
            Initiation::owner(),
        )
        .await,
    )
}

/// Accepts any folder as named and makes one under `/data/channels`.
struct AnyFolder;
impl crate::conversation::application::ChannelFolders for AnyFolder {
    fn accept_folder(
        &self,
        chosen: &ChannelPath,
    ) -> Result<ChannelPath, crate::conversation::application::FolderRefusal> {
        Ok(chosen.clone())
    }
    fn accept_resource(
        &self,
        path: &ChannelPath,
    ) -> Result<ChannelPath, crate::conversation::application::FolderRefusal> {
        Ok(path.clone())
    }
    fn make(
        &self,
        id: &crate::conversation::domain::ChannelId,
    ) -> Result<ChannelPath, crate::conversation::application::FolderRefusal> {
        Ok(ChannelPath::new(std::path::PathBuf::from(absolute(&format!(
            "/data/channels/{id}"
        ))))
        .unwrap())
    }
}

#[tokio::test]
async fn owner_phone_creation_in_a_channel_is_judged_by_the_channels_preset() {
    let phone = fixture().await;
    let (service, _, repository, _) =
        conversation_test_support::fixture(ConversationLimits::default());
    bind_command_store(&service);
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("gateway");
    nessa_local_storage::create_directory(&root).unwrap();
    service.bind_channels(
        Arc::new(
            crate::conversation::infrastructure::LocalChannelStore::open(
                &root.join("channels.sqlite3"),
            )
            .unwrap(),
        ),
        Arc::new(AnyFolder),
    );
    let context = phone.session.context();
    let owner = crate::conversation::application::ConversationCaller {
        organization_id: context.organization_id().clone(),
        principal_id: context.principal_id().clone(),
        surface_id: "desktop".into(),
        action_id: "make".into(),
    };
    let agent = nessa_protocol::agents::AgentId::Claude;
    let channel = |mode| ChannelRequest {
        execution: crate::conversation::domain::ChannelExecution::default(),
        name: ChannelName::new("design-review").unwrap(),
        folder: None,
        defaults: ChannelDefaults {
            agent,
            model: nessa_protocol::conversation::domain::ConversationModelId::new("test").unwrap(),
            mode,
        },
        notes: ChannelNotes::new("").unwrap(),
        resources: ChannelResources::new(vec![]).unwrap(),
    };
    use nessa_protocol::conversation::domain::ConversationApprovalMode::{Ask, Auto};
    let auto = service
        .create_channel(owner.clone(), "auto".into(), channel(Auto))
        .await
        .unwrap();
    let ask = service
        .create_channel(owner, "ask".into(), channel(Ask))
        .await
        .unwrap();
    let clock = Arc::new(FixedClock);
    let state = ProductRouteState::new(
        ResourceId::new("gateway").unwrap(),
        OrganizationId::new("org").unwrap(),
        AudienceId::new("gateway").unwrap(),
        ProductDependencies {
            verifier: phone.registry.clone(),
            access: phone.registry.clone(),
            clock: clock.clone(),
            policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
            owner_phone_purpose: phone.registry.clone(),
            owner_phone_policy: Arc::new(
                crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new().unwrap(),
            ),
            owner_phone_receivers: Some(Arc::new(ActualHolding(phone.receivers.clone()))),
            uptime_clock: clock,
            agent_probe: Arc::new(StubAgentProbe::answering(false, false)),
        },
    )
    .with_conversations(Arc::new(service))
    .with_passive_read(
        phone.receivers.clone(),
        repository,
        Arc::new(EveryConversationGranted),
        conversation_test_support::access_policy(),
    );
    let refused = call(
        &state,
        &phone.session,
        "in-auto",
        "conversation.create",
        json!({
            "conversationId": "00000000-0000-4000-8000-0000000000c1",
            "requestId": "in-auto",
            "channelId": auto.id().as_str(),
        }),
    )
    .await;
    assert_eq!(
        refused.error.map(|error| error.code).as_deref(),
        Some("forbidden")
    );
    // Naming Ask itself, or a channel that starts in Ask, is admitted. This
    // fixture's agents are fixed, so none can be built in a channel's folder
    // and the creation stops there: past the phone's check, not at it.
    for (id, params) in [
        (
            "00000000-0000-4000-8000-0000000000c2",
            json!({"channelId": auto.id().as_str(), "approvalMode": "ask"}),
        ),
        (
            "00000000-0000-4000-8000-0000000000c3",
            json!({"channelId": ask.id().as_str()}),
        ),
    ] {
        let mut params = params;
        params["conversationId"] = json!(id);
        params["requestId"] = json!(id);
        let admitted = call(&state, &phone.session, id, "conversation.create", params).await;
        assert_ne!(
            admitted.error.map(|error| error.code).as_deref(),
            Some("forbidden"),
            "{id}"
        );
    }
}
