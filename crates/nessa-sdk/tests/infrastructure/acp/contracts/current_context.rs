//! Current-context admission across agent switches and fingerprint changes.
//! Each case uses a real Agent, record storage and a fresh local ACP process on
//! each attachment. The native fixture saves context only after a prompt.
use super::support::*;
use crate::{
    application::agent_execution::sessions::{HandoffIntent, SessionStorage},
    domain::{
        agent_execution::sessions::SessionId,
        effective_capabilities::value_objects::EffectiveCapabilities,
    },
    infrastructure::session_storage::{RecordStorage, RuntimeMessageCommitClock},
};
use std::path::Path;

/// The prior agent is a separate provider namespace with the same local ACP
/// fixture transport. Its actual invocations are retained across the switch.
struct PriorProvider(ClaudeAcpProvider);
impl AgentProvider for PriorProvider {
    fn identity(&self) -> ProviderIdentity {
        let identity = self.0.identity();
        ProviderIdentity::new("prior-agent", identity.model_id(), identity.context()).unwrap()
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        self.0.capabilities()
    }
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.0.harness_host_identity()
    }
    fn open(&self, request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        self.0.open(request)
    }
}

fn provider(config: AcpConfig, model: &ModelMetadata) -> ClaudeAcpProvider {
    ClaudeAcpProvider::new(
        config,
        model,
        TokenLimits::new(900, 100).unwrap(),
        Arc::new(RecordingAudit::default()),
    )
    .unwrap()
}

async fn open(
    provider: Arc<dyn AgentProvider>,
    storage_path: &Path,
    id: &SessionId,
    switching: bool,
) -> Result<Agent, AgentError> {
    // Reconstruct the adapter as well as the native process: a previous
    // in-memory snapshot cannot provide the admission watermark on reopen.
    let storage = Arc::new(RecordStorage::new(storage_path).unwrap());
    let mut manager = SessionManager::open(
        Some(id.clone()),
        storage,
        Arc::new(RuntimeMessageCommitClock::new()),
    )
    .await
    .unwrap();
    if switching {
        manager.expect_handoff(HandoffIntent::Switch {
            actor: ActionContext::new("fixture-host", "fixture-tests", "switch-current-context")
                .unwrap(),
            at_ms: 5,
        });
    }
    attached_agent(provider, manager).await
}

fn count(root: &Path, name: &str) -> usize {
    std::fs::read_to_string(root.join(name))
        .unwrap()
        .parse()
        .unwrap()
}

