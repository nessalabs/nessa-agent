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
    let taken = reports.take(
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
    // The sender read the reply while the report was being queued.
    assert!(reports.claim(&child, "work", &parent).is_empty());
    assert!(
        taken[0].delivering.claimed(),
        "the queued report is called off"
    );
    drop(taken);

    // Claimed before it is taken: no report is left to send.
    assert_eq!(
        reports.claim(&child, "other", &parent),
        vec!["report-2".to_owned()]
    );
    assert!(reports.take(&child, &[caller], finished).is_empty());
    service.shutdown().await.unwrap();
}
