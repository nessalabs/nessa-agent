use super::*;

/// A configuration naming both agents, with no file on disk for either.
///
/// Nothing here needs to exist: these are the paths the probe would stat, and
/// this is the code that decides which agents it gets to stat at all.
fn both_agents() -> AgentsConfig {
    let runtimes: serde_json::Map<String, serde_json::Value> = [AgentId::Claude, AgentId::Opencode]
        .into_iter()
        .map(|agent| {
            (
                agent.name().to_owned(),
                serde_json::json!({
                    "command": format!("/{}", agent.name()),
                    "args": [],
                    "model": "configured-model",
                    "toolsEnabled": true,
                }),
            )
        })
        .collect();
    serde_json::from_value(serde_json::json!({
        "catalog": "/catalog.json",
        "workspace": "/workspace",
        "selected": "claude",
        "runtimes": runtimes,
    }))
    .unwrap()
}

/// An agent the gateway could not start, with the agent already installed, is
/// not one the probe is asked about.
///
/// `providers` leaves an agent out for either of two reasons and only one of
/// them is a fact about this installation. A command nobody has installed yet
/// is a command somebody can install, and the probe re-stats it on every ask so
/// that setup keeps offering the button. A provider that could not be built
/// with everything already on disk failed on something no install re-asks, and
/// letting the probe stat that command would have it answer `ready` for an
/// agent no conversation can be opened on.
#[test]
fn an_agent_this_run_cannot_start_is_not_one_readiness_answers_for() {
    let config = both_agents();

    let all = launch_files(Some(&config), &HashSet::new());
    assert_eq!(all.len(), 2);
    assert_eq!(
        all[&AgentId::Opencode].command,
        Path::new("/opencode"),
        "an agent nobody has installed is still the probe's to answer for"
    );

    let narrowed = launch_files(Some(&config), &HashSet::from([AgentId::Opencode]));
    assert!(!narrowed.contains_key(&AgentId::Opencode));
    assert!(
        narrowed.contains_key(&AgentId::Claude),
        "and the agents that did build are untouched"
    );
}

/// No agents configured is no agents to answer for, which is not the same
/// answer as an agent that is configured and unstartable — it is setup having
/// nothing to list.
#[test]
fn a_gateway_configured_with_no_agents_answers_for_none() {
    assert!(launch_files(None, &HashSet::new()).is_empty());
}

#[test]
fn agent_catalog_uses_binding_choices_for_each_catalog_model() {
    let catalog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../nessa-sdk/data/models.json");
    let config: AgentsConfig = serde_json::from_value(serde_json::json!({
        "catalog": catalog,
        "workspace": "/workspace",
        "selected": "claude",
        "runtimes": {
            "claude": {"command": "/claude", "model": "claude-sonnet-5", "toolsEnabled": true},
            "codex": {"command": "/codex", "model": "gpt-6-astra", "toolsEnabled": true}
        }
    }))
    .unwrap();
    let offered = agent_catalog(
        &config,
        &HashSet::from([AgentId::Claude, AgentId::Codex]),
        None,
    )
    .unwrap();
    assert_eq!(offered.agents.len(), 2);
    for agent in &offered.agents {
        let selected = agent
            .models
            .iter()
            .find(|model| model.model_id == agent.default_model)
            .unwrap();
        assert_eq!(selected.approval_modes.len(), 3);
        assert_eq!(selected.approval_modes[0].id, WireApprovalMode::Ask);
        assert_eq!(selected.approval_modes[1].id, WireApprovalMode::Auto);
        assert_eq!(selected.approval_modes[2].id, WireApprovalMode::Full);
    }
    assert_eq!(offered.agents[0].agent, "claude");
    assert_eq!(offered.agents[1].agent, "codex");
    for agent in &offered.agents {
        for model in &agent.models {
            let ids: Vec<_> = model
                .approval_modes
                .iter()
                .map(|choice| choice.id)
                .collect();
            let expected = match model.model_id.as_str() {
                "claude-sonnet-5" | "gpt-6-astra" => vec![
                    WireApprovalMode::Ask,
                    WireApprovalMode::Auto,
                    WireApprovalMode::Full,
                ],
                _ if !cfg!(target_os = "macos") => vec![WireApprovalMode::Ask],
                "claude-haiku-4-5-20251001" => vec![WireApprovalMode::Ask, WireApprovalMode::Full],
                _ => vec![
                    WireApprovalMode::Ask,
                    WireApprovalMode::Auto,
                    WireApprovalMode::Full,
                ],
            };
            assert_eq!(ids, expected, "{} {}", agent.agent, model.model_id);
        }
    }
}

