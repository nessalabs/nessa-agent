//! Explicit reads retry stopped cleanup without replaying an acknowledged control.
use super::*;
use crate::conversation::application::{
    HistoryGeneration, PreferenceUpdate, ScheduleActor, ScheduleDraft, ScheduledMessageStore,
};
use crate::conversation::domain::ContinuationCause;
use crate::conversation::infrastructure::LocalScheduledMessages;
use crate::conversation_test_support::ProviderFactory;

#[derive(Clone, Copy)]
enum Control {
    Model,
    Preset,
}

struct PreparedStop {
    service: ConversationService,
    provider: Arc<ProviderFactory>,
    id: ConversationId,
    actor: ConversationCaller,
    directory: tempfile::TempDir,
}

#[tokio::test]
async fn acknowledged_model_recovery_retries_the_stopped_owner() {
    stopped_acknowledgement(Control::Model).await;
}

#[tokio::test]
async fn acknowledged_preset_recovery_retries_the_stopped_owner() {
    stopped_acknowledgement(Control::Preset).await;
}

#[tokio::test]
async fn a_recovered_model_change_cancels_the_open_continuation() {
    let prepared = prepare_stopped(Control::Model).await;
    let store = Arc::new(
        LocalScheduledMessages::open(
            &prepared
                .directory
                .path()
                .join("conversations")
                .join("scheduled-chat-messages.sqlite3"),
        )
        .unwrap(),
    );
    let recorded = store
        .record_preference_decision(
            true,
            true,
            &["usage-limit-preference.admin-grant".into()],
            ScheduleActor::system(),
            1,
        )
        .await
        .unwrap();
    assert!(matches!(recorded.update, PreferenceUpdate::Stored(true)));
    let accepted = store
        .accept(
            ScheduleDraft {
                conversation: prepared.id.to_string(),
                causal_execution: "causal".into(),
                execution_id: "exec-1".into(),
                due_at_ms: 9_000_000_000_000,
                cause: ContinuationCause::ProviderLimit,
                actor: ScheduleActor::system(),
                agent: "claude".into(),
                model: "test".into(),
                prompt: "continue".into(),
                organization_id: "org".into(),
                principal_id: "person".into(),
                surface_id: "desktop".into(),
                action_id: "action".into(),
                initiation: "owner".into(),
                grant_id: None,
                policy_revision: None,
                tools_checked: false,
                preference_revision: recorded.revision,
                history: HistoryGeneration {
                    incarnation: "history".into(),
                    position: 1,
                    generation: 0,
                },
            },
            2,
        )
        .await
        .unwrap();
    assert!(accepted.is_some());
    assert_eq!(
        store.status("exec-1").await.unwrap().as_deref(),
        Some("scheduled")
    );
    prepared.service.bind_scheduled_messages(store.clone());
    let recovered = tokio::time::timeout(
        Duration::from_secs(5),
        prepared.service.read(prepared.id.clone(), caller()),
    )
    .await
    .unwrap()
    .expect("recovery reconciles the open continuation");
    assert_eq!(recovered.selection.unwrap().model, "second-model");
    assert_eq!(
        store.status("exec-1").await.unwrap().as_deref(),
        Some("cancelled")
    );
    prepared.service.shutdown().await.unwrap();
}

/// A model change left pending by a failed cleanup does not refuse a later
/// mode change as `model_change_uncertain`: the mode change settles it
/// first, as a send would, and is judged against the model it applied.
#[tokio::test]
async fn a_mode_change_settles_a_stranded_model_change_first() {
    let PreparedStop {
        service,
        provider,
        id,
        actor,
        directory: _directory,
    } = prepare_stopped(Control::Model).await;
    let mode_caller = ConversationCaller {
        action_id: "mode-after-stranded-model".into(),
        ..actor
    };
    // The test catalogue's second model takes Ask only: the refusal names
    // the mode, judged against the model the recovery applied, not the
    // stranded model change.
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        service.set_approval_mode(id.clone(), mode_caller, ConversationApprovalMode::Auto),
    )
    .await
    .unwrap();
    assert!(
        matches!(refused, Err(ConversationError::ApprovalModeUnavailable)),
        "{refused:?}"
    );
    let view = service.read(id.clone(), caller()).await.unwrap();
    assert!(view.model_change.is_none());
    let selection = view.selection.unwrap();
    assert_eq!(selection.model, "second-model");
    assert_eq!(selection.approval_mode, ConversationApprovalMode::Ask);
    assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
    service.shutdown().await.unwrap();
}

