//! Retries and restored model effects use their retained authoritative records.
use super::*;

#[tokio::test]
async fn a_submission_retry_keeps_its_original_budget_after_a_model_change() {
    for mode in [SubmissionMode::Queue, SubmissionMode::Steer] {
        let directory = tempfile::tempdir().unwrap();
        let private = directory.path().join("conversations");
        nessa_local_storage::create_directory(&private).unwrap();
        let metadata = Arc::new(
            LocalConversationStore::open(&private.join("metadata.sqlite3"))
                .unwrap()
                .with_access_policy(access_policy()),
        );
        let (mut service, provider, _, _, _) = mode_fixture();
        Arc::get_mut(&mut service.inner).unwrap().metadata = metadata;
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(id.clone(), caller(), RequestedConversation::default())
            .await
            .unwrap();
        let actor = caller();
        let message = SubmittedMessage {
            text: "recover my delivery".into(),
            ..SubmittedMessage::default()
        };
        let first = service
            .submit(
                id.clone(),
                actor.clone(),
                "saved".into(),
                message.clone(),
                mode,
            )
            .await
            .unwrap();
        let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while !live.agent.idle_for_approval_change().await {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let before = live
            .agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations[0]
            .request
            .clone();
        service
            .set_model(
                id.clone(),
                caller(),
                ConversationModelId::new("second-model").unwrap(),
            )
            .await
            .unwrap();
        let retried = service
            .submit(
                id.clone(),
                actor.clone(),
                "saved".into(),
                message.clone(),
                mode,
            )
            .await
            .unwrap();
        assert_eq!(retried.execution_id, first.execution_id);
        let after = live
            .agent
            .session_manager()
            .snapshot()
            .await
            .unwrap()
            .invocations[0]
            .request
            .clone();
        assert_eq!(after, before);
        for (retry_actor, retry_message, mode) in [
            (
                actor.clone(),
                SubmittedMessage {
                    text: "different".into(),
                    ..SubmittedMessage::default()
                },
                mode,
            ),
            (
                ConversationCaller {
                    surface_id: "other-window".into(),
                    ..actor.clone()
                },
                message.clone(),
                mode,
            ),
            (
                actor,
                message.clone(),
                if matches!(mode, SubmissionMode::Queue) {
                    SubmissionMode::Steer
                } else {
                    SubmissionMode::Queue
                },
            ),
        ] {
            assert!(matches!(
                service
                    .submit(id.clone(), retry_actor, "saved".into(), retry_message, mode)
                    .await,
                Err(ConversationError::Agent(AgentError::SubmissionConflict))
            ));
        }
        service
            .submit(id.clone(), caller(), "fresh".into(), message, mode)
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(5), async {
            while provider.output_reservations.lock().unwrap().len() != 2 {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert_eq!(
            *provider.output_reservations.lock().unwrap(),
            vec![4096, 2048]
        );
        let fresh_snapshot = live.agent.session_manager().snapshot().await.unwrap();
        let fresh = &fresh_snapshot
            .invocations
            .iter()
            .find(|record| record.request.execution_id.as_str() == "fresh")
            .unwrap()
            .request;
        assert_eq!(fresh.reserved_output_tokens, 2048);
        assert_eq!(
            fresh.estimated_input_tokens,
            u64::from(live.agent.capabilities().limits().max_context_window() - 2048)
        );
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn a_confirmed_model_effect_survives_detached_reads_and_reopening() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let metadata = Arc::new(
        LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap()
            .with_access_policy(access_policy()),
    );
    let (mut service, provider, _, _, _) = mode_fixture();
    Arc::get_mut(&mut service.inner).unwrap().metadata = metadata.clone();
    let storage = service.inner.storage.clone();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service.read(id.clone(), caller()).await.unwrap();
    let selected = service
        .set_model(
            id.clone(),
            caller(),
            ConversationModelId::new("second-model").unwrap(),
        )
        .await
        .unwrap();
    service.close(id.clone(), caller()).await.unwrap();
    let detached = service
        .read_at(id.clone(), caller(), ReadOpening::LiveOnly)
        .await
        .unwrap()
        .0;
    assert_eq!(detached.selection.unwrap().model, "second-model");
    assert_eq!(detached.model_change_effect, Some(selected.effect));
    let reopened = service.read(id.clone(), caller()).await.unwrap();
    assert_eq!(reopened.model_change_effect, Some(selected.effect));
    assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
    service
        .set_model(
            id.clone(),
            caller(),
            ConversationModelId::new("test").unwrap(),
        )
        .await
        .unwrap();
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Full)
        .await
        .unwrap();
    service.close(id.clone(), caller()).await.unwrap();
    let with_later_preset = service
        .read_at(id.clone(), caller(), ReadOpening::LiveOnly)
        .await
        .unwrap()
        .0;
    assert_eq!(
        with_later_preset.selection.unwrap().approval_mode,
        ConversationApprovalMode::Full
    );
    assert_eq!(with_later_preset.model_change_effect, Some(selected.effect));
    service.shutdown().await.unwrap();
    let (mut restarted, restarted_provider, _, _, _) = mode_fixture();
    let inner = Arc::get_mut(&mut restarted.inner).unwrap();
    inner.metadata = metadata;
    inner.storage = storage;
    let restored = restarted.read(id, caller()).await.unwrap();
    assert_eq!(
        restored.selection.unwrap().approval_mode,
        ConversationApprovalMode::Full
    );
    assert_eq!(restored.model_change_effect, Some(selected.effect));
    assert!(restarted_provider.model_updates.lock().unwrap().is_empty());
    restarted.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_model_effect_requires_the_matching_committed_request() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let path = private.join("metadata.sqlite3");
    let metadata = Arc::new(
        LocalConversationStore::open(&path)
            .unwrap()
            .with_access_policy(access_policy()),
    );
    let (mut service, _, _, _, _) = mode_fixture();
    Arc::get_mut(&mut service.inner).unwrap().metadata = metadata;
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service.read(id.clone(), caller()).await.unwrap();
    let actor = caller();
    service
        .set_model(
            id.clone(),
            actor.clone(),
            ConversationModelId::new("second-model").unwrap(),
        )
        .await
        .unwrap();
    service.close(id.clone(), caller()).await.unwrap();
    let raw = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    for mutation in [
        "UPDATE model_requests SET initiator='other-person'",
        "UPDATE model_requests SET surface='other-window'",
        "UPDATE model_requests SET request_id='another-request'",
        "UPDATE model_requests SET prior_context='another-context'",
        "UPDATE model_requests SET requested_context='another-context'",
        "UPDATE model_requests SET applied_mode='auto'",
        "UPDATE model_requests SET effect='subsequent_responses'",
        "UPDATE model_requests SET state='not_applied',application='refused',applied_mode=NULL,effect=NULL",
        "UPDATE conversations SET model='test'",
        "UPDATE model_requests SET state='pending'",
    ] {
        raw.execute("UPDATE model_requests SET initiator='person',surface='desktop',request_id=?1,prior_context='test',requested_context='test',state='applied',application='applied',applied_mode='ask',effect='next_turn'", [&actor.action_id]).unwrap();
        raw.execute("UPDATE conversations SET model='second-model'", []).unwrap();
        raw.execute(mutation, []).unwrap();
        if mutation == "UPDATE model_requests SET state='pending'" {
            raw.execute("UPDATE conversations SET model='test'", []).unwrap();
        }
        let detached = service.read_at(id.clone(), caller(), ReadOpening::LiveOnly).await.unwrap().0;
        assert_eq!(detached.model_change_effect, None, "{mutation}");
    }
    service.shutdown().await.unwrap();
}
