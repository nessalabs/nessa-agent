//! Actual gateway composition: persistent SDK graph + ordinary command receipts.

use super::*;
use crate::conversation::application::{
    AgentToolSource, ConversationCreationAudit, ConversationCreationAuditRecord,
    ConversationFuture, Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease,
    PendingReport, ThreadReport, ThreadReportStore,
};
use crate::conversation::infrastructure::{DurableOwnershipAudit, LocalThreadReports};
use nessa_sdk::application::agent_execution::agents::{owed_turn_ids, resumed_turn_id, Resumed};
use nessa_sdk::application::agent_execution::commands;
use nessa_sdk::application::agent_execution::executions::{
    ExecutionRequest, ExecutionUpdate, Submitter,
};
use nessa_sdk::application::agent_execution::providers::{
    AgentProvider, CleanupReport, CloseOutcome, ExecutionReport, ObservationFailure,
    ObservationFailureCause, ProviderExecutionReply, ProviderSessionState,
};
use nessa_sdk::application::agent_execution::sessions::StorageError;
use nessa_sdk::application::agent_execution::subagents::{
    CloseCommand, LiveCapacity, OwnershipAudit, OwnershipCoordinator, OwnershipDependencies,
    OwnershipStore, PortFailure,
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionOutcome, Resumption};
use nessa_sdk::domain::agent_execution::leases::{LeaseCleanup, LeaseId, LeaseRefusal, LeaseTerms};
use nessa_sdk::domain::agent_execution::prompts::{PromptText, UserMessage};
use nessa_sdk::domain::agent_execution::subagents::{
    HostActor, Initiator, LifetimeCause, LifetimeState, OwnershipEvidence, OwnershipMeaning,
    OwnershipSnapshot, SpawnProgress,
};
use nessa_sdk::infrastructure::session_storage::SqliteOwnershipStore;
use sha2::{Digest, Sha256};
use std::sync::atomic::AtomicBool;
use std::sync::Mutex;
use tokio::sync::Notify;
pub(super) async fn bind(
    service: &ConversationService,
    storage: &Arc<RecordStorage>,
    root: &Path,
    cascade: bool,
) -> OwnershipCoordinator {
    bind_with_capacity(service, storage, root, cascade, 32).await
}

async fn bind_with_capacity(
    service: &ConversationService,
    storage: &Arc<RecordStorage>,
    root: &Path,
    cascade: bool,
    capacity: usize,
) -> OwnershipCoordinator {
    service.bind_commands(storage.clone());
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap()),
        audit: Arc::new(
            DurableOwnershipAudit::new(root.join("ownership-audit"), Arc::new(TestClock)).unwrap(),
        ),
        factory: service.owned_thread_factory(),
        room: Arc::new(LiveCapacity::new(capacity)),
    });
    coordinator.resume().await.unwrap();
    service
        .bind_owned_threads(coordinator.clone(), cascade)
        .unwrap();
    let restored = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let snapshot = OwnershipStore::read(&restored).await.unwrap();
    service.recover_owned_threads(&snapshot).await.unwrap();
    coordinator
}
fn source(parent: &ConversationId, execution: &str, request: &str) -> AgentToolSource {
    AgentToolSource {
        caller: caller(request),
        source: parent.clone(),
        execution: execution.into(),
    }
}
async fn child(
    service: &ConversationService,
    parent: &ConversationId,
    execution: &str,
    request: &str,
) -> ConversationId {
    let child = id();
    service
        .create_owned_thread(
            source(parent, execution, request),
            child.clone(),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    child
}

struct RejectChildCreationAudit;
impl ConversationCreationAudit for RejectChildCreationAudit {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async move {
            if record.correlation_id == "audit-failed-child" {
                Err(ConversationError::Audit)
            } else {
                Ok(())
            }
        })
    }
}