async fn stopped_acknowledgement(control: Control) {
    let prepared = prepare_stopped(control).await;
    let PreparedStop {
        service,
        provider,
        id,
        actor,
        directory: _directory,
    } = prepared;
    let recovered =
        tokio::time::timeout(Duration::from_secs(5), service.read(id.clone(), caller()))
            .await
            .unwrap()
            .expect("Open retries cleanup after the fault is released");
    assert!(recovered.model_change.is_none() && recovered.approval_mode_change.is_none());
    let selection = recovered.selection.unwrap();
    match control {
        Control::Model => {
            assert_eq!(selection.model, "second-model");
            assert_eq!(selection.approval_mode, ConversationApprovalMode::Ask);
            service
                .set_model(
                    id.clone(),
                    actor.clone(),
                    ConversationModelId::new("second-model").unwrap(),
                )
                .await
                .unwrap();
            assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
        }
        Control::Preset => {
            assert_eq!(selection.model, "test");
            assert_eq!(selection.approval_mode, ConversationApprovalMode::Auto);
            service
                .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
                .await
                .unwrap();
            assert_eq!(provider.mode_updates.lock().unwrap().len(), 1);
        }
    }
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 2);
    let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
    let snapshot = live.agent.session_manager().snapshot().await.unwrap();
    let retained = snapshot.current_configuration.unwrap();
    assert_eq!(retained.actor().principal_id(), actor.principal_id.as_str());
    assert_eq!(retained.actor().surface_id(), actor.surface_id);
    assert_eq!(retained.actor().request_id(), actor.action_id);
    drop(live);
    service.shutdown().await.unwrap();
}

async fn prepare_stopped(control: Control) -> PreparedStop {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let metadata = Arc::new(
        LocalConversationStore::open(&path)
            .unwrap()
            .with_access_policy(access_policy()),
    );
    let (mut service, provider, _, _, _) = mode_fixture();
    Arc::get_mut(&mut service.inner).unwrap().metadata = metadata.clone();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service.read(id.clone(), caller()).await.unwrap();
    let raw = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    let table = match control {
        Control::Model => "model_requests",
        Control::Preset => "mode_requests",
    };
    raw.execute_batch(&format!("CREATE TRIGGER refuse_selection_terminal BEFORE UPDATE OF state ON {table} BEGIN SELECT RAISE(ABORT,'controlled terminal refusal'); END;")).unwrap();
    *provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    let actor = caller();
    match control {
        Control::Model => {
            assert!(matches!(
                service
                    .set_model(
                        id.clone(),
                        actor.clone(),
                        ConversationModelId::new("second-model").unwrap()
                    )
                    .await,
                Err(ConversationError::ModelChangeUncertain)
            ));
            assert_eq!(
                provider.close_calls.load(Ordering::SeqCst),
                0,
                "known pending metadata alone does not retire a healthy model owner"
            );
            assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
        }
        Control::Preset => {
            assert!(matches!(
                service
                    .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
                    .await,
                Err(ConversationError::ApprovalModeUncertain)
            ));
            assert_eq!(provider.mode_updates.lock().unwrap().len(), 1);
        }
    }
    assert!(service.close(id.clone(), caller()).await.is_err());
    raw.execute_batch("DROP TRIGGER refuse_selection_terminal;")
        .unwrap();
    match control {
        Control::Model => assert!(matches!(
            service
                .set_model(
                    id.clone(),
                    actor.clone(),
                    ConversationModelId::new("second-model").unwrap(),
                )
                .await,
            Err(ConversationError::ModelChangeUncertain)
        )),
        Control::Preset => assert!(matches!(
            service
                .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto,)
                .await,
            Err(ConversationError::ApprovalModeUncertain)
        )),
    }
    for opening in [ReadOpening::LiveOnly, ReadOpening::Open] {
        assert!(matches!(
            service.read_at(id.clone(), caller(), opening).await,
            Err(ConversationError::Agent(AgentError::Closed))
        ));
    }
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    match control {
        Control::Model => assert!(metadata.pending_model_change(&id).await.unwrap().is_some()),
        Control::Preset => assert!(metadata.pending_mode_change(&id).await.unwrap().is_some()),
    }
    *provider.close_failure.lock().unwrap() = None;
    PreparedStop {
        service,
        provider,
        id,
        actor,
        directory,
    }
}
