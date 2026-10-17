//! An unchanged preset is correlated work without a new selection publication.
use super::*;
use crate::conversation::infrastructure::LocalConversationStore;
use crate::conversation_test_support::access_policy;

#[tokio::test]
async fn an_unchanged_preset_pending_read_keeps_its_publication() {
    let (service, provider, repository, _, _) = mode_fixture();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    let before = service
        .read_at(id.clone(), caller(), ReadOpening::Open)
        .await
        .unwrap();
    let actor = caller();
    let mode = before.0.selection.as_ref().unwrap().approval_mode;
    let intent = ConversationModeRequest {
        conversation_id: id.clone(),
        organization_id: actor.organization_id.clone(),
        request_id: actor.action_id.clone(),
        initiator_principal_id: actor.principal_id.clone(),
        initiator_surface_id: actor.surface_id.clone(),
        prior: mode,
        requested: mode,
        state: ConversationModeRequestState::Pending,
        application: None,
        requested_at_ms: 1,
    };
    repository.begin_mode_change(intent).await.unwrap();
    let guard = service.inner.mode_changes.lock(&id).await;
    for opening in [ReadOpening::Open, ReadOpening::LiveOnly] {
        let pending = tokio::time::timeout(
            Duration::from_secs(5),
            service.read_at(id.clone(), caller(), opening),
        )
        .await
        .expect("a pending no-op read must not wait for its control")
        .unwrap();
        assert_eq!(
            serde_json::to_value(&pending.0).unwrap(),
            serde_json::to_value(&before.0).unwrap()
        );
        assert_eq!(pending.1, before.1);
    }
    drop(guard);
    service.read(id.clone(), caller()).await.unwrap();
    assert!(provider.mode_updates.lock().unwrap().is_empty());
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unchanged_preset_recovers_its_exact_request_without_retiring() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let metadata = Arc::new(
        LocalConversationStore::open(&path)
            .unwrap()
            .with_access_policy(access_policy()),
    );
    let (mut service, provider, _, _, audit) = mode_fixture();
    Arc::get_mut(&mut service.inner).unwrap().metadata = metadata.clone();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    let before = service
        .read_at(id.clone(), caller(), ReadOpening::Open)
        .await
        .unwrap();
    let mode = before.0.selection.as_ref().unwrap().approval_mode;
    let actor = caller();
    let raw = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("CREATE TRIGGER refuse_noop_terminal BEFORE UPDATE OF state ON mode_requests BEGIN SELECT RAISE(ABORT,'controlled terminal refusal'); END;").unwrap();
    assert!(matches!(
        service
            .set_approval_mode(id.clone(), actor.clone(), mode)
            .await,
        Err(ConversationError::ApprovalModeUncertain)
    ));
    let pending = metadata.pending_mode_change(&id).await.unwrap().unwrap();
    assert_eq!(pending.request_id, actor.action_id);
    assert_eq!(pending.initiator_principal_id, actor.principal_id);
    assert_eq!(pending.initiator_surface_id, actor.surface_id);
    assert_eq!(pending.prior, pending.requested);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert!(provider.mode_updates.lock().unwrap().is_empty());
    raw.execute_batch("DROP TRIGGER refuse_noop_terminal;")
        .unwrap();
    let after = service
        .read_at(id.clone(), caller(), ReadOpening::Open)
        .await
        .unwrap();
    assert_eq!(before.0.revision, after.0.revision);
    assert_eq!(before.1, after.1);
    let terminal = metadata
        .mode_change(&id, &actor.action_id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(terminal.state, ConversationModeRequestState::Applied);
    assert_eq!(terminal.request_id, actor.action_id);
    assert_eq!(terminal.initiator_surface_id, actor.surface_id);
    service
        .set_approval_mode(id.clone(), actor.clone(), mode)
        .await
        .unwrap();
    assert!(matches!(
        service
            .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Full)
            .await,
        Err(ConversationError::RequestConflict)
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert!(provider.mode_updates.lock().unwrap().is_empty());
    assert!(audit
        .records
        .lock()
        .unwrap()
        .iter()
        .any(|(request, phase)| request.request_id == actor.action_id
            && request.initiator_surface_id == actor.surface_id
            && *phase == ConversationModeAuditPhase::RecoveryApplied));
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_unchanged_preset_does_not_notify_a_changing_selection() {
    let (service, provider, repository, _, _) = mode_fixture();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    let before = service
        .read_at(id.clone(), caller(), ReadOpening::Open)
        .await
        .unwrap();
    let mode = before.0.selection.as_ref().unwrap().approval_mode;
    let notices = service.live_changes(&id);
    let began = Arc::new(tokio::sync::Notify::new());
    let (release, gate) = oneshot::channel();
    *repository.mode_terminal_gate.lock().unwrap() = Some((began.clone(), gate));
    let changing = {
        let service = service.clone();
        let id = id.clone();
        tokio::spawn(async move { service.set_approval_mode(id, caller(), mode).await })
    };
    tokio::time::timeout(Duration::from_secs(5), began.notified())
        .await
        .unwrap();
    // This fixture pauses terminal readback after its durable write. There is
    // no native operation, and no intent notification should precede settlement.
    assert!(!notices.has_changed().unwrap());
    release.send(()).unwrap();
    changing.await.unwrap().unwrap();
    assert!(provider.mode_updates.lock().unwrap().is_empty());
    service.shutdown().await.unwrap();
}
