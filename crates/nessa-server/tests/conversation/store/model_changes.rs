//! A model journal cannot commit a different request, actor, or half selection.
use super::{new_id, opened, owned, raw, LocalConversationStore};
use crate::conversation::application::{
    ConversationError, ConversationModeRequest, ConversationModeRequestState,
    ConversationModelApplication, ConversationModelRequest, ConversationModelRequestState,
    ConversationModelSelection, ConversationRepository,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::{
    ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_protocol::conversation::view::ConversationModelChangeEffect;
use nessa_sdk::application::agent_execution::providers::ProviderIdentity;

fn intent(id: ConversationId) -> ConversationModelRequest {
    ConversationModelRequest {
        conversation_id: id,
        organization_id: OrganizationId::new("org").unwrap(),
        request_id: "model-choice".into(),
        initiator_principal_id: PrincipalId::new("alice").unwrap(),
        initiator_surface_id: "desktop".into(),
        prior_identity: ProviderIdentity::new("provider", "test-model", "same-context").unwrap(),
        requested_identity: ProviderIdentity::new("provider", "second-model", "same-context")
            .unwrap(),
        prior_model: ConversationModelId::new("test-model").unwrap(),
        prior_approval_mode: ConversationApprovalMode::Ask,
        requested_model: ConversationModelId::new("second-model").unwrap(),
        state: ConversationModelRequestState::Pending,
        application: None,
        requested_at_ms: 20,
    }
}
fn applied(request: &ConversationModelRequest) -> ConversationModelApplication {
    ConversationModelApplication::Applied(ConversationModelSelection {
        model: request.requested_model.clone(),
        approval_mode: ConversationApprovalMode::Auto,
        effect: ConversationModelChangeEffect::NextTurn,
    })
}

#[tokio::test]
async fn acknowledged_model_and_native_mode_commit_together_and_survive_reopen() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let request = intent(id.clone());
    assert_eq!(
        opened
            .store
            .begin_model_change(request.clone())
            .await
            .unwrap(),
        request
    );
    assert_eq!(
        opened.store.load(&id).await.unwrap().unwrap().model(),
        &request.prior_model
    );
    assert!(matches!(
        opened
            .store
            .finish_model_change(
                &id,
                &request.request_id,
                ConversationModelRequestState::Applied
            )
            .await,
        Err(ConversationError::ModelChangeUncertain)
    ));
    let observed = opened
        .store
        .observe_model_application(&id, &request.request_id, applied(&request))
        .await
        .unwrap();
    assert_eq!(
        opened
            .store
            .load(&id)
            .await
            .unwrap()
            .unwrap()
            .approval_mode(),
        ConversationApprovalMode::Ask
    );
    let expected = ConversationModelRequest {
        state: ConversationModelRequestState::Applied,
        ..observed
    };
    assert_eq!(
        opened
            .store
            .finish_model_change(
                &id,
                &request.request_id,
                ConversationModelRequestState::Applied
            )
            .await
            .unwrap(),
        expected
    );
    let row = opened.store.load(&id).await.unwrap().unwrap();
    assert_eq!(row.model(), &request.requested_model);
    assert_eq!(row.approval_mode(), ConversationApprovalMode::Auto);
    assert!(opened
        .store
        .pending_model_change(&id)
        .await
        .unwrap()
        .is_none());
    let reopened = LocalConversationStore::open(&opened.path).unwrap();
    assert_eq!(
        reopened
            .model_change(&id, &request.request_id)
            .await
            .unwrap(),
        Some(expected.clone())
    );
    assert_eq!(
        reopened
            .finish_model_change(
                &id,
                &request.request_id,
                ConversationModelRequestState::Applied
            )
            .await
            .unwrap(),
        expected
    );
}

#[tokio::test]
async fn retries_cannot_change_actor_surface_target_or_native_observation() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let request = intent(id.clone());
    opened
        .store
        .begin_model_change(request.clone())
        .await
        .unwrap();
    for altered in [
        ConversationModelRequest {
            initiator_principal_id: PrincipalId::new("mallory").unwrap(),
            ..request.clone()
        },
        ConversationModelRequest {
            initiator_surface_id: "other".into(),
            ..request.clone()
        },
        ConversationModelRequest {
            requested_model: ConversationModelId::new("third-model").unwrap(),
            ..request.clone()
        },
        ConversationModelRequest {
            requested_identity: ProviderIdentity::new("provider", "second-model", "other-context")
                .unwrap(),
            ..request.clone()
        },
    ] {
        assert!(matches!(
            opened.store.begin_model_change(altered).await,
            Err(ConversationError::RequestConflict)
        ));
    }
    let wrong = ConversationModelApplication::Applied(ConversationModelSelection {
        model: ConversationModelId::new("third-model").unwrap(),
        approval_mode: ConversationApprovalMode::Auto,
        effect: ConversationModelChangeEffect::NextTurn,
    });
    assert!(matches!(
        opened
            .store
            .observe_model_application(&id, &request.request_id, wrong)
            .await,
        Err(ConversationError::RequestConflict)
    ));
    opened
        .store
        .observe_model_application(&id, &request.request_id, applied(&request))
        .await
        .unwrap();
    assert!(matches!(
        opened
            .store
            .observe_model_application(
                &id,
                &request.request_id,
                ConversationModelApplication::Uncertain
            )
            .await,
        Err(ConversationError::RequestConflict)
    ));
    assert!(matches!(
        opened
            .store
            .finish_model_change(
                &new_id(),
                &request.request_id,
                ConversationModelRequestState::Applied
            )
            .await,
        Err(ConversationError::NotFound)
    ));
    assert_eq!(
        opened.store.load(&id).await.unwrap().unwrap().model(),
        &request.prior_model
    );
}

