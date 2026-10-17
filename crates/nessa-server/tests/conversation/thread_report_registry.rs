//! Registry ownership when cleanup overtakes a report already taken for delivery.
use super::*;
use crate::conversation::application::ConversationLimits;
use crate::conversation::infrastructure::LocalThreadReports;
use crate::conversation_test_support::fixture;

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
    use crate::conversation::application::RequestedConversation;
    use nessa_sdk::application::agent_execution::{
        agents::{AttachmentRequest, Resumed},
        permissions::ActionContext,
        providers::{ExecutionReport, ProviderExecutionReply, ProviderSessionState},
    };
    use nessa_sdk::domain::agent_execution::executions::{ExecutionOutcome, MessageChunk};
    use std::sync::atomic::Ordering;
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
    service.shutdown().await.unwrap();
}