async fn current_context_survives_cold_reopen(switching: bool, used: bool) {
    let _process_slot = process_test_slot().await;
    let (root, mut config, model) = test_acp_configuration("uninvoked-context", 16);
    config.environment.insert(
        "CLAUDE_CONFIG_DIR".into(),
        "/fixture/current-context-a".into(),
    );
    let original = provider(config.clone(), &model);
    let prior: Arc<dyn AgentProvider> = if switching {
        Arc::new(PriorProvider(original))
    } else {
        Arc::new(original)
    };
    let storage_root = tempfile::tempdir().unwrap();
    let storage_path = storage_root.path().join("sessions");
    let id = SessionId::new("current-context-admission").unwrap();
    let first = open(prior.clone(), &storage_path, &id, false)
        .await
        .unwrap();
    assert_eq!(
        first
            .invoke(
                prompt("old-context-input"),
                Submitter::owner(close_action())
            )
            .await,
        Ok(ExecutionOutcome::Completed)
    );
    let old = first.session_manager().snapshot().await.unwrap();
    first.close(close_action()).await.unwrap();
    drop(first);
    assert_eq!(count(root.path(), "prompt-count"), 1);

    if !switching {
        config.environment.insert(
            "CLAUDE_CONFIG_DIR".into(),
            "/fixture/current-context-b".into(),
        );
    }
    let current = Arc::new(provider(config, &model));
    assert_ne!(prior.identity(), current.identity());
    let second = open(current.clone(), &storage_path, &id, switching)
        .await
        .unwrap();
    let fresh = second.session_manager().snapshot().await.unwrap();
    assert_eq!(fresh.invocations, old.invocations);
    assert_eq!(fresh.provider, current.identity());
    assert_ne!(
        fresh.provider_context.recorded(),
        old.provider_context.recorded()
    );
    assert_eq!(fresh.provider_context.admission_watermark(), Some(1));
    assert!(!fresh
        .provider_context
        .has_admitted_work(fresh.invocations.len()));
    assert_eq!(fresh.handoffs.len(), usize::from(switching));
    assert_eq!(count(root.path(), "new-session-count"), 2);
    assert_eq!(count(root.path(), "prompt-count"), 1);

    if used {
        assert_eq!(
            second
                .invoke(
                    prompt("new-context-input"),
                    Submitter::owner(close_action())
                )
                .await,
            Ok(ExecutionOutcome::Completed)
        );
    }
    let before_restart = second.session_manager().snapshot().await.unwrap();
    second.close(close_action()).await.unwrap();
    drop(second);
    let third = open(current.clone(), &storage_path, &id, false)
        .await
        .unwrap();
    let restored = third.session_manager().snapshot().await.unwrap();
    assert_eq!(restored.invocations, before_restart.invocations);
    assert_eq!(restored.handoffs, before_restart.handoffs);
    assert_eq!(restored.invocations[0], old.invocations[0]);
    assert_eq!(restored.invocations.len(), 1 + usize::from(used));
    assert_eq!(restored.provider_context.admission_watermark(), Some(1));
    assert_eq!(
        restored
            .provider_context
            .has_admitted_work(restored.invocations.len()),
        used
    );
    assert_eq!(count(root.path(), "prompt-count"), 1 + usize::from(used));
    if used {
        assert_eq!(restored.provider_context, before_restart.provider_context);
        assert_eq!(count(root.path(), "new-session-count"), 2);
        assert_eq!(
            std::fs::read_to_string(root.path().join("resume-observed")).unwrap(),
            restored.provider_context.recorded().unwrap().as_str()
        );
        let native: serde_json::Value =
            serde_json::from_slice(&std::fs::read(root.path().join("saved-session")).unwrap())
                .unwrap();
        assert_eq!(native["history"].as_array().unwrap().len(), 1);
    } else {
        assert_ne!(
            restored.provider_context.recorded(),
            before_restart.provider_context.recorded()
        );
        assert_eq!(count(root.path(), "new-session-count"), 3);
        assert!(!root.path().join("resume-observed").exists());
    }
    third.close(close_action()).await.unwrap();
    drop(third);

    if used {
        // A failed native resume does not authorize context replacement, and
        // opening the adapter anew must retain the original invocation evidence.
        std::fs::remove_file(root.path().join("saved-session")).unwrap();
        let error = open(current, &storage_path, &id, false)
            .await
            .err()
            .expect("missing used context must fail");
        assert!(matches!(error, AgentError::Provider { code: -32002, .. }));
        let storage = RecordStorage::new(&storage_path).unwrap();
        let committed = storage.read_committed(id).await.unwrap().unwrap();
        let saved = committed.snapshot().unwrap();
        assert_eq!(saved.provider_context, restored.provider_context);
        assert_eq!(saved.invocations, restored.invocations);
        assert_eq!(saved.handoffs, restored.handoffs);
        assert_eq!(count(root.path(), "new-session-count"), 2);
        assert_eq!(count(root.path(), "prompt-count"), 2);
    }
}

#[tokio::test]
async fn unused_switched_claude_context_reopens_without_replaying_old_history() {
    current_context_survives_cold_reopen(true, false).await;
}

#[tokio::test]
async fn used_switched_claude_context_requires_exact_native_history_after_restart() {
    current_context_survives_cold_reopen(true, true).await;
}

#[tokio::test]
async fn unused_fingerprint_changed_claude_context_reopens_without_replaying_old_history() {
    current_context_survives_cold_reopen(false, false).await;
}

#[tokio::test]
async fn used_fingerprint_changed_claude_context_requires_exact_native_history_after_restart() {
    current_context_survives_cold_reopen(false, true).await;
}