#[tokio::test]
async fn model_and_permission_intents_exclude_each_other_before_effects() {
    let opened = opened();
    let id = new_id();
    opened.store.create(owned(&id)).await.unwrap();
    let request = intent(id.clone());
    opened
        .store
        .begin_model_change(request.clone())
        .await
        .unwrap();
    let mode = ConversationModeRequest {
        conversation_id: id.clone(),
        organization_id: request.organization_id.clone(),
        request_id: "permission-choice".into(),
        initiator_principal_id: request.initiator_principal_id.clone(),
        initiator_surface_id: request.initiator_surface_id.clone(),
        prior: request.prior_approval_mode,
        requested: ConversationApprovalMode::Full,
        state: ConversationModeRequestState::Pending,
        application: None,
        requested_at_ms: 21,
    };
    assert!(matches!(
        opened.store.begin_mode_change(mode.clone()).await,
        Err(ConversationError::ModelChangeUncertain)
    ));
    opened
        .store
        .observe_model_application(
            &id,
            &request.request_id,
            ConversationModelApplication::Refused,
        )
        .await
        .unwrap();
    opened
        .store
        .finish_model_change(
            &id,
            &request.request_id,
            ConversationModelRequestState::NotApplied,
        )
        .await
        .unwrap();
    opened.store.begin_mode_change(mode).await.unwrap();
    let second = ConversationModelRequest {
        request_id: "next-model".into(),
        ..request
    };
    assert!(matches!(
        opened.store.begin_model_change(second).await,
        Err(ConversationError::ModelChangeUncertain)
    ));
}

#[test]
fn earlier_current_version_shape_is_refused_without_creating_a_model_journal() {
    let opened = opened();
    drop(opened.store);
    let version_before: i64 = raw(&opened.path)
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    raw(&opened.path)
        .execute_batch("DROP TABLE model_requests;")
        .unwrap();
    assert!(matches!(
        LocalConversationStore::open(&opened.path),
        Err(nessa_local_database::OpenError::Damaged(_))
    ));
    let tables: i64 = raw(&opened.path)
        .query_row(
            "SELECT count(*) FROM sqlite_master WHERE name='model_requests'",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(tables, 0);
    let version_after: i64 = raw(&opened.path)
        .query_row("PRAGMA user_version", [], |row| row.get(0))
        .unwrap();
    assert_eq!(version_after, version_before);
}

#[test]
fn a_partial_current_shape_or_missing_pending_constraint_is_refused_at_open() {
    for mutation in [
        "DROP TABLE model_requests; CREATE TABLE model_requests(conversation_id TEXT,request_id TEXT) STRICT;",
        "DROP INDEX one_pending_model_request;",
    ] {
        let opened=opened();
        drop(opened.store);
        let version_before: i64 = raw(&opened.path)
            .query_row("PRAGMA user_version", [], |row| row.get(0))
            .unwrap();
        raw(&opened.path).execute_batch(mutation).unwrap();
        assert!(matches!(LocalConversationStore::open(&opened.path),Err(nessa_local_database::OpenError::Damaged(_))));
        let version:i64=raw(&opened.path).query_row("PRAGMA user_version",[],|row|row.get(0)).unwrap();
        assert_eq!(version,version_before);
    }
}
