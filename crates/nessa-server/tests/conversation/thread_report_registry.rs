//! Registry ownership when cleanup overtakes a report already taken for delivery.
use super::*;
use crate::conversation::application::{ConversationLimits, RequestedConversation};
use crate::conversation::infrastructure::{DurableOwnershipAudit, LocalThreadReports};
use crate::conversation_test_support::{fixture, TestClock};
use nessa_sdk::application::agent_execution::{
    agents::{AgentError, AttachmentRequest, Resumed},
    commands::{
        CreationFuture, CreationStorage, CreationStorageError, CreationStorageLease,
        MutationReceipt,
    },
    permissions::ActionContext,
    providers::{ExecutionReport, ProviderExecutionReply, ProviderSessionState},
    sessions::StorageError,
    subagents::{LiveCapacity, MemoryOwnershipStore, OwnershipCoordinator, OwnershipDependencies},
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionOutcome, MessageChunk};
use std::future::{poll_fn, Future};
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;

#[tokio::test]
async fn thread_report_cleanup_preserves_other_ownership_and_clears_an_absent_row() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("reports");
    nessa_local_storage::create_directory(&private).unwrap();
    let store = Arc::new(LocalThreadReports::open(&private.join("reports.sqlite3")).unwrap());
    let (service, _, _, _) = fixture(ConversationLimits::default());
    service.bind_thread_reports(store.clone());
    let child = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let parent = ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let other = ConversationId::new("00000000-0000-4000-8000-000000000003").unwrap();
    let caller = ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: "report-action".into(),
    };
    let report = ThreadReport {
        parent,
        caller: caller.clone(),
        execution: "report".into(),
    };
    assert_eq!(
        service
            .report_thread_turn(child.clone(), "work".into(), report.clone())
            .await,
        ReportRegistration::Registered
    );
    assert_eq!(
        service
            .report_thread_turn(other.clone(), "work".into(), report)
            .await,
        ReportRegistration::Refused
    );
    assert!(service.inner.thread_reports.fresh().contains("report"));
    service.forget_thread_report(&other, "report").await;
    assert!(service.inner.thread_reports.fresh().contains("report"));
    assert_eq!(store.pending().await.unwrap().len(), 1);

    // Delivery owns the report now; deletion no longer finds it among watches.
    let taken = service
        .inner
        .thread_reports
        .take(&child, &[caller], |_, _| Some("done".into()));
    assert_eq!(taken.len(), 1);
    service.forget_thread_reports_of(&child).await;
    assert!(store.pending().await.unwrap().is_empty());
    assert!(service.inner.thread_reports.fresh().contains("report"));
    // This is the final cleanup of that Taken, after deletion removed its row.
    service.settle_saved(&child, "report").await;
    assert!(service.inner.thread_reports.fresh().is_empty());
    drop(taken);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn an_injected_report_waits_for_its_suspended_hosts_exact_successor() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    provider.native_steering.store(true, Ordering::SeqCst);
    provider.inject_steers.store(true, Ordering::SeqCst);
    let child = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    let caller = ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: "host".into(),
    };
    service
        .create(
            child.clone(),
            caller.clone(),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    let (release, gate) = tokio::sync::oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    *provider.execution_reply.lock().unwrap() =
        Some(ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Ok(ExecutionOutcome::Cancelled)),
            None,
            ProviderSessionState::Usable,
        )));
    let input = |text: &str| SubmittedMessage {
        text: text.into(),
        images: vec![],
        files: vec![],
    };
    service
        .submit(
            child.clone(),
            caller.clone(),
            "host".into(),
            input("host work"),
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
    service
        .submit(
            child.clone(),
            caller.clone(),
            "steer".into(),
            input("also this"),
            SubmissionMode::Steer,
        )
        .await
        .unwrap();
    let live = service.resolve(&child, &caller).await.unwrap();
    let before = live.agent.session_manager().snapshot().await.unwrap();
    let injected = invocation(&before.invocations, "steer").unwrap();
    assert_eq!(
        injected.scheduling.last().unwrap().stage,
        InvocationStage::Injected
    );
    let closing = {
        let agent = live.agent.clone();
        tokio::spawn(async move {
            agent
                .suspend(ActionContext::new("person", "gateway", "shutdown").unwrap())
                .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while provider.close_calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled));
    drop(release);
    tokio::time::timeout(Duration::from_secs(5), closing)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let saved = live.agent.session_manager().snapshot().await.unwrap();
    assert!(matches!(
        in_record(
            &saved.invocations,
            invocation(&saved.invocations, "steer").unwrap(),
            false
        ),
        Found::Continuing
    ));
    assert_eq!(
        service.thread_turn_end(&child, &caller, "steer").await,
        ThreadTurn::Unfinished,
        "subscribe and the report await the same exact successor"
    );
    let owed = live
        .agent
        .owed_turns()
        .await
        .into_iter()
        .find(|owed| owed.execution_id().as_str() == "host")
        .unwrap();
    let actor = caller.actor().unwrap();
    let authorization = live
        .agent
        .authorize_attachment(AttachmentRequest::CallerRequested(actor.clone()))
        .unwrap();
    live.agent
        .start_attachment(authorization)
        .unwrap()
        .wait()
        .await
        .unwrap();
    provider.execution_updates.lock().unwrap().extend([
        ExecutionUpdate::Message(MessageChunk::text("Successor answer.")),
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    ]);
    *provider.execution_reply.lock().unwrap() =
        Some(ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Ok(ExecutionOutcome::Completed)),
            None,
            ProviderSessionState::Usable,
        )));
    let Resumed::Continued(admission) = live.agent.resume_turn(&owed, actor).await.unwrap() else {
        panic!("host continues")
    };
    assert_eq!(admission.wait().await.unwrap(), ExecutionOutcome::Completed);
    let saved = live.agent.session_manager().snapshot().await.unwrap();
    match in_record(
        &saved.invocations,
        invocation(&saved.invocations, "steer").unwrap(),
        false,
    ) {
        Found::Finished("completed", Some(reply)) => assert_eq!(reply, "Successor answer."),
        _ => panic!("the report must follow its injected host's exact successful successor"),
    }
    assert_eq!(
        service.thread_turn_end(&child, &caller, "steer").await,
        ThreadTurn::Ended("completed", Some("Successor answer.".into())),
        "subscribe and the report return the same successor's reply"
    );
    service.shutdown().await.unwrap();
}

