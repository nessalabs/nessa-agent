//! Live model controls preserve the native turn and commit a single selection.
use super::*;
use crate::conversation::application::ConversationModelRequestState;
use crate::conversation::infrastructure::LocalConversationStore;
use crate::conversation_test_support::{access_policy, mode_fixture};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::view::{
    ConversationModelChangeEffect, ConversationModelChangeStatus,
};
use nessa_sdk::infrastructure::session_storage::InMemoryStorage;
use std::time::Duration;

fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "desktop".into(),
        action_id: Uuid::new_v4().to_string(),
    }
}

// Read the current public keyed-content contract, including the full review's
// turn/request identities. The view's key remains its immutable identity while
// a model or preset control is pending.
async fn listed_model_review(
    service: &ConversationService,
    id: &ConversationId,
) -> serde_json::Value {
    let key = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let view = service.read(id.clone(), caller()).await.unwrap();
            if let Some(entry) = view.permissions.first() {
                break entry.review.clone();
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the native review is listed");
    let (bytes, total) = service
        .permission_review(id.clone(), caller(), key.clone(), 0, 32_768)
        .await
        .unwrap();
    assert_eq!(
        bytes.len() as u64,
        total,
        "the fixture review fits one content range"
    );
    let review: serde_json::Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(review["review"].as_str(), Some(key.as_str()));
    review
}

#[tokio::test]
async fn model_choice_during_a_turn_keeps_native_session_and_exact_attributed_history() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("conversations");
    nessa_local_storage::create_directory(&private).unwrap();
    let metadata = Arc::new(
        LocalConversationStore::open(&private.join("metadata.sqlite3"))
            .unwrap()
            .with_access_policy(access_policy()),
    );
    let storage = Arc::new(InMemoryStorage::new());
    let (mut service, provider, _, _, _) = mode_fixture();
    let inner = Arc::get_mut(&mut service.inner).expect("composition precedes any live work");
    inner.metadata = metadata.clone();
    inner.storage = storage.clone();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    let before = service.read(id.clone(), caller()).await.unwrap();
    let (finish, gate) = tokio::sync::oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller(),
            "ongoing".into(),
            SubmittedMessage {
                text: "keep running".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        provider.execution_started.notified(),
    )
    .await
    .expect("turn began");
    let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
    let native = live
        .agent
        .session_manager()
        .snapshot()
        .await
        .unwrap()
        .provider_context
        .clone();
    let actor = caller();
    let requested = ConversationModelId::new("second-model").unwrap();
    let selected = tokio::time::timeout(
        Duration::from_secs(5),
        service.set_model(id.clone(), actor.clone(), requested.clone()),
    )
    .await
    .expect("model control cannot wait for the turn to finish")
    .unwrap();
    assert_eq!(selected.model, requested);
    assert_eq!(selected.effect, ConversationModelChangeEffect::NextTurn);
    assert_eq!(
        live.agent.active_execution_id().unwrap().as_str(),
        "ongoing"
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    let snapshot = live.agent.session_manager().snapshot().await.unwrap();
    assert_eq!(snapshot.provider_context, native);
    let record = snapshot.model_change.as_ref().unwrap();
    assert_eq!(record.actor(), &actor.actor().unwrap());
    assert_eq!(record.change().before().model_id(), "test");
    assert_eq!(record.change().after().model_id(), "second-model");
    let current = service.read(id.clone(), caller()).await.unwrap();
    assert_eq!(current.selection.as_ref().unwrap().model, "second-model");
    assert_ne!(current.revision, before.revision);
    assert!(current.model_change.is_none());
    assert_eq!(
        current.model_change_effect,
        Some(ConversationModelChangeEffect::NextTurn)
    );
    assert_eq!(
        service
            .set_model(id.clone(), actor, requested)
            .await
            .unwrap(),
        selected
    );
    assert_eq!(
        provider.model_updates.lock().unwrap().len(),
        1,
        "retry must not send a second control"
    );
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if live.agent.active_execution_id().is_none() {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("original turn finishes");
    service
        .submit(
            id.clone(),
            caller(),
            "after-model".into(),
            SubmittedMessage {
                text: "uses the acknowledged model".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider.output_reservations.lock().unwrap().len() == 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("next turn reaches the same native owner");
    assert_eq!(
        *provider.output_reservations.lock().unwrap(),
        vec![4096, 2048]
    );
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_pending_model_control_keeps_the_original_review_answerable_after_caller_loss() {
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
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service.read(id.clone(), caller()).await.unwrap();
    provider.request_permission.store(1, Ordering::SeqCst);
    provider.answer_succeeds.store(true, Ordering::SeqCst);
    let (finish_turn, turn_gate) = tokio::sync::oneshot::channel();
    *provider.permission_gate.lock().unwrap() = Some(turn_gate);
    service
        .submit(
            id.clone(),
            caller(),
            "review-turn".into(),
            SubmittedMessage {
                text: "needs approval".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let review = listed_model_review(&service, &id).await;
    let (ack, model_gate) = tokio::sync::oneshot::channel();
    *provider.model_gate.lock().unwrap() = Some(model_gate);
    let actor = caller();
    let changing = {
        let service = service.clone();
        let id = id.clone();
        let actor = actor.clone();
        tokio::spawn(async move {
            service
                .set_model(id, actor, ConversationModelId::new("second-model").unwrap())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.model_started.notified())
        .await
        .expect("model RPC entered");
    let during = tokio::time::timeout(Duration::from_secs(5), service.read(id.clone(), caller()))
        .await
        .expect("read must not wait behind the native control")
        .unwrap();
    assert_eq!(during.selection.as_ref().unwrap().model, "test");
    assert_eq!(
        during.model_change.as_ref().unwrap().request_id,
        actor.action_id
    );
    assert_eq!(during.permissions.len(), 1);
    assert_eq!(
        during.permissions[0].review,
        review["review"].as_str().unwrap()
    );
    changing.abort();
    assert!(changing.await.unwrap_err().is_cancelled());
    tokio::time::timeout(
        Duration::from_secs(5),
        service.answer(
            id.clone(),
            caller(),
            review["executionId"].as_str().unwrap().to_owned(),
            review["permissionId"].as_str().unwrap().to_owned(),
            review["review"].as_str().unwrap().to_owned(),
            "allow".into(),
            PermissionCaller::Owner,
        ),
    )
    .await
    .expect("the original review remains answerable while setting RPC waits")
    .unwrap();
    ack.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if metadata
                .model_change(&id, &actor.action_id)
                .await
                .unwrap()
                .is_some_and(|request| request.state == ConversationModelRequestState::Applied)
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("owned setting task settles despite caller loss");
    assert_eq!(provider.permission_answers.lock().unwrap().len(), 1);
    assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    finish_turn.send(()).unwrap();
    let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.active_execution_id().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("reviewed turn finishes");
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_failed_metadata_commit_recovers_from_exact_sdk_evidence_without_replaying_or_cancelling()
{
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
    let (finish, gate) = tokio::sync::oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller(),
            "held-across-storage-failure".into(),
            SubmittedMessage {
                text: "ongoing".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        provider.execution_started.notified(),
    )
    .await
    .unwrap();
    let raw = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    raw.execute_batch("CREATE TRIGGER refuse_model_terminal BEFORE UPDATE OF state ON model_requests BEGIN SELECT RAISE(ABORT,'controlled terminal failure'); END;").unwrap();
    let actor = caller();
    let target = ConversationModelId::new("second-model").unwrap();
    assert!(matches!(
        tokio::time::timeout(
            Duration::from_secs(5),
            service.set_model(id.clone(), actor.clone(), target.clone())
        )
        .await
        .unwrap(),
        Err(ConversationError::ModelChangeUncertain)
    ));
    assert_eq!(
        provider.close_calls.load(Ordering::SeqCst),
        0,
        "a proven pending metadata commit need not cancel active native work"
    );
    assert_eq!(
        ConversationRepository::load(metadata.as_ref(), &id)
            .await
            .unwrap()
            .unwrap()
            .model()
            .as_str(),
        "test"
    );
    let pending = metadata.pending_model_change(&id).await.unwrap().unwrap();
    assert_eq!(pending.request_id, actor.action_id);
    assert_eq!(pending.initiator_principal_id, actor.principal_id);
    assert_eq!(pending.initiator_surface_id, actor.surface_id);
    let during = service
        .read_at(id.clone(), caller(), ReadOpening::LiveOnly)
        .await
        .unwrap()
        .0;
    assert!(matches!(
        during.model_change.unwrap().status,
        ConversationModelChangeStatus::RecoveryRequired
    ));
    assert_eq!(during.selection.unwrap().model, "test");
    raw.execute_batch("DROP TRIGGER refuse_model_terminal;")
        .unwrap();
    let recovered = service.read(id.clone(), caller()).await.unwrap();
    assert!(recovered.model_change.is_none());
    assert_eq!(recovered.selection.as_ref().unwrap().model, "second-model");
    assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(
        service
            .set_model(id.clone(), actor, target)
            .await
            .unwrap()
            .model
            .as_str(),
        "second-model"
    );
    let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
    assert_eq!(
        live.agent.active_execution_id().unwrap().as_str(),
        "held-across-storage-failure"
    );
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.active_execution_id().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_active_permission_preset_keeps_the_original_review_and_native_turn() {
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
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service.read(id.clone(), caller()).await.unwrap();
    provider.request_permission.store(1, Ordering::SeqCst);
    provider.answer_succeeds.store(true, Ordering::SeqCst);
    let (finish, gate) = tokio::sync::oneshot::channel();
    *provider.permission_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller(),
            "permission-preset-turn".into(),
            SubmittedMessage {
                text: "wait for review".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let review = listed_model_review(&service, &id).await;
    let before_noop = service
        .read_at(id.clone(), caller(), ReadOpening::LiveOnly)
        .await
        .unwrap();
    let noop_actor = caller();
    service
        .set_approval_mode(
            id.clone(),
            noop_actor.clone(),
            ConversationApprovalMode::Ask,
        )
        .await
        .unwrap();
    let after_noop = service
        .read_at(id.clone(), caller(), ReadOpening::LiveOnly)
        .await
        .unwrap();
    assert_eq!(before_noop.0.revision, after_noop.0.revision);
    assert_eq!(before_noop.1, after_noop.1);
    assert!(provider.mode_updates.lock().unwrap().is_empty());
    assert_eq!(
        after_noop.0.permissions[0].review,
        review["review"].as_str().unwrap()
    );
    assert_eq!(
        metadata
            .mode_change(&id, &noop_actor.action_id)
            .await
            .unwrap()
            .unwrap()
            .state,
        ConversationModeRequestState::Applied
    );
    let (ack, mode_gate) = tokio::sync::oneshot::channel();
    *provider.mode_gate.lock().unwrap() = Some(mode_gate);
    let actor = caller();
    let changing = {
        let service = service.clone();
        let id = id.clone();
        let actor = actor.clone();
        tokio::spawn(async move {
            service
                .set_approval_mode(id, actor, ConversationApprovalMode::Full)
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.mode_started.notified())
        .await
        .expect("the live native preset RPC starts before the turn finishes");
    let during = tokio::time::timeout(Duration::from_secs(5), service.read(id.clone(), caller()))
        .await
        .expect("pending control does not hide a review behind its wait")
        .unwrap();
    assert_eq!(
        during.selection.unwrap().approval_mode,
        ConversationApprovalMode::Ask
    );
    assert_eq!(
        during.approval_mode_change.unwrap().request_id,
        actor.action_id
    );
    assert_eq!(during.permissions.len(), 1);
    assert_eq!(
        during.permissions[0].review,
        review["review"].as_str().unwrap()
    );
    assert!(
        provider.permission_answers.lock().unwrap().is_empty(),
        "changing a preset is not an answer to the existing review"
    );
    service
        .answer(
            id.clone(),
            caller(),
            review["executionId"].as_str().unwrap().to_owned(),
            review["permissionId"].as_str().unwrap().to_owned(),
            review["review"].as_str().unwrap().to_owned(),
            "allow".into(),
            PermissionCaller::Owner,
        )
        .await
        .unwrap();
    ack.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), changing)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
        ConversationApprovalMode::Full
    );
    let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
    assert_eq!(
        live.agent.active_execution_id().unwrap().as_str(),
        "permission-preset-turn"
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    let snapshot = live.agent.session_manager().snapshot().await.unwrap();
    assert_eq!(
        snapshot.current_configuration.unwrap().actor(),
        &actor.actor().unwrap()
    );
    assert_eq!(
        ConversationRepository::load(metadata.as_ref(), &id)
            .await
            .unwrap()
            .unwrap()
            .approval_mode(),
        ConversationApprovalMode::Full
    );
    finish.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.active_execution_id().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_model_that_reconciles_an_unsupported_old_preset_commits_the_native_acknowledgement() {
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
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service.read(id.clone(), caller()).await.unwrap();
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
        .await
        .unwrap();
    let updates = provider.mode_updates.lock().unwrap().len();
    let selected = service
        .set_model(
            id.clone(),
            caller(),
            ConversationModelId::new("second-model").unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(selected.approval_mode, ConversationApprovalMode::Ask);
    let record = ConversationRepository::load(metadata.as_ref(), &id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(record.model().as_str(), "second-model");
    assert_eq!(record.approval_mode(), ConversationApprovalMode::Ask);
    assert_eq!(
        provider.mode_updates.lock().unwrap().len(),
        updates,
        "provisional Ask factory construction must not send a separate native preset control"
    );
    assert_eq!(provider.model_updates.lock().unwrap().len(), 1);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn a_foreign_pending_initiator_cannot_authorize_retirement_or_publication() {
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
    let (finish, gate) = tokio::sync::oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller(),
            "owned-turn".into(),
            SubmittedMessage {
                text: "keep running".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(5),
        provider.execution_started.notified(),
    )
    .await
    .unwrap();
    let raw = nessa_local_database::rusqlite::Connection::open(&path).unwrap();
    raw.execute("INSERT INTO model_requests(conversation_id,request_id,initiator,surface,prior_model,prior_mode,requested_model,state,requested_at_ms,prior_provider,prior_context,requested_provider,requested_context) VALUES(?1,'foreign-request','other-person','other-window','test','ask','second-model','pending',1,'gateway-test','test','gateway-test','test')",[id.to_string()]).unwrap();
    assert!(matches!(
        service.read(id.clone(), caller()).await,
        Err(ConversationError::ModelChangeUncertain)
    ));
    assert!(matches!(
        service
            .set_model(
                id.clone(),
                caller(),
                ConversationModelId::new("second-model").unwrap()
            )
            .await,
        Err(ConversationError::ModelChangeUncertain)
    ));
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert!(provider.model_updates.lock().unwrap().is_empty());
    raw.execute(
        "DELETE FROM model_requests WHERE conversation_id=?1",
        [id.to_string()],
    )
    .unwrap();
    finish.send(()).unwrap();
    let live = service.live_now(&id, &caller()).await.unwrap().unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.active_execution_id().is_some() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    service.shutdown().await.unwrap();
}

#[path = "model_changes/adapted_environment.rs"]
mod adapted_environment;

#[path = "model_changes/stopped_recovery.rs"]
mod stopped_recovery;

#[path = "model_changes/retained_identity.rs"]
mod retained_identity;
