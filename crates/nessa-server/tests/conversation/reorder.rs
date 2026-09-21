//! Queue moves cross the authenticated service and actual SDK runner.
use super::{
    ConversationCaller, ConversationLimits, ConversationMessageStatus, ConversationReorderOutcome,
    SubmissionMode,
};
use crate::{conversation::domain::ConversationId, conversation_test_support::fixture};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use std::time::Duration;
use tokio::sync::oneshot;

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("owner").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}
#[tokio::test]
async fn reordered_view_matches_real_dispatch_and_stale_order_cannot_resubmit() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    service
        .submit(
            id.clone(),
            caller("a"),
            "a".into(),
            "first".into(),
            Vec::new(),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(
        Duration::from_secs(2),
        provider.execution_started.notified(),
    )
    .await
    .unwrap();
    for execution in ["b", "c"] {
        service
            .submit(
                id.clone(),
                caller(execution),
                execution.into(),
                execution.into(),
                Vec::new(),
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
    }
    assert_eq!(
        service
            .reorder(id.clone(), caller("move"), vec!["c".into(), "b".into()])
            .await
            .unwrap(),
        ConversationReorderOutcome::Applied
    );
    let view = service.read(id.clone(), caller("read")).await.unwrap();
    assert_eq!(
        view.pending
            .iter()
            .map(|item| item.execution_id.as_str())
            .collect::<Vec<_>>(),
        ["c", "b"]
    );
    assert_eq!(
        service
            .reorder(id.clone(), caller("noop"), vec!["c".into(), "b".into()])
            .await
            .unwrap(),
        ConversationReorderOutcome::Unchanged
    );
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let view = service.read(id.clone(), caller("read")).await.unwrap();
            if view
                .messages
                .iter()
                .filter(|m| m.status == ConversationMessageStatus::Completed)
                .count()
                == 3
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(*provider.executions.lock().unwrap(), ["a", "c", "b"]);
    assert_eq!(
        service
            .reorder(id.clone(), caller("late"), vec!["b".into(), "c".into()])
            .await
            .unwrap(),
        ConversationReorderOutcome::QueueChanged
    );
    assert_eq!(provider.executions.lock().unwrap().len(), 3);
    service.shutdown().await.unwrap();
}