#[tokio::test]
async fn child_creation_audit_failure_cannot_strand_unacquired_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (mut service, storage, metadata) = fixture(&root, provider.clone());
    Arc::get_mut(&mut service.inner)
        .expect("fixture is not shared yet")
        .creation_audit = Arc::new(RejectChildCreationAudit);
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let rejected = id();
    assert!(service
        .create_owned_thread(
            source(&parent, "parent", "audit-failed-child"),
            rejected.clone(),
            RequestedConversation::default()
        )
        .await
        .is_err());
    assert!(metadata.load(&rejected).await.unwrap().is_some());
    let snapshot = SqliteOwnershipStore::open(&root.join("ownership.sqlite3"))
        .unwrap()
        .read()
        .await
        .unwrap();
    let failed = snapshot
        .spawns
        .iter()
        .find(|row| row.child_session == conversation_session(&rejected))
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&failed.child_lifetime),
        Some(LifetimeState::Closed)
    );
    assert_eq!(
        provider.open_calls.load(Ordering::SeqCst),
        1,
        "the rejected audit acquired no child provider"
    );
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    child(&service, &parent, "parent", "after-audit-failure").await;
    assert_eq!(
        provider.open_calls.load(Ordering::SeqCst),
        2,
        "the single child slot is available after definite absence"
    );
    drop(p);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn owned_preparation_cannot_adopt_an_existing_unowned_conversation() {
    for ordinary_ready in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture_with_sessions(
            &root,
            provider.clone(),
            crate::conversation::infrastructure::in_process_environment().into(),
            crate::conversation_test_support::access_policy(),
            crate::conversation_test_support::claude_erasers(),
        );
        // Ordinary metadata and a live slot can predate ownership composition.
        let unrelated = id();
        if ordinary_ready {
            service
                .create_command(
                    storage.clone(),
                    unrelated.clone(),
                    caller("colliding-child"),
                    RequestedConversation::default(),
                )
                .await
                .unwrap();
        } else {
            opened(&service, &provider, &unrelated).await;
        }
        let original = metadata.load(&unrelated).await.unwrap().unwrap();
        let u = hold_turn(&service, &provider, &unrelated, "unrelated").await;
        let coordinator = bind(&service, &storage, &root, true).await;
        let parent = id();
        opened(&service, &provider, &parent).await;
        let p = hold_turn(&service, &provider, &parent, "parent").await;
        assert!(service
            .create_owned_thread(
                source(&parent, "parent", "colliding-child"),
                unrelated.clone(),
                RequestedConversation::default(),
            )
            .await
            .is_err());
        assert!(
            coordinator
                .active_lifetime_for_session(&conversation_session(&unrelated))
                .is_none(),
            "a rejected preparation cannot adopt the unrelated target"
        );
        service
            .stop_command(
                storage.clone(),
                parent.clone(),
                caller("stop-parent"),
                "parent".into(),
            )
            .await
            .unwrap();
        assert_eq!(provider.cancel_turns.lock().unwrap().as_slice(), ["parent"]);
        drop(p);
        let closes = provider.close_calls.load(Ordering::SeqCst);
        assert!(service
            .delete(parent, caller("delete-parent"))
            .await
            .unwrap());
        assert_eq!(provider.close_calls.load(Ordering::SeqCst), closes + 1);
        assert_eq!(metadata.load(&unrelated).await.unwrap().unwrap(), original);
        assert_eq!(
            service
                .resolve(&unrelated, &caller("inspect-unrelated"))
                .await
                .unwrap()
                .agent
                .active_execution_id()
                .unwrap()
                .as_str(),
            "unrelated"
        );
        drop(u);
        service
            .close(unrelated.clone(), caller("close-unrelated"))
            .await
            .unwrap();
        service
            .create(
                unrelated.clone(),
                caller("reopen-unrelated"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        assert!(
            coordinator
                .active_lifetime_for_session(&conversation_session(&unrelated))
                .is_some(),
            "a proven absent reservation cannot poison the unrelated conversation's later opening"
        );
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn parent_stop_default_override_grandchildren_and_unrelated_execution() {
    for (cascade, idle_intermediate) in [(true, false), (false, false), (true, true)] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        let coordinator = bind(&service, &storage, &root, cascade).await;
        let parent = id();
        opened(&service, &provider, &parent).await;
        let p = hold_turn(&service, &provider, &parent, "parent").await;
        let c_id = child(&service, &parent, "parent", "create-child").await;
        assert_eq!(provider.executions.lock().unwrap().as_slice(), ["parent"]);
        let mut c = Some(hold_turn(&service, &provider, &c_id, "child").await);
        let g_id = child(&service, &c_id, "child", "create-grandchild").await;
        let g = hold_turn(&service, &provider, &g_id, "grandchild").await;
        if idle_intermediate {
            service
                .stop_command(
                    storage.clone(),
                    c_id.clone(),
                    caller("idle-child"),
                    "child".into(),
                )
                .await
                .unwrap();
            drop(c.take());
            let live = service
                .resolve(&c_id, &caller("inspect-idle"))
                .await
                .unwrap();
            tokio::time::timeout(Duration::from_secs(5), async {
                while !live.agent.is_idle().await {
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .expect("the intermediate child is idle before root Stop");
            provider.cancel_turns.lock().unwrap().clear();
        }
        let unrelated = id();
        opened(&service, &provider, &unrelated).await;
        let u = hold_turn(&service, &provider, &unrelated, "unrelated").await;
        let receipt = service
            .stop_command(
                storage.clone(),
                parent.clone(),
                caller("stop-tree"),
                "parent".into(),
            )
            .await
            .unwrap();
        assert_eq!(receipt.outcome(), Some(MutationOutcome::Cancelled));
        let cancelled = provider.cancel_turns.lock().unwrap().clone();
        assert!(cancelled.contains(&"parent".to_owned()));
        assert_eq!(
            cancelled.contains(&"child".to_owned()),
            cascade && !idle_intermediate
        );
        assert_eq!(cancelled.contains(&"grandchild".to_owned()), cascade);
        assert!(!cancelled.contains(&"unrelated".to_owned()));
        assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
        assert_eq!(
            coordinator.lifetime_state(
                &coordinator
                    .active_lifetime_for_session(&conversation_session(&parent))
                    .unwrap()
            ),
            Some(LifetimeState::Open)
        );
        let count = provider.cancel_calls.load(Ordering::SeqCst);
        assert_eq!(
            service
                .stop_command(
                    storage.clone(),
                    parent.clone(),
                    caller("stop-tree"),
                    "parent".into()
                )
                .await
                .unwrap(),
            receipt
        );
        assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), count);
        let binding =
            ConversationService::stop_lookup(&parent, &caller("stop-tree"), "parent").unwrap();
        assert_eq!(
            service
                .lookup_command(storage.clone(), parent, caller("lookup-stop"), binding)
                .await
                .unwrap(),
            Some(receipt)
        );
        drop((p, c, g, u));
        retire(service, storage, metadata).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_during_its_queue_handoff_still_captures_the_owned_child() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let coordinator = bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let first = hold_turn(&service, &provider, &parent, "first").await;
    let child_id = child(&service, &parent, "first", "create-child").await;
    let child_work = hold_turn(&service, &provider, &child_id, "child").await;
    drop(first);
    let live = service.resolve(&parent, &caller("inspect")).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !live.agent.is_idle().await {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let began = Arc::new(Notify::new());
    let (prepare, preparation) = oneshot::channel();
    *provider.prepare_gate.lock().unwrap() = Some((began.clone(), preparation));
    let (execution, execution_gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(execution_gate);
    let preparing = began.notified();
    send(&service, &parent, "moving-root", "now").await;
    tokio::time::timeout(Duration::from_secs(5), preparing)
        .await
        .unwrap();
    assert!(live.agent.queued_ids().await.is_empty());
    assert!(live.agent.active_execution_id().is_none());
    let stopping = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let parent = parent.clone();
        async move {
            service
                .stop_command(storage, parent, caller("stop-moving"), "moving-root".into())
                .await
        }
    });
    // Same held provider-preparation prefix as the upstream moving-turn test.
    tokio::time::sleep(Duration::from_millis(100)).await;
    let waited = !stopping.is_finished();
    prepare.send(()).unwrap();
    let receipt = tokio::time::timeout(Duration::from_secs(5), stopping)
        .await
        .unwrap()
        .unwrap();
    drop((execution, child_work, live));
    let cancelled = provider.cancel_turns.lock().unwrap().clone();
    let child_lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&child_id))
        .unwrap();
    let state = coordinator.lifetime_state(&child_lifetime);
    retire(service, storage, metadata).await;
    assert!(waited, "Stop waits for the exact parent's selected turn");
    assert!(matches!(
        receipt.unwrap().outcome(),
        Some(MutationOutcome::Cancelled | MutationOutcome::AlreadyFinal)
    ));
    assert!(cancelled.contains(&"moving-root".into()));
    assert!(cancelled.contains(&"child".into()));
    assert_eq!(state, Some(LifetimeState::Open));
}

#[tokio::test]
async fn parent_stop_cancels_owned_active_and_queued_turns_and_keeps_threads_reusable() {
    for (cascade, handoff) in [(true, false), (true, true), (false, false)] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        let coordinator = bind(&service, &storage, &root, cascade).await;
        let parent = id();
        opened(&service, &provider, &parent).await;
        let p = hold_turn(&service, &provider, &parent, "parent").await;
        let child_id = child(&service, &parent, "parent", "create-child").await;
        let mut c = Some(hold_turn(&service, &provider, &child_id, "child").await);
        let grandchild = child(&service, &child_id, "child", "create-grandchild").await;
        let mut g = Some(hold_turn(&service, &provider, &grandchild, "grandchild").await);
        for (target, execution) in [
            (&child_id, "child-queued"),
            (&grandchild, "grandchild-queued"),
        ] {
            service
                .submit_command(
                    storage.clone(),
                    target.clone(),
                    caller(execution),
                    Initiation::owner(),
                    false,
                    execution.into(),
                    message("queued work captured by parent Stop"),
                    SubmissionMode::Queue,
                )
                .await
                .unwrap();
        }
        let first_queued = if child_id.to_string() < grandchild.to_string() {
            "child-queued"
        } else {
            "grandchild-queued"
        };
        let mut queued_release = None;
        if handoff {
            let (release_root_cancel, root_cancel) = oneshot::channel();
            *provider.cancel_gate.lock().unwrap() = Some(root_cancel);
            let root_started = provider.cancel_started.notified();
            let stop = tokio::spawn({
                let service = service.clone();
                let storage = storage.clone();
                let parent = parent.clone();
                async move {
                    service
                        .stop_command(storage, parent, caller("stop"), "parent".into())
                        .await
                }
            });
            tokio::time::timeout(TURN_HOLD_HANG, root_started)
                .await
                .unwrap();
            let (release_child_cancel, child_cancel) = oneshot::channel();
            *provider.cancel_gate.lock().unwrap() = Some(child_cancel);
            let child_started = provider.cancel_started.notified();
            release_root_cancel.send(()).unwrap();
            tokio::time::timeout(TURN_HOLD_HANG, child_started)
                .await
                .unwrap();
            let (release, queued) = oneshot::channel();
            queued_release = Some(release);
            *provider.execution_gate.lock().unwrap() = Some(queued);
            // Settle the active invocation while its cancellation ACK waits.
            // The captured queued identity then becomes the active identity.
            if first_queued == "child-queued" {
                c.take().unwrap().send(()).unwrap();
            } else {
                g.take().unwrap().send(()).unwrap();
            }
            tokio::time::timeout(TURN_HOLD_HANG, async {
                while !provider
                    .executions
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|id| id == first_queued)
                {
                    tokio::task::yield_now().await;
                }
            })
            .await
            .expect("the captured queued turn dispatches during active cancellation");
            release_child_cancel.send(()).unwrap();
            stop.await.unwrap().unwrap();
        } else {
            service
                .stop_command(
                    storage.clone(),
                    parent.clone(),
                    caller("stop"),
                    "parent".into(),
                )
                .await
                .unwrap();
        }
        for (target, execution) in [
            (&child_id, "child-queued"),
            (&grandchild, "grandchild-queued"),
        ] {
            let live = service.resolve(target, &caller("inspect")).await.unwrap();
            assert_eq!(
                live.agent
                    .queued_ids()
                    .await
                    .contains(&ExecutionId::new(execution).unwrap()),
                !cascade,
                "recursive Stop leaves no captured descendant turn pending in the queue"
            );
            assert_eq!(
                provider
                    .executions
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|id| id == execution),
                handoff && execution == first_queued
            );
        }
        let cancelled = provider.cancel_turns.lock().unwrap().clone();
        assert!(cancelled.iter().any(|id| id == "parent"));
        for execution in ["child", "grandchild"] {
            assert_eq!(cancelled.iter().any(|id| id == execution), cascade);
        }
        for execution in ["child-queued", "grandchild-queued"] {
            assert_eq!(
                cancelled.iter().any(|id| id == execution),
                handoff && execution == first_queued,
                "a dispatched queued turn is cancelled using its captured exact identity"
            );
        }
        assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
        for target in [&parent, &child_id, &grandchild] {
            let lifetime = coordinator
                .active_lifetime_for_session(&conversation_session(target))
                .unwrap();
            assert_eq!(
                coordinator.lifetime_state(&lifetime),
                Some(LifetimeState::Open)
            );
        }
        drop((p, c, g, queued_release));
        if cascade {
            for (target, execution) in [
                (&parent, "parent-next"),
                (&child_id, "child-next"),
                (&grandchild, "grandchild-next"),
            ] {
                let live = service
                    .resolve(target, &caller("inspect-idle"))
                    .await
                    .unwrap();
                tokio::time::timeout(TURN_HOLD_HANG, async {
                    while !live.agent.is_idle().await {
                        tokio::task::yield_now().await;
                    }
                })
                .await
                .expect("the stopped exact turn retires before new work");
                finish_turn(&service, &provider, target, execution).await;
                assert!(provider
                    .executions
                    .lock()
                    .unwrap()
                    .iter()
                    .any(|id| id == execution));
            }
        }
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn a_stale_parent_stop_does_not_stop_its_child_or_fence_new_reports() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    finish_turn(&service, &provider, &parent, "old-parent").await;
    let p = hold_turn(&service, &provider, &parent, "new-parent").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let live = service.resolve(&parent, &caller("inspect")).await.unwrap();
            if live
                .agent
                .active_execution_id()
                .as_ref()
                .map(ExecutionId::as_str)
                == Some("new-parent")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let c_id = child(&service, &parent, "new-parent", "create-child").await;
    let c = hold_turn(&service, &provider, &c_id, "child").await;
    let result = service
        .stop_command(
            storage.clone(),
            parent.clone(),
            caller("stale-stop"),
            "old-parent".into(),
        )
        .await
        .unwrap();
    assert_eq!(result.outcome(), Some(MutationOutcome::AlreadyFinal));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert!(service
        .captured_stop_actor(&parent, "old-parent", &caller("read"))
        .await
        .unwrap()
        .is_none());
    drop((p, c));
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn caller_disconnect_and_late_creation_cannot_escape_a_selected_stop() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let c_id = child(&service, &parent, "parent", "create-child").await;
    let c = hold_turn(&service, &provider, &c_id, "child").await;
    let (release, gate) = oneshot::channel();
    *provider.cancel_gate.lock().unwrap() = Some(gate);
    let task = {
        let service = service.clone();
        let storage = storage.clone();
        let parent = parent.clone();
        tokio::spawn(async move {
            service
                .stop_command(storage, parent, caller("stop"), "parent".into())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), provider.cancel_started.notified())
        .await
        .unwrap();
    task.abort();
    let _ = task.await;
    let late = id();
    assert!(service
        .create_owned_thread(
            source(&parent, "parent", "too-late"),
            late.clone(),
            RequestedConversation::default()
        )
        .await
        .is_err());
    assert!(metadata.load(&late).await.unwrap().is_none());
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider
                .cancel_turns
                .lock()
                .unwrap()
                .iter()
                .any(|id| id == "child")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop((p, c));
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn queued_child_report_is_retained_across_stop_restart_and_only_person_work_releases_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let reports = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(reports.clone());
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let c_id = child(&service, &parent, "parent", "create-child").await;
    let c = hold_turn(&service, &provider, &c_id, "child").await;
    service
        .report_thread_turn(
            c_id.clone(),
            "child".into(),
            ThreadReport {
                parent: parent.clone(),
                caller: caller("report-action"),
                execution: "child-report".into(),
            },
        )
        .await;
    c.send(()).unwrap();
    service.look_for_reports(c_id.clone(), false).await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let history = service
                .resolve(&parent, &caller("read"))
                .await
                .unwrap()
                .agent
                .session_manager()
                .snapshot()
                .await
                .unwrap();
            if history
                .invocations
                .iter()
                .any(|record| record.request.execution_id.as_str() == "child-report")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        reports.pending().await.unwrap().len(),
        1,
        "queued report keeps its durable attribution"
    );
    service
        .submit_command(
            storage.clone(),
            parent.clone(),
            caller("person-queued-before-stop"),
            Initiation::owner(),
            false,
            "person-already-queued".into(),
            message("work admitted before Stop"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    // Report polling may hold the exclusive principal stream. Retry only
    // typed contention, retaining the original command and exact selection.
    tokio::time::timeout(TURN_HOLD_HANG, async {
        loop {
            match service
                .stop_command(
                    storage.clone(),
                    parent.clone(),
                    caller("stop-parent"),
                    "parent".into(),
                )
                .await
            {
                Ok(_) => break,
                Err(commands::MutationFailure::Storage(
                    commands::CreationStorageError::Storage(StorageError::Busy),
                )) => tokio::task::yield_now().await,
                Err(error) => panic!("same-identity Stop retry failed: {error:?}"),
            }
        }
    })
    .await
    .expect("Stop settles after transient principal-stream contention");
    assert!(!provider
        .executions
        .lock()
        .unwrap()
        .iter()
        .any(|id| id == "child-report"));
    assert_eq!(
        reports.pending().await.unwrap().len(),
        1,
        "withdrawing delivery does not lose the child result"
    );
    drop(p);
    retire(service, storage, metadata).await;

    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    service.bind_thread_reports(reports.clone());
    assert_eq!(service.resume_thread_reports().await.unwrap(), 1);
    let automated = id();
    opened(&service, &provider, &automated).await;
    let a = hold_turn(&service, &provider, &automated, "other-agent").await;
    service
        .submit_agent_command(
            storage.clone(),
            parent.clone(),
            source(&automated, "other-agent", "automated-send"),
            "automated-parent-message".into(),
            message("an agent message"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let live = service.resolve(&parent, &caller("read")).await.unwrap();
    let history = live.agent.session_manager().snapshot().await.unwrap();
    {
        let _admission = service.inner.thread_admission.clone().lock_owned().await;
        assert!(
            service
                .reports_paused(&parent, &caller("read"), &history.invocations)
                .await
                .unwrap(),
            "automated work does not release the person's Stop"
        );
    }
    assert_eq!(reports.pending().await.unwrap().len(), 1);
    service
        .submit_command(
            storage.clone(),
            parent.clone(),
            caller("person-send"),
            Initiation::owner(),
            false,
            "person-next".into(),
            message("continue"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if reports.pending().await.unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let executions = provider.executions.lock().unwrap().clone();
    assert_eq!(
        executions
            .iter()
            .filter(|id| id.starts_with("thread-report-"))
            .count(),
        1,
        "one new delivery after explicit person admission"
    );
    let history = live.agent.session_manager().snapshot().await.unwrap();
    let old = history
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == "child-report")
        .unwrap();
    assert_eq!(
        old.scheduling.last().unwrap().stage,
        InvocationStage::Cancelled
    );
    assert!(
        !live
            .agent
            .owed_turns()
            .await
            .iter()
            .any(|turn| matches!(turn.execution_id().as_str(), "parent" | "child-report")),
        "person Stop has not become suspended/owed work"
    );
    drop((a, live));
    retire(service, storage, metadata).await;
}

struct FaultyCommands {
    storage: Arc<RecordStorage>,
    target: ConversationId,
    commit: bool,
    stage: MutationStage,
    once: Arc<AtomicBool>,
}
struct FaultyLease {
    inner: Arc<dyn commands::CreationStorageLease>,
    target: ConversationId,
    commit: bool,
    stage: MutationStage,
    once: Arc<AtomicBool>,
}
impl commands::CreationStorage for FaultyCommands {
    fn mutations(&self, principal: String) -> commands::CreationFuture<'_, Vec<MutationReceipt>> {
        self.storage.mutations(principal)
    }
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> commands::CreationFuture<'_, Option<Arc<dyn commands::CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .storage
                .open_creation(principal, request, create)
                .await?
                .map(|inner| {
                    Arc::new(FaultyLease {
                        inner,
                        target: self.target.clone(),
                        commit: self.commit,
                        stage: self.stage,
                        once: self.once.clone(),
                    }) as Arc<dyn commands::CreationStorageLease>
                }))
        })
    }
}
impl commands::CreationStorageLease for FaultyLease {
    fn load(
        &self,
        request: &str,
    ) -> commands::CreationFuture<'_, Option<commands::CreationReceipt>> {
        self.inner.load(request)
    }
    fn save(&self, receipt: commands::CreationReceipt) -> commands::CreationFuture<'_, ()> {
        self.inner.save(receipt)
    }
    fn load_mutation(
        &self,
        request: &str,
    ) -> commands::CreationFuture<'_, Option<MutationReceipt>> {
        self.inner.load_mutation(request)
    }
    fn save_mutation(&self, receipt: MutationReceipt) -> commands::CreationFuture<'_, ()> {
        let fail = receipt.binding().target() == &conversation_session(&self.target)
            && receipt.stage() == self.stage
            && self.once.swap(false, Ordering::SeqCst);
        Box::pin(async move {
            if !fail {
                return self.inner.save_mutation(receipt).await;
            }
            if self.commit {
                self.inner.save_mutation(receipt).await?;
            }
            Err(commands::CreationStorageError::Storage(
                StorageError::Corrupt("simulated settlement acknowledgement loss".into()),
            ))
        })
    }
}
#[tokio::test]
async fn partial_child_settlement_and_lost_ack_keep_exact_retry_and_honest_lookup() {
    for commit in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        bind(&service, &storage, &root, true).await;
        let parent = id();
        opened(&service, &provider, &parent).await;
        let p = hold_turn(&service, &provider, &parent, "parent").await;
        let a_id = child(&service, &parent, "parent", "create-a").await;
        let a = hold_turn(&service, &provider, &a_id, "a").await;
        let b_id = child(&service, &parent, "parent", "create-b").await;
        let b = hold_turn(&service, &provider, &b_id, "b").await;
        let first = if a_id.to_string() < b_id.to_string() {
            a_id.clone()
        } else {
            b_id.clone()
        };
        let faulty: Arc<dyn commands::CreationStorage> = Arc::new(FaultyCommands {
            storage: storage.clone(),
            target: first.clone(),
            commit,
            stage: MutationStage::Settled,
            once: Arc::new(AtomicBool::new(true)),
        });
        assert!(service
            .stop_command(
                faulty.clone(),
                parent.clone(),
                caller("stop"),
                "parent".into()
            )
            .await
            .is_err());
        assert_eq!(
            provider.cancel_calls.load(Ordering::SeqCst),
            3,
            "one child failure does not prevent the next captured child Stop"
        );
        let binding = ConversationService::stop_lookup(&parent, &caller("stop"), "parent").unwrap();
        let mut foreign = caller("foreign-lookup");
        foreign.principal_id = PrincipalId::new("stranger").unwrap();
        assert!(
            matches!(
                service
                    .lookup_command(storage.clone(), parent.clone(), foreign, binding.clone())
                    .await,
                Err(commands::MutationFailure::Target(
                    ConversationError::NotFound
                ))
            ),
            "current root access precedes aggregate child receipt evidence"
        );
        let lookup = service
            .lookup_command(storage.clone(), parent.clone(), caller("read"), binding)
            .await;
        assert_eq!(
            lookup.is_ok(),
            commit,
            "lookup cannot advertise completed cascade with an Attempted child"
        );
        assert_eq!(
            service
                .stop_command(faulty, parent.clone(), caller("stop"), "parent".into())
                .await
                .is_ok(),
            commit
        );
        assert_eq!(
            provider.cancel_calls.load(Ordering::SeqCst),
            3,
            "uncertain effects are not repeated"
        );
        drop((p, a, b));
        retire(service, storage, metadata).await;
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        bind(&service, &storage, &root, true).await;
        opened(&service, &provider, &first).await;
        let fresh = hold_turn(&service, &provider, &first, "after-restart").await;
        assert_eq!(
            service
                .stop_command(
                    storage.clone(),
                    parent.clone(),
                    caller("stop"),
                    "parent".into()
                )
                .await
                .is_ok(),
            commit
        );
        assert_eq!(
            provider.cancel_calls.load(Ordering::SeqCst),
            0,
            "restart retry keeps original targets instead of a child's new work"
        );
        drop(fresh);
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn an_accepted_selection_recovers_without_replaying_work_or_reinterpreting_settings() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let c_id = child(&service, &parent, "parent", "create-child").await;
    let c = hold_turn(&service, &provider, &c_id, "child").await;
    let faulty: Arc<dyn commands::CreationStorage> = Arc::new(FaultyCommands {
        storage: storage.clone(),
        target: parent.clone(),
        commit: true,
        stage: MutationStage::Accepted,
        once: Arc::new(AtomicBool::new(true)),
    });
    assert!(service
        .stop_command(
            faulty,
            parent.clone(),
            caller("stop-before-effect"),
            "parent".into()
        )
        .await
        .is_err());
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    let retiring = {
        let service = service.clone();
        tokio::spawn(async move { service.shutdown().await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider.close_calls.load(Ordering::SeqCst) >= 2 {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop((p, c));
    retiring.await.unwrap().unwrap();
    drop(service);
    storage.shutdown().await.unwrap();
    drop(storage);
    drop(metadata);

    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, false).await;
    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let snapshot = owned.read().await.unwrap();
    assert_eq!(service.recover_parent_stops(&snapshot).await.unwrap(), 1);
    let binding =
        ConversationService::stop_lookup(&parent, &caller("stop-before-effect"), "parent").unwrap();
    let mut last = String::new();
    let completed = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let found = service
                .lookup_command(
                    storage.clone(),
                    parent.clone(),
                    caller("read"),
                    binding.clone(),
                )
                .await;
            last = format!("{found:?}");
            if found.is_ok_and(|receipt| {
                receipt.is_some_and(|receipt| receipt.stage() == MutationStage::Settled)
            }) {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await;
    assert!(completed.is_ok(), "last lookup: {last}");
    for target in [&parent, &c_id] {
        let live = service.resolve(target, &caller("inspect")).await.unwrap();
        assert!(live.agent.owed_turns().await.is_empty());
        let history = live.agent.session_manager().snapshot().await.unwrap();
        assert!(
            history.invocations[0].result.is_some(),
            "recovery preserves the provider's original final outcome"
        );
    }
    assert!(
        provider.executions.lock().unwrap().is_empty(),
        "durable Stop is not resumed as a model turn"
    );
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn foreign_callers_and_report_id_collisions_do_not_cancel_unrelated_user_work() {
    person_input_releases_report_pause("later-person").await;
}

#[tokio::test]
async fn an_ordinary_resume_shaped_person_submit_releases_report_pause() {
    let execution = Resumption::of(&ExecutionId::new("arbitrary-person-input").unwrap())
        .successor()
        .unwrap();
    person_input_releases_report_pause(execution.as_str()).await;
}

async fn person_input_releases_report_pause(later_person: &str) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let reports = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(reports.clone());
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let c_id = child(&service, &parent, "parent", "create-child").await;
    let c = hold_turn(&service, &provider, &c_id, "child").await;
    let mut foreign = caller("foreign-stop");
    foreign.principal_id = PrincipalId::new("someone-else").unwrap();
    assert!(service
        .stop_command(storage.clone(), parent.clone(), foreign, "parent".into())
        .await
        .is_err());
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    service
        .submit_command(
            storage.clone(),
            parent.clone(),
            caller("report-action"),
            Initiation::owner(),
            false,
            "colliding-report".into(),
            message("unrelated person work"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    service
        .report_thread_turn(
            c_id.clone(),
            "child".into(),
            ThreadReport {
                parent: parent.clone(),
                caller: caller("report-action"),
                execution: "colliding-report".into(),
            },
        )
        .await;
    service
        .stop_command(
            storage.clone(),
            parent.clone(),
            caller("stop"),
            "parent".into(),
        )
        .await
        .unwrap();
    drop((p, c));
    assert!(!provider
        .cancel_turns
        .lock()
        .unwrap()
        .iter()
        .any(|id| id == "colliding-report"));
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if provider
                .executions
                .lock()
                .unwrap()
                .iter()
                .any(|id| id == "colliding-report")
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let live = service.resolve(&parent, &caller("read")).await.unwrap();
    let history = live.agent.session_manager().snapshot().await.unwrap();
    assert!(
        service
            .reports_paused(&parent, &caller("read"), &history.invocations)
            .await
            .unwrap(),
        "a person admission before Stop cannot release it afterward"
    );
    assert_eq!(
        reports.pending().await.unwrap().len(),
        1,
        "an unrelated record cannot acknowledge the report"
    );
    let submitted = service
        .submit_command(
            storage.clone(),
            parent.clone(),
            caller("later-person"),
            Initiation::owner(),
            false,
            later_person.into(),
            message("continue"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(
        submitted.progress.outcome(),
        Some(MutationOutcome::Dispatched)
    );
    let history = live.agent.session_manager().snapshot().await.unwrap();
    let record = history
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == later_person)
        .unwrap();
    assert_eq!(record.resumed_from, None);
    assert!(
        !service
            .reports_paused(&parent, &caller("read"), &history.invocations)
            .await
            .unwrap(),
        "the exact later person Submit receipt releases reports regardless of execution-ID spelling"
    );
    tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if reports.pending().await.unwrap().is_empty() {
                break;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        provider
            .executions
            .lock()
            .unwrap()
            .iter()
            .filter(|id| id.starts_with("thread-report-"))
            .count(),
        1
    );
    drop(live);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn a_real_sdk_resume_releases_report_pause_only_after_an_explicit_person_submit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let parent = id();
    let source = ExecutionId::new("sdk-source").unwrap();
    {
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        bind(&service, &storage, &root, true).await;
        opened(&service, &provider, &parent).await;
        let p = hold_turn(&service, &provider, &parent, "parent").await;
        service
            .stop_command(
                storage.clone(),
                parent.clone(),
                caller("stop"),
                "parent".into(),
            )
            .await
            .unwrap();
        drop(p);
        // Direct submission admits SDK work without a person command receipt.
        let release = hold_turn(&service, &provider, &parent, source.as_str()).await;
        suspend_child_turn(&service, &provider, &parent, release).await;
        retire(service, storage, metadata).await;
    }
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    // Prevent automatic gateway resume from winning the deliberate SDK call.
    let admission = service.inner.thread_admission.lock().await;
    let live = service.resolve(&parent, &caller("reopen")).await.unwrap();
    let owed = live
        .agent
        .owed_turns()
        .await
        .into_iter()
        .find(|turn| turn.execution_id() == &source)
        .unwrap();
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    let started = provider.execution_started.notified();
    let Resumed::Continued(resumed) = live
        .agent
        .resume_turn(&owed, caller("sdk-resume").actor().unwrap())
        .await
        .unwrap()
    else {
        panic!("the saved SDK source must resume")
    };
    tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();
    let successor = resumed.id().clone();
    let history = live.agent.session_manager().snapshot().await.unwrap();
    let saved = history
        .invocations
        .iter()
        .find(|record| record.request.execution_id == successor)
        .unwrap();
    assert_eq!(saved.resumed_from.as_ref(), Some(&source));
    assert_eq!(
        resumed_turn_id(&history.invocations, &source),
        Some(successor.clone())
    );
    assert!(
        service
            .reports_paused(&parent, &caller("read"), &history.invocations)
            .await
            .unwrap(),
        "a genuine SDK resume alone is not an explicit person command"
    );
    drop(admission);

    acknowledge_report_delivery(&service, &storage, &parent, successor.as_str()).await;
    let history = live.agent.session_manager().snapshot().await.unwrap();
    assert!(
        !service
            .reports_paused(&parent, &caller("read"), &history.invocations)
            .await
            .unwrap(),
        "the exact later person Submit receipt establishes a separate intervention"
    );
    assert_eq!(
        history
            .invocations
            .iter()
            .find(|record| record.request.execution_id == successor)
            .unwrap()
            .resumed_from
            .as_ref(),
        Some(&source)
    );
    drop((release, resumed, live));
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deleting_an_owned_child_releases_ownership_and_does_not_block_parent_stop() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        crate::conversation::infrastructure::in_process_environment().into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let coordinator = bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let removed = child(&service, &parent, "parent", "removed-child").await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&removed))
        .unwrap();
    let sibling = child(&service, &parent, "parent", "surviving-child").await;
    let s = hold_turn(&service, &provider, &sibling, "sibling").await;
    let owned_store = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let before_erasure_close = OwnershipStore::read(&owned_store).await.unwrap();
    assert!(service
        .delete(removed, caller("delete-child"))
        .await
        .unwrap());
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    service
        .stop_command(
            storage.clone(),
            parent,
            caller("stop-after-delete"),
            "parent".into(),
        )
        .await
        .unwrap();
    let cancelled = provider.cancel_turns.lock().unwrap().clone();
    assert!(cancelled.contains(&"parent".to_owned()));
    assert!(cancelled.contains(&"sibling".to_owned()));
    drop((p, s));
    retire(service, storage, metadata).await;
    // Restore the exact valid ownership snapshot from before close, leaving
    // deletion's saved erased tombstone: the crash/lost-close-write boundary.
    OwnershipStore::write(&owned_store, &before_erasure_close)
        .await
        .unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let coordinator = bind(&service, &storage, &root, false).await;
    assert_eq!(
        service
            .recover_owned_threads(&before_erasure_close)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert!(provider.executions.lock().unwrap().is_empty());
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deleting_a_cold_owned_tree_reconciles_confirmed_leases_and_cannot_reparent_children() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let coordinator = bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let child_id = child(&service, &parent, "parent", "child").await;
    let c = hold_turn(&service, &provider, &child_id, "child").await;
    let grandchild = child(&service, &child_id, "child", "grandchild").await;
    let lifetimes: Vec<_> = [&parent, &child_id, &grandchild]
        .into_iter()
        .map(|id| {
            coordinator
                .active_lifetime_for_session(&conversation_session(id))
                .unwrap()
        })
        .collect();
    drop((p, c));
    retire(service, storage, metadata).await;
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture_with_sessions(
        &root,
        provider.clone(),
        crate::conversation::infrastructure::in_process_environment().into(),
        crate::conversation_test_support::access_policy(),
        crate::conversation_test_support::claude_erasers(),
    );
    let coordinator = bind(&service, &storage, &root, true).await;
    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let snapshot = OwnershipStore::read(&owned).await.unwrap();
    service.recover_owned_threads(&snapshot).await.unwrap();
    assert!(service
        .delete(parent, caller("delete-cold-parent"))
        .await
        .unwrap());
    for lifetime in lifetimes {
        assert_eq!(
            coordinator.lifetime_state(&lifetime),
            Some(LifetimeState::Closed)
        );
    }
    assert!(service
        .ownership_gate(&child_id, &caller("reopen-child").actor().unwrap())
        .await
        .is_err());
    assert!(service
        .ownership_gate(&grandchild, &caller("reopen-grandchild").actor().unwrap())
        .await
        .is_err());
    assert!(provider.executions.lock().unwrap().is_empty());
    retire(service, storage, metadata).await;
}

struct PausedOwnedOpening {
    inner: Arc<dyn Environment>,
    pause: AtomicBool,
    entered: Notify,
    release: Notify,
}
impl Environment for PausedOwnedOpening {
    fn declaration(&self) -> EnvironmentDeclaration {
        self.inner.declaration()
    }
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            let opened = self.inner.open(lease, grant, binding).await?;
            if self.pause.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(opened)
        })
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        self.inner.account(lease)
    }
}

#[tokio::test]
async fn an_opening_owner_prevents_historical_cleanup_from_closing_its_lifetime() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let environment = Arc::new(PausedOwnedOpening {
        inner: crate::conversation::infrastructure::in_process_environment(),
        pause: AtomicBool::new(false),
        entered: Notify::new(),
        release: Notify::new(),
    });
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture_in(
        &root,
        provider.clone(),
        (environment.clone() as Arc<dyn Environment>).into(),
    );
    let coordinator = bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let child = child(&service, &parent, "parent", "child").await;
    let parent_lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&parent))
        .unwrap();
    let child_lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&child))
        .unwrap();
    drop(p);
    retire(service, storage, metadata).await;
    let (service, storage, metadata) = fixture_in(
        &root,
        Arc::new(ProviderFactory::default()),
        (environment.clone() as Arc<dyn Environment>).into(),
    );
    let coordinator = bind(&service, &storage, &root, true).await;
    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let snapshot = OwnershipStore::read(&owned).await.unwrap();
    service.recover_owned_threads(&snapshot).await.unwrap();
    environment.pause.store(true, Ordering::SeqCst);
    let opening = {
        let service = service.clone();
        let child = child.clone();
        tokio::spawn(async move { service.resolve(&child, &caller("reopen")).await })
    };
    tokio::time::timeout(Duration::from_secs(5), environment.entered.notified())
        .await
        .unwrap();
    assert!(coordinator
        .end_lifetime(CloseCommand {
            lifetime: parent_lifetime,
            cause: LifetimeCause::Deletion,
            initiator: Initiator::Runtime,
            external_attachment: false,
            timeout: Some(Duration::from_millis(50)),
        })
        .await
        .is_err());
    assert_eq!(
        coordinator.lifetime_state(&child_lifetime),
        Some(LifetimeState::Closing),
        "a newer opening's hold is not released by the previous lease's cleanup"
    );
    environment.release.notify_one();
    assert!(opening.await.unwrap().is_err());
    let _ = service.shutdown().await;
    drop(service);
    storage.shutdown().await.unwrap();
    drop((storage, metadata));
}

