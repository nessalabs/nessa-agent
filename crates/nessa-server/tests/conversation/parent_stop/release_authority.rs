//! Actual SDK host/disposal handoff joins the registered environment owner.
use super::*;
use crate::conversation::infrastructure::ssh_environment::preparation_environment;
use nessa_sdk::application::agent_execution::permissions::ActionContext;

#[derive(Clone, Copy)]
enum Entry {
    Host,
    Disposal,
}
async fn external_close(entry: Entry, child_target: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (environment, host) = preparation_environment();
    let (service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        environment.into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let work = if child_target {
        Some(hold_turn(&service, &provider, &parent, "parent").await)
    } else {
        None
    };
    let target = if child_target {
        child(&service, &parent, "parent", "external-child").await
    } else {
        parent.clone()
    };
    let opening = Arc::downgrade(
        service
            .inner
            .conversations
            .lock()
            .await
            .get(&target)
            .unwrap(),
    );
    let live = service
        .resolve(&target, &caller("inspect-live"))
        .await
        .unwrap();
    live.join_attachment_owner().await.unwrap();
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    let gate = coordinator.participation(&lifetime).unwrap();
    let actor = ActionContext::new("person", "panel", "external-first").unwrap();
    let closing = tokio::spawn({
        let agent = live.agent.clone();
        let gate = gate.clone();
        async move {
            match entry {
                Entry::Host => agent.end_owned_lifetime(actor).await.map(|_| ()),
                Entry::Disposal => {
                    // Exercise the disposal gate with the real Agent cleanup;
                    // final-handle-drop triggering itself has separate SDK coverage.
                    gate.seal_for_disposal();
                    agent.close(actor).await.unwrap();
                    gate.join_descendants().await
                }
            }
        }
    });
    host.wait_end().await;
    let result = tokio::time::timeout(TURN_HOLD_HANG, closing)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(result, Err(AgentError::InvalidInput(_))),
        "{result:?}"
    );
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closing)
    );
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    assert_eq!(host.held(), if child_target { 2 } else { 1 });
    assert!(service.owns_unreleased_resources().await);
    let cause = coordinator.close_cause(&lifetime).unwrap();
    let first = coordinator.close_initiator(&lifetime).unwrap();
    match entry {
        Entry::Host => {
            assert_eq!(cause, LifetimeCause::HostClose);
            assert_eq!(
                first,
                Initiator::Host(HostActor::new("person", "panel", "external-first").unwrap())
            );
        }
        Entry::Disposal => {
            assert_eq!(cause, LifetimeCause::OwnerDisposed);
            assert_eq!(first, Initiator::Runtime);
        }
    }
    if child_target {
        assert!(service
            .create_owned_thread(
                source(&parent, "parent", "while-external-pending"),
                id(),
                RequestedConversation::default()
            )
            .await
            .is_err());
    }
    host.release();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while host.held() != usize::from(child_target) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::Deletion,
            initiator: Initiator::Runtime,
            external_attachment: false,
            timeout: None,
        })
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(coordinator.close_cause(&lifetime), Some(cause));
    assert_eq!(coordinator.close_initiator(&lifetime), Some(first));
    assert!(
        opening.upgrade().is_none(),
        "acknowledged owner releases its exact Slot"
    );
    assert_eq!(host.end_requests(), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    if child_target {
        child(&service, &parent, "parent", "after-external-ack").await;
    } else {
        assert!(!service.owns_unreleased_resources().await);
    }
    drop(work);
    drop(live);
    host.close();
    retire(service, storage, metadata).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_root_end_waits_for_actual_environment_owner() {
    external_close(Entry::Host, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn external_child_end_keeps_capacity_until_actual_environment_ack() {
    external_close(Entry::Host, true).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disposal_root_phase_hands_off_to_actual_environment_owner() {
    external_close(Entry::Disposal, false).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disposal_child_phase_keeps_capacity_until_actual_environment_ack() {
    external_close(Entry::Disposal, true).await;
}