struct UnavailableReportCommands {
    observing: AtomicBool,
    observed: AtomicBool,
}
impl CreationStorage for UnavailableReportCommands {
    fn open_creation(
        &self,
        _: String,
        _: String,
        _: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async { Err(CreationStorageError::Storage(StorageError::Busy)) })
    }
    fn mutations(&self, _: String) -> CreationFuture<'_, Vec<MutationReceipt>> {
        Box::pin(async {
            if self.observing.load(Ordering::SeqCst) {
                self.observed.store(true, Ordering::SeqCst);
            }
            Err(CreationStorageError::Storage(StorageError::Busy))
        })
    }
}

struct ObservedReportSettlement {
    inner: Arc<LocalThreadReports>,
    calls: AtomicUsize,
}
impl ThreadReportStore for ObservedReportSettlement {
    fn save(&self, row: &PendingReport) -> ConversationFuture<'_, bool> {
        self.inner.save(row)
    }
    fn settle(&self, child: &ConversationId, report: &str) -> ConversationFuture<'_, ()> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.inner.settle(child, report)
    }
    fn forget_conversation(&self, id: &ConversationId) -> ConversationFuture<'_, ()> {
        self.inner.forget_conversation(id)
    }
    fn pending(&self) -> ConversationFuture<'_, Vec<PendingReport>> {
        self.inner.pending()
    }
}