/// Composing the gateway's conversations runs the one-shot identity retrofit
/// before the service is built: a conversation saved under the previous
/// restoration identity is current afterwards, and the marker is written.
#[tokio::test]
async fn composing_conversations_moves_a_conversation_saved_under_the_previous_identity() {
    use super::super::agent::tests::{no_credentials, two_agents, NoImages};
    use crate::conversation::application::conversation_session;
    use crate::conversation::domain::{
        Conversation, ConversationApprovalMode, ConversationId, ConversationModelId,
    };
    use crate::conversation::infrastructure::IDENTITY_RETROFIT_MARKER;
    use nessa_sdk::application::agent_execution::sessions::{
        ProviderContext, SavedProviderIdentity, SessionChange, SessionSaveUnit, SessionSnapshot,
        SessionStorage,
    };

    let namespace = tempfile::tempdir().unwrap();
    let (agents, root) = two_agents(namespace.path(), "claude", AgentId::Opencode);
    assert_eq!(root, conversation_root(namespace.path()));
    nessa_local_storage::create_directory(&root).unwrap();
    let built = super::super::agent::providers(
        &agents,
        &root,
        Arc::new(SystemClock),
        Arc::new(NoImages),
        no_credentials(),
        &HashSet::new(),
    )
    .unwrap();
    let claude = &built.providers[&AgentId::Claude];
    let current = claude.provider.identity();
    let previous = claude.previous_identity.clone().unwrap();

    // A conversation saved by an earlier gateway, under the previous identity.
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    LocalConversationStore::open(&root.join("metadata.sqlite3"))
        .unwrap()
        .create(
            Conversation::new(
                id.clone(),
                OrganizationId::new("org").unwrap(),
                nessa_auth::domain::PrincipalId::new("alice").unwrap(),
                "panel".into(),
                "create".into(),
                1,
                AgentId::Claude,
                ConversationModelId::new("configured-model").unwrap(),
                ConversationApprovalMode::Ask,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let session = conversation_session(&id);
    let sessions = || async {
        let storage = Arc::new(RecordStorage::new(root.join("sessions")).unwrap());
        storage.initialize().await.unwrap();
        storage
    };
    {
        let storage = sessions().await;
        let lease = storage.open(session.clone()).await.unwrap();
        let binding = lease.load().await.unwrap().binding().clone();
        lease
            .save_changes(
                binding,
                SessionSnapshot {
                    id: session.clone(),
                    provider: previous.clone(),
                    provider_context: ProviderContext::Absent,
                    invocations: Vec::new(),
                    queue_history: Vec::new(),
                },
                vec![SessionSaveUnit::new(vec![SessionChange::Opened {
                    id: session.clone(),
                    provider: previous,
                    context: ProviderContext::Absent,
                }])
                .unwrap()],
            )
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
    }

    let marker = root.join("retrofit").join(IDENTITY_RETROFIT_MARKER);
    assert!(!marker.exists());
    let composed = conversations(
        &agents,
        &namespace.path().join("gateway"),
        receiver_access(namespace.path(), "policy").unwrap(),
        no_credentials(),
        false,
        RecordId::new("gateway").unwrap(),
    )
    .await
    .unwrap();
    composed.service.shutdown().await.unwrap();
    drop(composed);
    assert!(marker.exists());
    let storage = sessions().await;
    let lease = storage
        .open_existing(session.clone())
        .await
        .unwrap()
        .unwrap();
    let saved = SavedProviderIdentity::load(lease.as_ref(), &session)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(saved.provider(), &current);
}