#[tokio::test]
async fn stopping_owed_parent_work_cascades_only_after_command_attempt_is_durable() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    *provider.execution_reply.lock().unwrap() =
        Some(ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Err(AgentError::Transport("pipe".into()))),
            None,
            ProviderSessionState::Usable,
        )));
    *provider.execution_observation_failure.lock().unwrap() = Some(ObservationFailure::new(
        AgentError::Transport("pipe".into()),
        ObservationFailureCause::ExecutionFailed,
    ));
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let child_id = child(&service, &parent, "parent", "child").await;
    let c = hold_turn(&service, &provider, &child_id, "child").await;
    drop(p);
    let live = service
        .resolve(&parent, &caller("inspect-owed"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !live
            .agent
            .owed_turns()
            .await
            .iter()
            .any(|turn| turn.execution_id().as_str() == "parent")
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    let faulty: Arc<dyn commands::CreationStorage> = Arc::new(FaultyCommands {
        storage: storage.clone(),
        target: parent.clone(),
        commit: false,
        stage: MutationStage::Attempted,
        once: Arc::new(AtomicBool::new(true)),
    });
    assert!(service
        .stop_command(
            faulty.clone(),
            parent.clone(),
            caller("stop-owed"),
            "parent".into()
        )
        .await
        .is_err());
    assert!(
        live.agent
            .owed_turns()
            .await
            .iter()
            .any(|turn| turn.execution_id().as_str() == "parent"),
        "failed Attempted persistence cannot perform the root decline"
    );
    assert!(
        provider
            .cancel_turns
            .lock()
            .unwrap()
            .contains(&"child".to_owned()),
        "a failed root attempt does not skip its admitted child"
    );
    let receipt = service
        .stop_command(faulty, parent, caller("stop-owed"), "parent".into())
        .await
        .unwrap();
    assert_eq!(receipt.outcome(), Some(MutationOutcome::Withdrawn));
    assert!(live.agent.owed_turns().await.is_empty());
    drop((c, live));
    retire(service, storage, metadata).await;
}
fn recovery_caller_in_org(org: &str, request: &str) -> ConversationCaller {
    let mut admitted = caller(request);
    admitted.organization_id = OrganizationId::new(org).unwrap();
    admitted
}

async fn hold_recovery_turn_in_org(
    service: &ConversationService,
    storage: &Arc<RecordStorage>,
    provider: &ProviderFactory,
    target: &ConversationId,
    org: &str,
    execution: &str,
) -> oneshot::Sender<()> {
    let (release, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    let started = provider.execution_started.notified();
    service
        .submit_command(
            storage.clone(),
            target.clone(),
            recovery_caller_in_org(org, &format!("submit-{execution}")),
            Initiation::owner(),
            false,
            execution.into(),
            message("hold this exact turn"),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, started)
        .await
        .expect("the exact turn reached the real provider fixture");
    release
}

#[tokio::test]
async fn accepted_parent_stops_recover_each_targets_organization_for_one_principal() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let mut selected = Vec::new();
    let mut holds = Vec::new();
    for org in ["org-a", "org-b"] {
        let parent = id();
        service
            .create(
                parent.clone(),
                recovery_caller_in_org(org, &format!("create-{org}")),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        let parent_execution = format!("parent-{org}");
        holds.push(
            hold_recovery_turn_in_org(
                &service,
                &storage,
                &provider,
                &parent,
                org,
                &parent_execution,
            )
            .await,
        );
        let child = id();
        service
            .create_owned_thread(
                AgentToolSource {
                    caller: recovery_caller_in_org(org, &format!("create-child-{org}")),
                    source: parent.clone(),
                    execution: parent_execution.clone(),
                },
                child.clone(),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        holds.push(
            hold_recovery_turn_in_org(
                &service,
                &storage,
                &provider,
                &child,
                org,
                &format!("child-{org}"),
            )
            .await,
        );
        let stopper = recovery_caller_in_org(org, &format!("stop-{org}"));
        let faulty: Arc<dyn commands::CreationStorage> = Arc::new(FaultyCommands {
            storage: storage.clone(),
            target: parent.clone(),
            commit: true,
            stage: MutationStage::Accepted,
            once: Arc::new(AtomicBool::new(true)),
        });
        assert!(service
            .stop_command(
                faulty,
                parent.clone(),
                stopper.clone(),
                parent_execution.clone()
            )
            .await
            .is_err());
        selected.push((parent, child, stopper, parent_execution));
    }
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    let retiring = {
        let service = service.clone();
        tokio::spawn(async move { service.shutdown().await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.close_calls.load(Ordering::SeqCst) < 4 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(holds);
    retiring.await.unwrap().unwrap();
    drop(service);
    storage.shutdown().await.unwrap();
    drop((storage, metadata));

    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, false).await;
    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let snapshot = owned.read().await.unwrap();
    assert_eq!(service.recover_parent_stops(&snapshot).await.unwrap(), 2);
    for (parent, child, stopper, execution) in &selected {
        let binding = ConversationService::stop_lookup(parent, stopper, execution).unwrap();
        let mut last = String::new();
        let completed = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let found = service
                    .lookup_command(
                        storage.clone(),
                        parent.clone(),
                        stopper.clone(),
                        binding.clone(),
                    )
                    .await;
                last = format!("{found:?}");
                if found.is_ok_and(|receipt| {
                    receipt.is_some_and(|receipt| receipt.stage() == MutationStage::Settled)
                }) {
                    break;
                }
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await;
        assert!(
            completed.is_ok(),
            "org={} last lookup: {last}",
            stopper.organization_id.as_str()
        );
        for target in [parent, child] {
            let live = service.resolve(target, stopper).await.unwrap();
            assert!(live.agent.owed_turns().await.is_empty());
        }
    }
    assert!(provider.executions.lock().unwrap().is_empty());
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn owned_creation_retry_cannot_report_a_sealed_child_as_ready() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let coordinator = bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let held = hold_turn(&service, &provider, &parent, "parent").await;
    let target = child(&service, &parent, "parent", "create-sealed-child").await;
    let live = service
        .resolve(&target, &caller("inspect-child"))
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while live.agent.attachment_status().phase() != AttachmentPhase::Attached {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let lifetime = coordinator
        .child_lifetime_for_session(&conversation_session(&target))
        .unwrap();
    coordinator
        .end_lifetime(CloseCommand {
            lifetime: lifetime.clone(),
            cause: LifetimeCause::HostClose,
            initiator: Initiator::Host(HostActor::new("person", "panel", "close-child").unwrap()),
            external_attachment: false,
            timeout: None,
        })
        .await
        .unwrap();
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    assert!(
        matches!(
            service
                .create_owned_thread(
                    source(&parent, "parent", "create-sealed-child"),
                    target,
                    RequestedConversation::default(),
                )
                .await,
            Err(commands::CreationFailure::Target(
                ConversationError::Unavailable
            ))
        ),
        "a cached successful preparation does not make a sealed child runnable"
    );
    assert_eq!(provider.executions.lock().unwrap().as_slice(), ["parent"]);
    drop((held, live));
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn failed_queued_report_withdrawal_keeps_root_active_and_retries_before_root_stop() {
    struct AfterFault {
        inner: Arc<dyn commands::CreationStorage>,
        once: Arc<AtomicBool>,
        opens: Arc<Mutex<Vec<bool>>>,
    }
    impl commands::CreationStorage for AfterFault {
        fn mutations(
            &self,
            principal: String,
        ) -> commands::CreationFuture<'_, Vec<MutationReceipt>> {
            self.inner.mutations(principal)
        }
        fn open_creation(
            &self,
            principal: String,
            request: String,
            create: bool,
        ) -> commands::CreationFuture<'_, Option<Arc<dyn commands::CreationStorageLease>>> {
            Box::pin(async move {
                let after_fault = !self.once.load(Ordering::SeqCst);
                let result = self.inner.open_creation(principal, request, create).await;
                if after_fault {
                    self.opens.lock().unwrap().push(matches!(
                        &result,
                        Err(commands::CreationStorageError::Storage(StorageError::Busy))
                    ));
                }
                result
            })
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let reports = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(reports.clone());
    let parent = id();
    opened(&service, &provider, &parent).await;
    let held_parent = hold_turn(&service, &provider, &parent, "parent").await;
    let finished_child = child(&service, &parent, "parent", "create-reporter").await;
    let reporting = hold_turn(&service, &provider, &finished_child, "report-source").await;
    service
        .report_thread_turn(
            finished_child.clone(),
            "report-source".into(),
            ThreadReport {
                parent: parent.clone(),
                caller: caller("report-action"),
                execution: "queued-report".into(),
            },
        )
        .await;
    reporting.send(()).unwrap();
    service.look_for_reports(finished_child, false).await;
    let live = service.resolve(&parent, &caller("inspect")).await.unwrap();
    let queued = ExecutionId::new("queued-report").unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !live.agent.queued_ids().await.contains(&queued) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let independent = child(&service, &parent, "parent", "create-independent").await;
    let held_independent = hold_turn(&service, &provider, &independent, "independent").await;
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let (committed, _) = service.committed_with_order(&independent).await.unwrap();
            if committed.snapshot().is_some_and(|history| {
                history.invocations.iter().any(|record| {
                    record.request.execution_id.as_str() == "independent"
                        && owed_turn_ids(&history.invocations)
                            .contains(&record.request.execution_id)
                })
            }) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the independent child is committed before capturing Stop targets");
    let once = Arc::new(AtomicBool::new(true));
    let opens = Arc::new(Mutex::new(Vec::new()));
    let faulty: Arc<dyn commands::CreationStorage> = Arc::new(AfterFault {
        inner: Arc::new(FaultyCommands {
            storage: storage.clone(),
            target: parent.clone(),
            commit: false,
            stage: MutationStage::Attempted,
            once: once.clone(),
        }),
        once: once.clone(),
        opens: opens.clone(),
    });
    // Exercise the same real admission contention that can precede the injected
    // write failure on a loaded runner. That refusal is not the fault boundary.
    let principal_lease = tokio::time::timeout(TURN_HOLD_HANG, async {
        loop {
            match storage
                .open_creation("person".into(), "stop-parent".into(), true)
                .await
            {
                Ok(Some(lease)) => break lease,
                Err(commands::CreationStorageError::Storage(StorageError::Busy)) => {
                    tokio::task::yield_now().await;
                }
                Ok(None) => panic!("command admission stream is absent"),
                Err(error) => panic!("cannot hold command admission: {error:?}"),
            }
        }
    })
    .await
    .expect("command admission becomes available");
    assert!(matches!(
        service
            .stop_command(
                faulty.clone(),
                parent.clone(),
                caller("stop-parent"),
                "parent".into()
            )
            .await,
        Err(commands::MutationFailure::Storage(
            commands::CreationStorageError::Storage(StorageError::Busy)
        ))
    ));
    assert!(
        once.load(Ordering::SeqCst),
        "admission Busy did not reach the injected write"
    );
    assert!(
        opens.lock().unwrap().is_empty(),
        "pre-fault Busy is not a post-fault independent attempt"
    );
    drop(principal_lease);
    let failure = tokio::time::timeout(TURN_HOLD_HANG, async {
        loop {
            match service
                .stop_command(
                    faulty.clone(),
                    parent.clone(),
                    caller("stop-parent"),
                    "parent".into(),
                )
                .await
            {
                Err(commands::MutationFailure::Storage(
                    commands::CreationStorageError::Storage(StorageError::Busy),
                )) if once.load(Ordering::SeqCst) => tokio::task::yield_now().await,
                Err(error) => break error,
                Ok(_) => panic!("the selected report withdrawal must reach the injected failure"),
            }
        }
    })
    .await
    .expect("the original Stop reaches its selected write fault");
    assert!(
        !once.load(Ordering::SeqCst),
        "the fault must fire before testing post-fault effects"
    );
    assert!(
        matches!(
            failure,
            commands::MutationFailure::Storage(commands::CreationStorageError::Storage(
                StorageError::Corrupt(_)
            ))
        ),
        "unexpected fault result: {failure:?}"
    );
    if provider
        .cancel_turns
        .lock()
        .unwrap()
        .iter()
        .any(|id| id == "parent")
    {
        drop(held_parent);
        tokio::time::timeout(Duration::from_secs(5), async {
            while !provider
                .executions
                .lock()
                .unwrap()
                .iter()
                .any(|id| id == "queued-report")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .expect("the failed prerequisite exposes queued report dispatch after root settlement");
        panic!("root cancellation executed its queued report after withdrawal failed");
    }
    assert_eq!(
        live.agent
            .active_execution_id()
            .as_ref()
            .map(ExecutionId::as_str),
        Some("parent"),
        "an unconfirmed report withdrawal must not release the root invocation slot"
    );
    assert!(live.agent.queued_ids().await.contains(&queued));
    // Background report polling uses the original storage, not this wrapper.
    // With root withheld, this next command open proves the independent attempt.
    assert_eq!(opens.lock().unwrap().len(), 1);
    if provider.cancel_turns.lock().unwrap().is_empty() {
        assert_eq!(
            opens.lock().unwrap().as_slice(),
            [true],
            "an independent attempt without a provider effect encountered typed storage Busy"
        );
    } else {
        assert_eq!(
            provider.cancel_turns.lock().unwrap().as_slice(),
            ["independent"]
        );
    }
    assert_eq!(reports.pending().await.unwrap().len(), 1);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match service
                .stop_command(
                    faulty.clone(),
                    parent.clone(),
                    caller("stop-parent"),
                    "parent".into(),
                )
                .await
            {
                Ok(_) => break,
                Err(commands::MutationFailure::Storage(
                    commands::CreationStorageError::Storage(StorageError::Busy),
                )) => tokio::task::yield_now().await,
                Err(error) => panic!("exact retry failed: {error:?}"),
            }
        }
    })
    .await
    .expect("exact retry settles after transient command-stream contention");
    assert!(!provider
        .executions
        .lock()
        .unwrap()
        .iter()
        .any(|id| id == "queued-report"));
    let mut cancelled = provider.cancel_turns.lock().unwrap().clone();
    cancelled.sort();
    assert_eq!(cancelled, ["independent", "parent"]);
    assert_eq!(
        reports.pending().await.unwrap().len(),
        1,
        "the withdrawn report remains durable until later person input"
    );
    drop((held_parent, held_independent, live));
    retire(service, storage, metadata).await;
}

struct HoldPreparationAdmission {
    durable: DurableOwnershipAudit,
    entered: Notify,
    release: Notify,
}
#[async_trait::async_trait]
impl OwnershipAudit for HoldPreparationAdmission {
    async fn record(&self, evidence: &OwnershipEvidence) -> Result<(), PortFailure> {
        if evidence.after == OwnershipMeaning::Reserved {
            self.entered.notify_one();
            self.release.notified().await;
        }
        OwnershipAudit::record(&self.durable, evidence).await
    }
}

#[tokio::test]
async fn failed_child_preparation_cannot_claim_a_foreign_live_slot_during_storage_contention() {
    for busy in [false, true] {
        let directory = tempfile::tempdir().unwrap();
        let root = directory.path().join("data");
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture_with_sessions(
            &root,
            provider.clone(),
            crate::conversation::infrastructure::in_process_environment().into(),
            crate::conversation_test_support::access_policy(),
            crate::conversation_test_support::claude_erasers(),
        );
        service.bind_commands(storage.clone());
        let unrelated = id();
        let mut foreign = caller("foreign-create");
        foreign.principal_id = PrincipalId::new("foreign").unwrap();
        service
            .create(
                unrelated.clone(),
                foreign.clone(),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        let original = metadata.load(&unrelated).await.unwrap().unwrap();
        let (unrelated_release, gate) = oneshot::channel();
        *provider.execution_gate.lock().unwrap() = Some(gate);
        let started = provider.execution_started.notified();
        service
            .submit(
                unrelated.clone(),
                foreign.clone(),
                "foreign-active".into(),
                message("foreign work"),
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();

        let audit = Arc::new(HoldPreparationAdmission {
            durable: DurableOwnershipAudit::new(root.join("ownership-audit"), Arc::new(TestClock))
                .unwrap(),
            entered: Notify::new(),
            release: Notify::new(),
        });
        let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
            store: Arc::new(SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap()),
            audit: audit.clone(),
            factory: service.owned_thread_factory(),
            room: Arc::new(LiveCapacity::new(32)),
        });
        coordinator.resume().await.unwrap();
        service
            .bind_owned_threads(coordinator.clone(), true)
            .unwrap();
        let parent = id();
        opened(&service, &provider, &parent).await;
        let parent_release = hold_turn(&service, &provider, &parent, "parent").await;
        let preparing = tokio::spawn({
            let service = service.clone();
            let unrelated = unrelated.clone();
            let parent = parent.clone();
            async move {
                service
                    .create_owned_thread(
                        source(&parent, "parent", "foreign-collision"),
                        unrelated,
                        RequestedConversation::default(),
                    )
                    .await
            }
        });
        tokio::time::timeout(TURN_HOLD_HANG, audit.entered.notified())
            .await
            .unwrap();
        // Hold the principal stream only after source admission, so the fault
        // occurs in the factory's command owner, not before child reservation.
        let principal_lease = if busy {
            storage
                .open_creation("person".into(), "foreign-collision".into(), true)
                .await
                .unwrap()
        } else {
            None
        };
        audit.release.notify_one();
        assert!(tokio::time::timeout(TURN_HOLD_HANG, preparing)
            .await
            .unwrap()
            .unwrap()
            .is_err());
        drop(principal_lease);
        assert_eq!(
            provider.close_calls.load(Ordering::SeqCst),
            0,
            "a failure diagnostic cannot transfer another caller's live slot"
        );
        assert!(coordinator
            .active_lifetime_for_session(&conversation_session(&unrelated))
            .is_none());
        drop(parent_release);
        assert!(service
            .delete(parent, caller("delete-parent"))
            .await
            .unwrap());
        assert_eq!(
            provider.close_calls.load(Ordering::SeqCst),
            1,
            "parent cleanup owns only the parent attachment"
        );
        assert_eq!(metadata.load(&unrelated).await.unwrap().unwrap(), original);
        let live = service.resolve(&unrelated, &foreign).await.unwrap();
        assert_eq!(
            live.agent.active_execution_id().unwrap().as_str(),
            "foreign-active"
        );
        drop(live);
        drop(unrelated_release);
        service
            .close(unrelated.clone(), foreign.clone())
            .await
            .unwrap();
        service
            .create(
                unrelated.clone(),
                foreign.clone(),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        service
            .submit(
                unrelated.clone(),
                foreign.clone(),
                "foreign-next".into(),
                message("new foreign work"),
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        assert!(
            service
                .resolve(&unrelated, &foreign)
                .await
                .unwrap()
                .agent
                .attachment_status()
                .phase()
                == AttachmentPhase::Attached
        );
        retire(service, storage, metadata).await;
    }
}

struct RejectChildReadyCommands {
    storage: Arc<RecordStorage>,
    child: ConversationId,
}
struct RejectChildReadyLease {
    inner: Arc<dyn commands::CreationStorageLease>,
    child: ConversationId,
}
impl commands::CreationStorage for RejectChildReadyCommands {
    fn mutations(&self, principal: String) -> commands::CreationFuture<'_, Vec<MutationReceipt>> {
        self.storage.mutations(principal)
    }
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> commands::CreationFuture<'_, Option<Arc<dyn commands::CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .storage
                .open_creation(principal, request, create)
                .await?
                .map(|inner| {
                    Arc::new(RejectChildReadyLease {
                        inner,
                        child: self.child.clone(),
                    }) as Arc<dyn commands::CreationStorageLease>
                }))
        })
    }
}
impl commands::CreationStorageLease for RejectChildReadyLease {
    fn load(
        &self,
        request: &str,
    ) -> commands::CreationFuture<'_, Option<commands::CreationReceipt>> {
        self.inner.load(request)
    }
    fn save(&self, receipt: commands::CreationReceipt) -> commands::CreationFuture<'_, ()> {
        Box::pin(async move {
            if receipt.binding().target() == &conversation_session(&self.child)
                && receipt.stage() == commands::CreationStage::Ready
            {
                return Err(commands::CreationStorageError::Storage(StorageError::Io(
                    "injected rejection before child Ready append".into(),
                )));
            }
            self.inner.save(receipt).await
        })
    }
    fn load_mutation(
        &self,
        request: &str,
    ) -> commands::CreationFuture<'_, Option<MutationReceipt>> {
        self.inner.load_mutation(request)
    }
    fn save_mutation(&self, receipt: MutationReceipt) -> commands::CreationFuture<'_, ()> {
        self.inner.save_mutation(receipt)
    }
}

