//! Actual gateway composition: persistent SDK graph + ordinary command receipts.
use super::*;
use crate::conversation::application::{
    AgentToolSource, Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease,
};
use nessa_sdk::application::agent_execution::providers::AgentProvider;
use nessa_sdk::application::agent_execution::subagents::{
    LiveCapacity, OwnershipCoordinator, OwnershipDependencies,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseCleanup, LeaseId, LeaseRefusal, LeaseTerms};
use nessa_sdk::infrastructure::session_storage::SqliteOwnershipStore;

pub(super) async fn bind(
    service: &ConversationService,
    storage: &Arc<RecordStorage>,
    root: &Path,
    cascade: bool,
) -> OwnershipCoordinator {
    service.bind_commands(storage.clone());
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap()),
        audit: Arc::new(
            crate::conversation::infrastructure::DurableOwnershipAudit::new(
                root.join("ownership-audit"),
            )
            .unwrap(),
        ),
        factory: service.owned_thread_factory(),
        room: Arc::new(LiveCapacity::new(32)),
    });
    coordinator.resume().await.unwrap();
    service
        .bind_owned_threads(coordinator.clone(), cascade)
        .unwrap();
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
            Some(nessa_sdk::domain::agent_execution::subagents::LifetimeState::Open)
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
    use crate::conversation::application::{ThreadReport, ThreadReportStore};
    use crate::conversation::infrastructure::LocalThreadReports;
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
    service
        .stop_command(
            storage.clone(),
            parent.clone(),
            caller("stop-parent"),
            "parent".into(),
        )
        .await
        .unwrap();
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

use nessa_sdk::application::agent_execution::commands;
struct FaultyCommands {
    storage: Arc<RecordStorage>,
    target: ConversationId,
    commit: bool,
    stage: MutationStage,
    once: Arc<std::sync::atomic::AtomicBool>,
}
struct FaultyLease {
    inner: Arc<dyn commands::CreationStorageLease>,
    target: ConversationId,
    commit: bool,
    stage: MutationStage,
    once: Arc<std::sync::atomic::AtomicBool>,
}
impl commands::CreationStorage for FaultyCommands {
    fn open_creation(
        &self,
        principal: String,
        create: bool,
    ) -> commands::CreationFuture<'_, Option<Arc<dyn commands::CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .storage
                .open_creation(principal, create)
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
    fn mutations(&self) -> commands::CreationFuture<'_, Vec<MutationReceipt>> {
        self.inner.mutations()
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
                nessa_sdk::application::agent_execution::sessions::StorageError::Corrupt(
                    "simulated settlement acknowledgement loss".into(),
                ),
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
            once: Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
    use nessa_sdk::application::agent_execution::subagents::OwnershipStore;
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
        once: Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
    use crate::conversation::application::{ThreadReport, ThreadReportStore};
    use crate::conversation::infrastructure::LocalThreadReports;
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
    service
        .submit_command(
            storage.clone(),
            parent.clone(),
            caller("later-person"),
            Initiation::owner(),
            false,
            "later-person".into(),
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
    let before_erasure_close =
        nessa_sdk::application::agent_execution::subagents::OwnershipStore::read(&owned_store)
            .await
            .unwrap();
    assert!(service
        .delete(removed, caller("delete-child"))
        .await
        .unwrap());
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(nessa_sdk::domain::agent_execution::subagents::LifetimeState::Closed)
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
    nessa_sdk::application::agent_execution::subagents::OwnershipStore::write(
        &owned_store,
        &before_erasure_close,
    )
    .await
    .unwrap();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let coordinator = bind(&service, &storage, &root, false).await;
    assert_eq!(
        service
            .recover_deleted_ownership(&before_erasure_close)
            .await
            .unwrap(),
        1
    );
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(nessa_sdk::domain::agent_execution::subagents::LifetimeState::Closed)
    );
    assert!(provider.executions.lock().unwrap().is_empty());
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn deleting_a_cold_owned_tree_reconciles_confirmed_leases_and_cannot_reparent_children() {
    use nessa_sdk::domain::agent_execution::subagents::LifetimeState;
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
    let snapshot = nessa_sdk::application::agent_execution::subagents::OwnershipStore::read(&owned)
        .await
        .unwrap();
    service.recover_deleted_ownership(&snapshot).await.unwrap();
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
    pause: std::sync::atomic::AtomicBool,
    entered: tokio::sync::Notify,
    release: tokio::sync::Notify,
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
    use nessa_sdk::application::agent_execution::subagents::CloseCommand;
    use nessa_sdk::domain::agent_execution::subagents::{Initiator, LifetimeCause, LifetimeState};
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let environment = Arc::new(PausedOwnedOpening {
        inner: crate::conversation::infrastructure::in_process_environment(),
        pause: std::sync::atomic::AtomicBool::new(false),
        entered: tokio::sync::Notify::new(),
        release: tokio::sync::Notify::new(),
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
    let snapshot = nessa_sdk::application::agent_execution::subagents::OwnershipStore::read(&owned)
        .await
        .unwrap();
    service.recover_deleted_ownership(&snapshot).await.unwrap();
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
    use nessa_sdk::application::agent_execution::providers::{
        ExecutionReport, ProviderExecutionReply, ProviderSessionState,
    };
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
    *provider.execution_observation_failure.lock().unwrap() = Some(
        nessa_sdk::application::agent_execution::providers::ObservationFailure::new(
            AgentError::Transport("pipe".into()),
            nessa_sdk::application::agent_execution::providers::ObservationFailureCause::ExecutionFailed));
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
        once: Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
    use nessa_sdk::application::agent_execution::subagents::OwnershipStore;
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
            once: Arc::new(std::sync::atomic::AtomicBool::new(true)),
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
    use nessa_sdk::application::agent_execution::subagents::CloseCommand;
    use nessa_sdk::domain::agent_execution::subagents::{
        HostActor, Initiator, LifetimeCause, LifetimeState,
    };
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