#[tokio::test]
async fn unavailable_stop_receipts_cannot_acknowledge_a_declined_failed_report() {
    let directory = tempfile::tempdir().unwrap();
    let private = directory.path().join("reports");
    nessa_local_storage::create_directory(&private).unwrap();
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(MemoryOwnershipStore::new()),
        audit: Arc::new(
            DurableOwnershipAudit::new(directory.path().join("audit"), Arc::new(TestClock))
                .unwrap(),
        ),
        factory: service.owned_thread_factory(),
        room: Arc::new(LiveCapacity::new(1)),
    });
    coordinator.resume().await.unwrap();
    service.bind_owned_threads(coordinator, true).unwrap();
    let store = Arc::new(ObservedReportSettlement {
        inner: Arc::new(LocalThreadReports::open(&private.join("reports.sqlite3")).unwrap()),
        calls: AtomicUsize::new(0),
    });
    service.bind_thread_reports(store.clone());
    let child = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    let parent = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    let caller = ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: "report-action".into(),
    };
    for id in [&child, &parent] {
        service
            .create(id.clone(), caller.clone(), RequestedConversation::default())
            .await
            .unwrap();
    }
    let input = |text: String| SubmittedMessage {
        text,
        ..Default::default()
    };
    let (release_child, gate) = tokio::sync::oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(gate);
    let started = provider.execution_started.notified();
    service
        .submit(
            child.clone(),
            caller.clone(),
            "child-work".into(),
            input("child work".into()),
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(Duration::from_secs(60), started)
        .await
        .unwrap();
    let report = ThreadReport {
        parent: parent.clone(),
        caller: caller.clone(),
        execution: "failed-report".into(),
    };
    service
        .report_thread_turn(child.clone(), "child-work".into(), report)
        .await;
    let text = report_text(&child, "child-work", "completed", None, 1000);
    // The real registry transfers delivery ownership before the child's hook
    // runs, so this test owns/polls that future instead of a background task.
    let taken = service
        .inner
        .thread_reports
        .take(&child, std::slice::from_ref(&caller), |_, _| {
            Some(text.clone())
        })
        .pop()
        .unwrap();
    release_child.send(()).unwrap();
    let child_live = service.resolve(&child, &caller).await.unwrap();
    tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            let saved = child_live.agent.session_manager().snapshot().await.unwrap();
            if invocation(&saved.invocations, "child-work").is_some_and(settled) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    *provider.prepare_failure.lock().unwrap() =
        Some(AgentError::Transport("report preparation failed".into()));
    let admission = Arc::new(service.inner.thread_admission.clone().lock_owned().await);
    service
        .submit_as(
            parent.clone(),
            caller.clone(),
            "failed-report".into(),
            input(text),
            SubmissionMode::Queue,
            Writer::ThreadReport {
                _admission: admission,
            },
        )
        .await
        .unwrap();
    let live = service.resolve(&parent, &caller).await.unwrap();
    let owed = tokio::time::timeout(Duration::from_secs(60), async {
        loop {
            if let Some(owed) = live
                .agent
                .owed_turns()
                .await
                .into_iter()
                .find(|turn| turn.execution_id().as_str() == "failed-report")
            {
                break owed;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    live.agent
        .decline_turn(&owed, caller.actor().unwrap())
        .await
        .unwrap();
    let commands = Arc::new(UnavailableReportCommands {
        observing: AtomicBool::new(false),
        observed: AtomicBool::new(false),
    });
    service.bind_commands(commands.clone());
    let mut delivery = Box::pin(service.deliver_report(taken));
    // Current-thread execution records only this future's reads. Once its
    // unavailable fact is ready, one poll reaches either retry sleep or the
    // settlement call. The adapter observes that call before any SQLite await.
    tokio::time::timeout(
        Duration::from_secs(60),
        poll_fn(|cx| {
            commands.observing.store(true, Ordering::SeqCst);
            let outcome = delivery.as_mut().poll(cx);
            commands.observing.store(false, Ordering::SeqCst);
            assert!(
                outcome.is_pending(),
                "unavailable receipt evidence cannot finish delivery"
            );
            if commands.observed.load(Ordering::SeqCst) {
                assert_eq!(
                    store.calls.load(Ordering::SeqCst),
                    0,
                    "unknown Stop receipts cannot pay a failed report obligation"
                );
                Poll::Ready(())
            } else {
                Poll::Pending
            }
        }),
    )
    .await
    .unwrap();
    drop(delivery);
    assert_eq!(store.pending().await.unwrap().len(), 1);
    service.shutdown().await.unwrap();
}

/// A sender call waiting on a turn holds its report back; once let go, the
/// report is taken as usual. A claim while it is being queued calls it off,
/// and a claim before it is taken leaves nothing to take.
#[tokio::test]
async fn an_awaited_report_is_held_and_a_claim_calls_off_one_being_queued() {
    let (service, _, _, _) = fixture(ConversationLimits::default());
    let child = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let parent = ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let caller = ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: "report-action".into(),
    };
    let reports = &service.inner.thread_reports;
    let finished = |_: &str, _: bool| Some("done".to_owned());
    for (work, report) in [("work", "report"), ("other", "report-2")] {
        let report = ThreadReport {
            parent: parent.clone(),
            caller: caller.clone(),
            execution: report.into(),
        };
        assert_eq!(
            service
                .report_thread_turn(child.clone(), work.into(), report)
                .await,
            ReportRegistration::Registered
        );
    }

    // Held while a call waits on it, taken once that call lets go.
    reports.await_turn(&child, "work", &parent, 1);
    let taken = reports.take(
        &child,
        std::slice::from_ref(&caller),
        |execution, restored| {
            (execution == "work")
                .then(|| finished(execution, restored))
                .flatten()
        },
    );
    assert!(taken.is_empty(), "an awaited report is held");
    reports.await_turn(&child, "work", &parent, -1);
    let mut taken = reports.take(
        &child,
        std::slice::from_ref(&caller),
        |execution, restored| {
            (execution == "work")
                .then(|| finished(execution, restored))
                .flatten()
        },
    );
    assert_eq!(taken.len(), 1);
    assert!(!taken[0].delivering.claimed());
    assert_eq!(reports.delivering.load(Ordering::SeqCst), 1);
    // Submission/Stop releases live admission, but the same guard still
    // owns the claim. Releasing twice cannot consume another report's count.
    taken[0].delivering.release();
    taken[0].delivering.release();
    assert_eq!(reports.delivering.load(Ordering::SeqCst), 0);
    assert_eq!(reports.queuing.lock().unwrap().len(), 1);
    assert!(!taken[0].delivering.claimed());
    // The sender read the reply while the report was being queued.
    assert!(reports.claim(&child, "work", &parent).is_empty());
    assert!(
        taken[0].delivering.claimed(),
        "the queued report is called off"
    );
    drop(taken);
    assert!(reports.queuing.lock().unwrap().is_empty());
    assert_eq!(reports.delivering.load(Ordering::SeqCst), 0);

    // Claimed before it is taken: no report is left to send.
    assert_eq!(
        reports.claim(&child, "other", &parent),
        vec!["report-2".to_owned()]
    );
    assert!(reports.take(&child, &[caller], finished).is_empty());
    service.shutdown().await.unwrap();
}