struct CaptureReservedOwnership {
    inner: Arc<SqliteOwnershipStore>,
    reserved: Mutex<Option<OwnershipSnapshot>>,
}
#[async_trait::async_trait]
impl OwnershipStore for CaptureReservedOwnership {
    async fn read(&self) -> Result<OwnershipSnapshot, PortFailure> {
        OwnershipStore::read(self.inner.as_ref()).await
    }
    async fn write(&self, snapshot: &OwnershipSnapshot) -> Result<(), PortFailure> {
        if snapshot
            .spawns
            .iter()
            .any(|row| row.progress == SpawnProgress::Reserved)
        {
            self.reserved
                .lock()
                .unwrap()
                .get_or_insert_with(|| snapshot.clone());
        }
        OwnershipStore::write(self.inner.as_ref(), snapshot).await
    }
}

#[tokio::test]
async fn startup_retains_attempted_child_creation_without_binding_or_replaying_it() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let child = id();
    service.bind_commands(Arc::new(RejectChildReadyCommands {
        storage: storage.clone(),
        child: child.clone(),
    }));
    let owned = Arc::new(SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap());
    let captured = Arc::new(CaptureReservedOwnership {
        inner: owned.clone(),
        reserved: Mutex::new(None),
    });
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: captured.clone(),
        audit: Arc::new(
            DurableOwnershipAudit::new(root.join("ownership-audit"), Arc::new(TestClock)).unwrap(),
        ),
        factory: service.owned_thread_factory(),
        room: Arc::new(LiveCapacity::new(32)),
    });
    coordinator.resume().await.unwrap();
    service
        .bind_owned_threads(coordinator.clone(), true)
        .unwrap();
    let parent = id();
    opened(&service, &provider, &parent).await;
    let parent_release = hold_turn(&service, &provider, &parent, "parent").await;
    assert!(service
        .create_owned_thread(
            source(&parent, "parent", "attempted-child"),
            child.clone(),
            RequestedConversation::default()
        )
        .await
        .is_err());
    assert!(metadata.load(&child).await.unwrap().is_some());
    let lease = storage
        .open_creation("person".into(), "attempted-child".into(), false)
        .await
        .unwrap()
        .unwrap();
    assert!(lease
        .load("attempted-child")
        .await
        .unwrap()
        .is_some_and(|receipt| receipt.stage() == commands::CreationStage::Attempted));
    drop(lease);
    let crash_image = captured
        .reserved
        .lock()
        .unwrap()
        .clone()
        .expect("actual acknowledged SDK Reserved prefix");
    let child_lifetime = crash_image.spawns[0].child_lifetime.clone();
    drop(parent_release);
    retire(service, storage, metadata).await;
    drop(coordinator);
    drop(captured);
    // Reconstruct a crash while the factory had acquired its real target, but
    // before SDK preparation publication. Both retained facts came from their
    // production owners; no lifetime, actor or physical release is fabricated.
    owned.write(&crash_image).await.unwrap();
    drop(owned);

    for restart in 0..2 {
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, metadata) = fixture(&root, provider.clone());
        let coordinator = bind(&service, &storage, &root, true).await;
        assert!(
            !coordinator.has_bound_resources(&child_lifetime),
            "Attempted does not authorize recovery to invent a cleanup owner"
        );
        assert!(service
            .resolve(&child, &caller("inspect-unconfirmed"))
            .await
            .is_err());
        assert_eq!(
            provider.open_calls.load(Ordering::SeqCst),
            0,
            "unconfirmed child recovery does not open a provider attachment"
        );
        assert!(
            provider.executions.lock().unwrap().is_empty(),
            "recovery does not replay the child's uncertain initialization"
        );
        let unrelated = id();
        opened(&service, &provider, &unrelated).await;
        finish_turn(
            &service,
            &provider,
            &unrelated,
            &format!("unrelated-{restart}"),
        )
        .await;
        assert!(provider
            .executions
            .lock()
            .unwrap()
            .iter()
            .any(|id| id == &format!("unrelated-{restart}")));
        retire(service, storage, metadata).await;
    }
}

