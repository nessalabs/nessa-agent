//! Explicit reads retry stopped cleanup without replaying an acknowledged control.
use super::*;

#[derive(Clone, Copy)]
enum Control {
    Model,
    Preset,
}

#[tokio::test]
async fn acknowledged_model_recovery_retries_the_stopped_owner() {
    stopped_acknowledgement(Control::Model).await;
}

#[tokio::test]
async fn acknowledged_preset_recovery_retries_the_stopped_owner() {
    stopped_acknowledgement(Control::Preset).await;
}

async fn stopped_acknowledgement(control: Control) {
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