#[tokio::test]
async fn restart_preserves_child_capacity_without_preventing_its_cold_reopen() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind_with_capacity(&service, &storage, &root, true, 1).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let parent_release = hold_turn(&service, &provider, &parent, "parent").await;
    let retained = child(&service, &parent, "parent", "retained-child").await;
    drop(parent_release);
    retire(service, storage, metadata).await;

    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind_with_capacity(&service, &storage, &root, true, 1).await;
    opened(&service, &provider, &parent).await;
    let parent_release = hold_turn(&service, &provider, &parent, "after-restart").await;
    let refused = id();
    assert!(
        matches!(
            service
                .create_owned_thread(
                    source(&parent, "after-restart", "over-capacity"),
                    refused.clone(),
                    RequestedConversation::default(),
                )
                .await,
            Err(CreationFailure::Target(ConversationError::Unavailable))
        ),
        "the retained child's lifetime still occupies the single child slot"
    );
    assert!(metadata.load(&refused).await.unwrap().is_none());
    let ownership = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    assert_eq!(ownership.read().await.unwrap().spawns.len(), 1);
    drop(ownership);
    opened(&service, &provider, &retained).await;
    finish_turn(&service, &provider, &retained, "retained-after-restart").await;
    drop(parent_release);
    retire(service, storage, metadata).await;
}

#[path = "owned_recovery.rs"]
mod owned_recovery;

#[tokio::test]
async fn parent_stop_captures_the_last_permitted_running_successor() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let parent = id();
    let child_id;
    {
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        bind(&service, &storage, &root, true).await;
        opened(&service, &provider, &parent).await;
        let p = hold_turn(&service, &provider, &parent, "parent-original").await;
        child_id = child(&service, &parent, "parent-original", "create-child").await;
        let c = hold_turn(&service, &provider, &child_id, "child-original").await;
        suspend_child_turn(&service, &provider, &child_id, c).await;
        drop(p);
        retire(service, storage, metadata).await;
    }
    for attempt in 1..=Resumption::MAX_RESUMES {
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        let coordinator = bind(&service, &storage, &root, true).await;
        let live = service
            .resolve(&child_id, &caller("read-child"))
            .await
            .unwrap();
        let owed = live.agent.owed_turns().await.remove(0);
        let (release, gate) = oneshot::channel();
        *provider.execution_gate.lock().unwrap() = Some(gate);
        let started = provider.execution_started.notified();
        let Resumed::Continued(admission) = live
            .agent
            .resume_turn(&owed, caller("resume").actor().unwrap())
            .await
            .unwrap()
        else {
            panic!("successor is owed")
        };
        tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();
        let current = admission.id().clone();
        assert_eq!(Resumption::of(&current).attempt(), attempt);
        if attempt == Resumption::MAX_RESUMES {
            let p = hold_turn(&service, &provider, &parent, "parent-current").await;
            service
                .stop_command(
                    storage.clone(),
                    parent.clone(),
                    caller("stop-parent"),
                    "parent-current".into(),
                )
                .await
                .unwrap();
            let cancelled = provider.cancel_turns.lock().unwrap().clone();
            assert!(
                cancelled.contains(&current.as_str().to_owned()),
                "last allowed successor omitted: {current:?}; cancelled={cancelled:?}"
            );
            let lifetime = coordinator
                .active_lifetime_for_session(&conversation_session(&child_id))
                .unwrap();
            assert_eq!(
                coordinator.lifetime_state(&lifetime),
                Some(LifetimeState::Open)
            );
            assert!(live.agent.owed_turns().await.is_empty());
            drop(p);
        }
        if attempt < Resumption::MAX_RESUMES {
            suspend_child_turn(&service, &provider, &child_id, release).await;
        } else {
            drop(release);
        }
        drop(live);
        retire(service, storage, metadata).await;
    }
}

pub(super) async fn suspend_child_turn(
    service: &ConversationService,
    provider: &ProviderFactory,
    child: &ConversationId,
    release: oneshot::Sender<()>,
) {
    let live = service
        .resolve(child, &caller("suspend-read"))
        .await
        .unwrap();
    *provider.execution_updates.lock().unwrap() =
        vec![ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)];
    *provider.execution_reply.lock().unwrap() = Some(ProviderExecutionReply::Finished(
        ExecutionReport::cancelled_locally(CleanupReport::confirmed(CloseOutcome {
            forced: false,
        })),
    ));
    let stopping = {
        let live = live.clone();
        tokio::spawn(async move {
            live.agent
                .suspend(caller("host-shutdown").actor().unwrap())
                .await
        })
    };
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while live.agent.attachment_status().phase() == AttachmentPhase::Attached {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    drop(release);
    tokio::time::timeout(TURN_HOLD_HANG, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
}

struct PausedPendingReports {
    inner: Arc<dyn ThreadReportStore>,
    settled: Notify,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}
impl ThreadReportStore for PausedPendingReports {
    fn save(&self, report: &PendingReport) -> ConversationFuture<'_, bool> {
        self.inner.save(report)
    }
    fn settle(&self, child: &ConversationId, report: &str) -> ConversationFuture<'_, ()> {
        let child = child.clone();
        let report = report.to_owned();
        Box::pin(async move {
            self.inner.settle(&child, &report).await?;
            self.settled.notify_one();
            Ok(())
        })
    }
    fn forget_conversation(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        self.inner.forget_conversation(id)
    }
    fn pending(&self) -> ConversationFuture<'_, Vec<PendingReport>> {
        let gate = self.gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            self.inner.pending().await
        })
    }
}

#[tokio::test]
async fn parent_stop_rejects_a_saved_selection_that_repeats_its_root_turn() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let work = hold_turn(&service, &provider, &parent, "parent").await;
    let origin = caller("stop-parent");
    let base = stop_binding(&parent, &origin, "parent").unwrap();
    let prefix = format!(
        "parent-stop-{:x}-",
        Sha256::digest(
            serde_json::to_vec(&(
                base.actor().principal_id(),
                base.actor().surface_id(),
                base.actor().request_id(),
                base.target().as_str(),
                base.turn_id().as_str(),
            ))
            .unwrap()
        )
    );
    let mut prerequisite_caller = origin.clone();
    prerequisite_caller.action_id = format!("{prefix}0000");
    let prerequisite = stop_binding(&parent, &prerequisite_caller, "parent").unwrap();
    let mut hash = Sha256::new();
    hash.update(b"nessa.gateway.parent-stop\0");
    hash.update(base.fingerprint());
    hash.update(
        serde_json::to_vec(&(
            prerequisite.actor().principal_id(),
            prerequisite.actor().surface_id(),
            prerequisite.actor().request_id(),
            prerequisite.target().as_str(),
            prerequisite.turn_id().as_str(),
            prerequisite.fingerprint(),
        ))
        .unwrap(),
    );
    let selected = MutationBinding::new(
        base.actor().clone(),
        MutationOperation::Stop,
        base.target().clone(),
        base.turn_id().as_str(),
        hash.finalize().into(),
    )
    .unwrap();
    for binding in [prerequisite, selected] {
        let lease = storage
            .open_creation(
                binding.actor().principal_id().into(),
                binding.actor().request_id().into(),
                true,
            )
            .await
            .unwrap()
            .unwrap();
        lease
            .save_mutation(MutationReceipt::accepted(binding))
            .await
            .unwrap();
    }
    let before = storage
        .mutations(origin.principal_id.as_str().into())
        .await
        .unwrap();
    let result = service
        .stop_command(
            storage.clone(),
            parent.clone(),
            origin.clone(),
            "parent".into(),
        )
        .await;
    let after = storage
        .mutations(origin.principal_id.as_str().into())
        .await
        .unwrap();
    let live = service.resolve(&parent, &caller("inspect")).await.unwrap();
    let active = live.agent.active_execution_id();
    let cancelled = provider.cancel_turns.lock().unwrap().clone();
    drop((work, live));
    retire(service, storage, metadata).await;
    assert!(matches!(result, Err(MutationFailure::Conflict)));
    assert_eq!(active, Some(ExecutionId::new("parent").unwrap()));
    assert!(cancelled.is_empty());
    assert_eq!(
        before, after,
        "invalid acknowledged selections are not rewritten"
    );
}

#[tokio::test]
async fn explicit_report_turn_stop_owns_queued_withdrawal() {
    explicit_report_turn_stop(false, MutationOutcome::Withdrawn).await;
}

#[tokio::test]
async fn explicit_report_turn_stop_owns_active_cancellation() {
    explicit_report_turn_stop(true, MutationOutcome::Cancelled).await;
}

async fn explicit_report_turn_stop(active: bool, expected: MutationOutcome) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let retained = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    service.bind_thread_reports(retained.clone());
    let parent = id();
    opened(&service, &provider, &parent).await;
    let mut parent_work = Some(hold_turn(&service, &provider, &parent, "parent").await);
    let child_id = child(&service, &parent, "parent", "create-reporter").await;
    let child_work = hold_turn(&service, &provider, &child_id, "child").await;
    service
        .report_thread_turn(
            child_id.clone(),
            "child".into(),
            ThreadReport {
                parent: parent.clone(),
                caller: caller("report-action"),
                execution: "requested-report".into(),
            },
        )
        .await;
    child_work.send(()).unwrap();
    service.look_for_reports(child_id, false).await;
    let live = service.resolve(&parent, &caller("inspect")).await.unwrap();
    let report_id = ExecutionId::new("requested-report").unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while !live.agent.queued_ids().await.contains(&report_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (report_work, report_gate) = oneshot::channel();
    if active {
        *provider.execution_gate.lock().unwrap() = Some(report_gate);
        let started = provider.execution_started.notified();
        parent_work.take().unwrap().send(()).unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();
        assert_eq!(live.agent.active_execution_id(), Some(report_id.clone()));
    } else {
        drop(report_gate);
    }
    let receipt = service
        .stop_command(
            storage.clone(),
            parent.clone(),
            caller("stop-report"),
            report_id.as_str().into(),
        )
        .await;
    let retried = service
        .stop_command(
            storage.clone(),
            parent.clone(),
            caller("stop-report"),
            report_id.as_str().into(),
        )
        .await;
    let root_stops: Vec<_> = storage
        .mutations(caller("stop-report").principal_id.as_str().into())
        .await
        .unwrap()
        .into_iter()
        .filter(|receipt| {
            let binding = receipt.binding();
            binding.operation() == MutationOperation::Stop
                && binding.target() == &conversation_session(&parent)
                && binding.turn_id() == &report_id
        })
        .collect();
    let cancelled = provider.cancel_turns.lock().unwrap().clone();
    let report_retained = retained.pending().await.unwrap().len();
    let unrelated_active = live.agent.active_execution_id();
    drop((parent_work, report_work, live));
    retire(service, storage, metadata).await;
    assert_eq!(root_stops.len(), 1, "one binding owns the requested turn");
    assert_eq!(root_stops[0].binding().actor().request_id(), "stop-report");
    let receipt = receipt.unwrap();
    let retried = retried.unwrap();
    assert_eq!(receipt.outcome(), Some(expected));
    assert_eq!(retried, receipt, "retry returns the root's actual effect");
    assert_eq!(
        cancelled
            .iter()
            .filter(|turn| *turn == "requested-report")
            .count(),
        usize::from(active)
    );
    assert!(!cancelled.iter().any(|turn| turn == "parent"));
    if !active {
        assert_eq!(unrelated_active, Some(ExecutionId::new("parent").unwrap()));
    }
    assert_eq!(
        report_retained, 1,
        "the cancelled report obligation remains durable"
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_captures_correlated_report_after_dequeue_active() {
    correlated_report_after_dequeue(ReportPhase::Active).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_captures_correlated_report_after_dequeue_handoff() {
    correlated_report_after_dequeue(ReportPhase::Handoff).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_captures_correlated_report_owed_after_dequeue() {
    correlated_report_after_dequeue(ReportPhase::Owed).await;
}
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_bounds_a_captured_reports_stalled_handoff_and_retries_its_selection() {
    correlated_report_after_dequeue(ReportPhase::StalledHandoff).await;
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum ReportPhase {
    Active,
    Handoff,
    StalledHandoff,
    Owed,
}
async fn correlated_report_after_dequeue(phase: ReportPhase) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let observed = Arc::new(PausedMutationObservation {
        inner: storage.clone(),
        gate: Default::default(),
    });
    service.bind_commands(observed.clone());
    bind(&service, &storage, &root, true).await;
    let retained = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    let reports = Arc::new(PausedPendingReports {
        inner: retained.clone(),
        settled: Notify::new(),
        gate: Default::default(),
    });
    service.bind_thread_reports(reports.clone());
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let child_id = child(&service, &parent, "parent", "create-reporter").await;
    let c = hold_turn(&service, &provider, &child_id, "child").await;
    let independent_work = if phase == ReportPhase::StalledHandoff {
        let independent = child(&service, &parent, "parent", "create-independent").await;
        Some(hold_turn(&service, &provider, &independent, "independent").await)
    } else {
        None
    };
    service
        .report_thread_turn(
            child_id.clone(),
            "child".into(),
            ThreadReport {
                parent: parent.clone(),
                caller: caller("report-action"),
                execution: "correlated-report".into(),
            },
        )
        .await;
    c.send(()).unwrap();
    service.look_for_reports(child_id, false).await;
    let live = service.resolve(&parent, &caller("inspect")).await.unwrap();
    let report_id = ExecutionId::new("correlated-report").unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, async {
        while !live.agent.queued_ids().await.contains(&report_id) {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let (entered, waiting) = oneshot::channel();
    let (continue_selection, gate) = oneshot::channel();
    *reports.gate.lock().unwrap() = Some((entered, gate));
    let mut stopping = {
        let service = service.clone();
        let storage = observed.clone();
        let parent = parent.clone();
        tokio::spawn(async move {
            service
                .stop_command(storage, parent, caller("stop-parent"), "parent".into())
                .await
        })
    };
    tokio::time::timeout(TURN_HOLD_HANG, waiting)
        .await
        .unwrap()
        .unwrap();
    // The original root was classified Effect. Its existing SDK runner can
    // now dequeue the correlated report despite gateway admission fencing.
    let (finish_report, report_gate) = oneshot::channel();
    if phase != ReportPhase::Owed {
        *provider.execution_gate.lock().unwrap() = Some(report_gate);
    } else {
        drop(report_gate);
    }
    let prepared = Arc::new(Notify::new());
    let (continue_prepare, prepare_gate) = oneshot::channel();
    let mut continue_prepare = Some(continue_prepare);
    if phase == ReportPhase::Owed {
        *provider.prepare_failure.lock().unwrap() =
            Some(AgentError::Transport("report preparation failed".into()));
    }
    if phase != ReportPhase::Active {
        *provider.prepare_gate.lock().unwrap() = Some((prepared.clone(), prepare_gate));
    } else {
        drop(prepare_gate);
    }
    let began = provider.execution_started.notified();
    p.send(()).unwrap();
    if phase != ReportPhase::Active {
        tokio::time::timeout(TURN_HOLD_HANG, prepared.notified())
            .await
            .unwrap();
    } else {
        tokio::time::timeout(TURN_HOLD_HANG, provider.execution_started.notified())
            .await
            .unwrap();
    }
    if phase == ReportPhase::Owed {
        continue_prepare.take().unwrap().send(()).unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, async {
            while !live
                .agent
                .owed_turns()
                .await
                .iter()
                .any(|turn| turn.execution_id() == &report_id)
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
    }
    assert!(!live.agent.queued_ids().await.contains(&report_id));
    assert_eq!(
        live.agent
            .unsettled_execution_ids()
            .await
            .contains(&report_id),
        phase != ReportPhase::Owed
    );
    let (captured, after_capture) = oneshot::channel();
    let (allow_control, control_gate) = oneshot::channel();
    *observed.gate.lock().unwrap() = Some((captured, control_gate));
    continue_selection.send(()).unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, after_capture)
        .await
        .unwrap()
        .unwrap();
    // This observation occurs after the unfinished-ID snapshot. Keep
    // provider preparation paused until capture has actually happened.
    if phase == ReportPhase::Handoff {
        assert!(live.agent.active_execution_id().is_none());
        continue_prepare.take().unwrap().send(()).unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, began).await.unwrap();
    }
    allow_control.send(()).unwrap();
    let mut stalled = None;
    let stopped = if phase == ReportPhase::StalledHandoff {
        // The real observation barrier is after capture, and preparation stays
        // held until Stop's existing mutation owner must have answered.
        let first = tokio::time::timeout(
            super::super::MOVING_TURN_WAIT + Duration::from_secs(2),
            &mut stopping,
        )
        .await;
        let bounded = matches!(
            &first,
            Ok(Ok(Err(MutationFailure::Target(ConversationError::Agent(
                AgentError::SubmissionUnresolved
            )))))
        );
        let exact_still_owned = live
            .agent
            .unsettled_execution_ids()
            .await
            .contains(&report_id)
            && live.agent.active_execution_id().is_none()
            && !live.agent.queued_ids().await.contains(&report_id);
        let receipts = storage
            .mutations(caller("stop-parent").principal_id.as_str().into())
            .await
            .unwrap();
        let original_accepted = receipts.iter().any(|receipt| {
            receipt.binding().turn_id() == &report_id && receipt.stage() == MutationStage::Accepted
        });
        let report_retained = retained.pending().await.unwrap().len() == 1;
        let independent_attempted = provider
            .cancel_turns
            .lock()
            .unwrap()
            .iter()
            .any(|turn| turn == "independent");
        let admission_free = if bounded {
            tokio::time::timeout(Duration::from_secs(1), service.hold_for_export())
                .await
                .is_ok_and(|hold| hold.is_ok())
        } else {
            false
        };
        // Release and join even under the original unbounded-loop regression.
        // Late preparation never authorizes a newly captured Stop selection.
        let late_began = provider.execution_started.notified();
        continue_prepare.take().unwrap().send(()).unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, late_began)
            .await
            .unwrap();
        match first {
            Ok(joined) => {
                let _original = joined.unwrap();
            }
            Err(_) => {
                let _late = tokio::time::timeout(TURN_HOLD_HANG, &mut stopping)
                    .await
                    .unwrap()
                    .unwrap();
            }
        }
        stalled = Some((
            bounded,
            exact_still_owned,
            original_accepted,
            report_retained,
            admission_free,
            independent_attempted,
        ));
        service
            .stop_command(
                observed.clone(),
                parent.clone(),
                caller("stop-parent"),
                "parent".into(),
            )
            .await
    } else {
        tokio::time::timeout(TURN_HOLD_HANG, stopping)
            .await
            .unwrap()
            .unwrap()
    };
    assert!(
        stopped.is_ok(),
        "exact report stop must settle: {stopped:?}"
    );
    let cancelled = provider.cancel_turns.lock().unwrap().clone();
    if phase == ReportPhase::Owed {
        assert!(
            !live
                .agent
                .owed_turns()
                .await
                .iter()
                .any(|turn| turn.execution_id() == &report_id),
            "owed correlated report escaped Stop"
        );
        assert!(
            cancelled.is_empty(),
            "an already failed report needs decline, not provider cancellation"
        );
    } else {
        assert!(
            cancelled.contains(&"correlated-report".into()),
            "dequeued report escaped Stop ({phase:?}): {cancelled:?}"
        );
    }
    if phase == ReportPhase::Owed {
        assert_report_poll_retains(&service, &observed, &reports, &retained).await;
    }
    assert_eq!(
        retained.pending().await.unwrap().len(),
        1,
        "cancelled report remains durable"
    );
    drop(finish_report);
    drop(independent_work);
    drop(live);
    retire(service, storage, metadata).await;
    if let Some((bounded, owned, accepted, retained, admission_free, independent)) = stalled {
        assert!(
            bounded,
            "captured handoff bypassed the mutation owner's bound"
        );
        assert!(
            independent,
            "a stalled report does not skip the independent captured child"
        );
        assert!(owned, "a bounded refusal preserves the exact held SDK work");
        assert!(
            accepted,
            "bounded classification cannot authorize an Attempted effect"
        );
        assert!(
            retained,
            "the durable report obligation survives bounded refusal"
        );
        assert!(
            admission_free,
            "bounded Stop releases service admission before preparation ends"
        );
    }
    if phase == ReportPhase::Owed {
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, metadata) = fixture(&root, provider.clone());
        let observed = Arc::new(PausedMutationObservation {
            inner: storage.clone(),
            gate: Default::default(),
        });
        service.bind_commands(observed.clone());
        bind(&service, &storage, &root, true).await;
        let retained = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
        let reports = Arc::new(PausedPendingReports {
            inner: retained.clone(),
            settled: Notify::new(),
            gate: Default::default(),
        });
        service.bind_thread_reports(reports.clone());
        assert_eq!(service.resume_thread_reports().await.unwrap(), 1);
        assert_report_poll_retains(&service, &observed, &reports, &retained).await;
        assert!(
            provider.executions.lock().unwrap().is_empty(),
            "restart does not dispatch the stopped report"
        );
        service
            .submit_command(
                storage.clone(),
                parent.clone(),
                caller("later-person"),
                Initiation::owner(),
                false,
                "later-person".into(),
                message("explicit later person input"),
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, reports.settled.notified())
            .await
            .unwrap();
        assert!(retained.pending().await.unwrap().is_empty());
        let live = service
            .resolve(&parent, &caller("inspect-preserved-result"))
            .await
            .unwrap();
        let history = live.agent.session_manager().snapshot().await.unwrap();
        let original = history
            .invocations
            .iter()
            .find(|record| record.request.execution_id.as_str() == "correlated-report")
            .unwrap();
        assert!(
            original.result.as_ref().unwrap().is_err(),
            "host decline preserves the acknowledged provider failure"
        );
        assert!(
            provider
                .executions
                .lock()
                .unwrap()
                .iter()
                .any(|id| id.starts_with("thread-report-")),
            "later person input receives a fresh report delivery"
        );
        assert!(
            !provider
                .executions
                .lock()
                .unwrap()
                .iter()
                .any(|id| id == "correlated-report"),
            "the stopped report identity is never replayed"
        );
        drop(live);
        retire(service, storage, metadata).await;
    }
}

async fn assert_report_poll_retains(
    service: &ConversationService,
    observed: &PausedMutationObservation,
    reports: &PausedPendingReports,
    retained: &LocalThreadReports,
) {
    // Force a real delivery poll after decline, rather than checking the
    // row before its retry timer fires (the Windows CI failure ordering).
    let (polled, poll) = oneshot::channel();
    let (continue_poll, poll_gate) = oneshot::channel();
    *observed.gate.lock().unwrap() = Some((polled, poll_gate));
    tokio::time::timeout(TURN_HOLD_HANG, async {
        tokio::select! {
            _ = reports.settled.notified() => panic!("declined failed report was settled before its pause fence"),
            result = poll => result.unwrap(),
        }
    }).await.unwrap();
    continue_poll.send(()).unwrap();
    // Delivery owns admission through its decision; join the next owner.
    let admission = service.inner.thread_admission.lock().await;
    assert_eq!(
        retained.pending().await.unwrap().len(),
        1,
        "cancelled report remains durable after a delivery poll"
    );
    drop(admission);
}

struct PausedMutationObservation {
    inner: Arc<dyn commands::CreationStorage>,
    gate: Mutex<Option<(oneshot::Sender<()>, oneshot::Receiver<()>)>>,
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_captures_a_restored_reports_queued_canonical_successor() {
    restored_report_successor(false, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_captures_a_restored_reports_active_canonical_successor() {
    restored_report_successor(true, None).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_preserves_a_canonical_successor_with_an_explicit_person_command() {
    restored_report_successor(false, Some(ReportPersonCommand::Original)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_preserves_a_later_successor_after_an_intermediate_person_submit() {
    restored_report_successor(false, Some(ReportPersonCommand::Intermediate)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_preserves_an_active_later_successor_after_an_intermediate_person_submit() {
    restored_report_successor(true, Some(ReportPersonCommand::Intermediate)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_preserves_a_report_successor_with_its_own_person_submit() {
    restored_report_successor(false, Some(ReportPersonCommand::Current)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_captures_a_report_successor_after_a_conflicting_person_submit() {
    restored_report_successor(false, Some(ReportPersonCommand::RejectedCurrent)).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn parent_stop_captures_an_active_report_after_a_conflicting_person_submit() {
    restored_report_successor(true, Some(ReportPersonCommand::RejectedCurrent)).await;
}

#[derive(Clone, Copy)]
enum ReportPersonCommand {
    Original,
    Intermediate,
    Current,
    RejectedCurrent,
}

async fn acknowledge_report_delivery(
    service: &ConversationService,
    storage: &Arc<RecordStorage>,
    parent: &ConversationId,
    execution: &str,
) {
    let live = service
        .resolve(parent, &caller("inspect-report"))
        .await
        .unwrap();
    let history = live.agent.session_manager().snapshot().await.unwrap();
    let record = history
        .invocations
        .iter()
        .find(|record| record.request.execution_id.as_str() == execution)
        .unwrap();
    let mut person = caller("report-action");
    person.surface_id = record.actor.surface_id().into();
    person.action_id = record.actor.request_id().into();
    assert_eq!(person.actor().unwrap(), record.actor);
    let submitted = service
        .submit_command(
            storage.clone(),
            parent.clone(),
            person,
            Initiation::owner(),
            false,
            execution.into(),
            message(record.request.user_message.text_str()),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    assert_eq!(
        submitted.progress.outcome(),
        Some(MutationOutcome::Dispatched)
    );
    let saved = commands::CreationStorage::mutations(storage.as_ref(), "person".into())
        .await
        .unwrap();
    assert!(saved
        .iter()
        .any(|receipt| receipt.binding().turn_id().as_str() == execution
            && receipt.binding().operation() == MutationOperation::Submit
            && receipt.outcome() == Some(MutationOutcome::Dispatched)));
}

async fn restored_report_successor(active: bool, person_command: Option<ReportPersonCommand>) {
    let command_collision = matches!(
        person_command,
        Some(
            ReportPersonCommand::Original
                | ReportPersonCommand::Intermediate
                | ReportPersonCommand::Current
        )
    );
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let parent = id();
    let mut owed_report = "restored-report".to_owned();
    {
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        bind(&service, &storage, &root, true).await;
        let reports = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
        service.bind_thread_reports(reports);
        opened(&service, &provider, &parent).await;
        let p = hold_turn(&service, &provider, &parent, "before-restart").await;
        let child_id = child(&service, &parent, "before-restart", "create-reporter").await;
        let c = hold_turn(&service, &provider, &child_id, "child").await;
        service
            .report_thread_turn(
                child_id.clone(),
                "child".into(),
                ThreadReport {
                    parent: parent.clone(),
                    caller: caller("report-action"),
                    execution: "restored-report".into(),
                },
            )
            .await;
        c.send(()).unwrap();
        service.look_for_reports(child_id, false).await;
        let live = service.resolve(&parent, &caller("read")).await.unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, async {
            while !live
                .agent
                .queued_ids()
                .await
                .iter()
                .any(|id| id.as_str() == "restored-report")
            {
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        if matches!(person_command, Some(ReportPersonCommand::Original)) {
            acknowledge_report_delivery(&service, &storage, &parent, "restored-report").await;
        }
        let (release, gate) = oneshot::channel();
        *provider.execution_gate.lock().unwrap() = Some(gate);
        let started = provider.execution_started.notified();
        p.send(()).unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();
        suspend_child_turn(&service, &provider, &parent, release).await;
        drop(live);
        retire(service, storage, metadata).await;
    }
    if matches!(person_command, Some(ReportPersonCommand::Intermediate)) {
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, metadata) = fixture(&root, provider.clone());
        bind(&service, &storage, &root, true).await;
        service.bind_thread_reports(Arc::new(
            LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap(),
        ));
        let live = service
            .resolve(&parent, &caller("reopen-first-successor"))
            .await
            .unwrap();
        let owed = live
            .agent
            .owed_turns()
            .await
            .into_iter()
            .find(|turn| turn.execution_id().as_str() == owed_report)
            .unwrap();
        let (release, gate) = oneshot::channel();
        *provider.execution_gate.lock().unwrap() = Some(gate);
        let started = provider.execution_started.notified();
        let Resumed::Continued(first) = live
            .agent
            .resume_turn(&owed, caller("resume-first").actor().unwrap())
            .await
            .unwrap()
        else {
            panic!("the original report resumes through the SDK")
        };
        tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();
        owed_report = first.id().as_str().to_owned();
        acknowledge_report_delivery(&service, &storage, &parent, &owed_report).await;
        suspend_child_turn(&service, &provider, &parent, release).await;
        drop((first, live));
        retire(service, storage, metadata).await;
    }
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    bind(&service, &storage, &root, true).await;
    let retained = Arc::new(LocalThreadReports::open(&root.join("reports.sqlite3")).unwrap());
    let reports = Arc::new(PausedPendingReports {
        inner: retained.clone(),
        settled: Notify::new(),
        gate: Default::default(),
    });
    service.bind_thread_reports(reports.clone());
    // The ordinary SDK owns both admissions. Hold gateway admission to make
    // the valid person-before-auto-resume order deterministic after reopening.
    let admission = service.inner.thread_admission.lock().await;
    let live = service.resolve(&parent, &caller("reopen")).await.unwrap();
    let (release_parent, gate) = oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    let started = provider.execution_started.notified();
    let person = live
        .agent
        .enqueue(
            ExecutionRequest {
                execution_id: ExecutionId::new("person-current").unwrap(),
                user_message: UserMessage::text_only(PromptText::new("new person input").unwrap()),
                estimated_input_tokens: 1,
                reserved_output_tokens: 100,
            },
            Submitter::owner(caller("person-current").actor().unwrap()),
        )
        .await
        .unwrap();
    tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();
    let owed = live
        .agent
        .owed_turns()
        .await
        .into_iter()
        .find(|turn| turn.execution_id().as_str() == owed_report)
        .unwrap();
    let Resumed::Continued(successor) = live
        .agent
        .resume_turn(&owed, caller("resume-report").actor().unwrap())
        .await
        .unwrap()
    else {
        panic!("the real suspended report must resume")
    };
    let next = successor.id().clone();
    assert!(live.agent.queued_ids().await.contains(&next));
    let unrelated = live
        .agent
        .enqueue(
            ExecutionRequest {
                execution_id: ExecutionId::new("unrelated-person").unwrap(),
                user_message: UserMessage::text_only(
                    PromptText::new("unrelated queued person").unwrap(),
                ),
                estimated_input_tokens: 1,
                reserved_output_tokens: 100,
            },
            Submitter::owner(caller("unrelated-person").actor().unwrap()),
        )
        .await
        .unwrap();
    let mut release_parent = Some(release_parent);
    let mut release_report = None;
    let mut pending = None;
    if active {
        let (entered, waiting) = oneshot::channel();
        let (resume_capture, gate) = oneshot::channel();
        *reports.gate.lock().unwrap() = Some((entered, gate));
        pending = Some((waiting, resume_capture));
    }
    drop(admission);
    if matches!(person_command, Some(ReportPersonCommand::Current)) {
        acknowledge_report_delivery(&service, &storage, &parent, next.as_str()).await;
    }
    if matches!(person_command, Some(ReportPersonCommand::RejectedCurrent)) {
        let refused = service
            .submit_command(
                storage.clone(),
                parent.clone(),
                caller("conflicting-person"),
                Initiation::owner(),
                false,
                next.as_str().into(),
                message("different person input"),
                SubmissionMode::Queue,
            )
            .await;
        assert!(matches!(
            refused,
            Err(commands::MutationFailure::Target(ConversationError::Agent(
                AgentError::SubmissionConflict
            )))
        ));
        let receipts = commands::CreationStorage::mutations(storage.as_ref(), "person".into())
            .await
            .unwrap();
        let rejected = receipts
            .iter()
            .find(|receipt| receipt.binding().actor().request_id() == "conflicting-person")
            .unwrap();
        assert_eq!(rejected.stage(), commands::MutationStage::Attempted);
        assert_eq!(rejected.outcome(), None);
        assert!(live.agent.queued_ids().await.contains(&next));
    }
    // The provider's cancellation acknowledgement and terminal result are
    // separate: hold the real result until after gateway Stop has selected.
    *provider.execution_updates.lock().unwrap() =
        vec![ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)];
    *provider.execution_reply.lock().unwrap() =
        Some(ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Ok(ExecutionOutcome::Cancelled)),
            None,
            ProviderSessionState::Usable,
        )));
    let stopping = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let parent = parent.clone();
        async move {
            service
                .stop_command(
                    storage,
                    parent,
                    caller("stop-current"),
                    "person-current".into(),
                )
                .await
        }
    });
    if let Some((waiting, resume_capture)) = pending {
        tokio::time::timeout(TURN_HOLD_HANG, waiting)
            .await
            .unwrap()
            .unwrap();
        let (release, gate) = oneshot::channel();
        *provider.execution_gate.lock().unwrap() = Some(gate);
        let started = provider.execution_started.notified();
        *provider.execution_updates.lock().unwrap() = Vec::new();
        *provider.execution_reply.lock().unwrap() = None;
        release_parent.take().unwrap().send(()).unwrap();
        tokio::time::timeout(TURN_HOLD_HANG, started).await.unwrap();
        assert_eq!(live.agent.active_execution_id(), Some(next.clone()));
        if !command_collision {
            *provider.execution_updates.lock().unwrap() =
                vec![ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)];
            *provider.execution_reply.lock().unwrap() =
                Some(ProviderExecutionReply::Finished(ExecutionReport::new(
                    Some(Ok(ExecutionOutcome::Cancelled)),
                    None,
                    ProviderSessionState::Usable,
                )));
        }
        release_report = Some(release);
        resume_capture.send(()).unwrap();
    }
    tokio::time::timeout(TURN_HOLD_HANG, stopping)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    if active {
        assert_eq!(
            provider
                .cancel_turns
                .lock()
                .unwrap()
                .contains(&next.as_str().to_owned()),
            !command_collision,
            "active cancellation follows the full report/person provenance"
        );
        release_report.take().unwrap().send(()).unwrap();
    } else {
        release_parent.take().unwrap().send(()).unwrap();
    }
    let saved = commands::CreationStorage::mutations(storage.as_ref(), "person".into())
        .await
        .unwrap();
    let selected = saved.iter().find(|receipt| {
        receipt.binding().target() == &conversation_session(&parent)
            && receipt.binding().turn_id() == &next
            && receipt.binding().operation() == MutationOperation::Stop
    });
    if command_collision {
        assert!(
            selected.is_none(),
            "an explicit person command cannot become owned report work through resumption"
        );
    } else {
        assert_eq!(
            selected.expect("exact successor Stop receipt").outcome(),
            Some(if active {
                MutationOutcome::Cancelled
            } else {
                MutationOutcome::Withdrawn
            })
        );
    }
    assert_eq!(
        tokio::time::timeout(TURN_HOLD_HANG, successor.wait())
            .await
            .unwrap(),
        if command_collision {
            Ok(ExecutionOutcome::Completed)
        } else if active {
            Ok(ExecutionOutcome::Cancelled)
        } else {
            Err(AgentError::Closed)
        },
        "canonical successor follows report provenance and explicit command authority"
    );
    assert!(
        live.agent.owed_turns().await.is_empty(),
        "person Stop must not become owed report work"
    );
    assert_eq!(
        provider.close_calls.load(Ordering::SeqCst),
        0,
        "ordinary Stop keeps the provider lifetime reusable"
    );
    assert_eq!(
        tokio::time::timeout(TURN_HOLD_HANG, unrelated.wait())
            .await
            .unwrap()
            .unwrap(),
        ExecutionOutcome::Completed
    );
    assert_eq!(
        retained.pending().await.unwrap().len(),
        1,
        "stopped successor keeps its report obligation"
    );
    if active {
        assert_eq!(
            provider
                .cancel_turns
                .lock()
                .unwrap()
                .contains(&next.as_str().to_owned()),
            !command_collision
        );
    } else {
        assert_eq!(
            provider
                .executions
                .lock()
                .unwrap()
                .contains(&next.as_str().to_owned()),
            command_collision
        );
    }
    drop((person, release_parent, release_report, live));
    retire(service, storage, metadata).await;
}
impl commands::CreationStorage for PausedMutationObservation {
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> commands::CreationFuture<'_, Option<Arc<dyn commands::CreationStorageLease>>> {
        self.inner.open_creation(principal, request, create)
    }
    fn mutations(&self, principal: String) -> commands::CreationFuture<'_, Vec<MutationReceipt>> {
        let gate = self.gate.lock().unwrap().take();
        Box::pin(async move {
            if let Some((entered, release)) = gate {
                entered.send(()).unwrap();
                release.await.unwrap();
            }
            self.inner.mutations(principal).await
        })
    }
}
