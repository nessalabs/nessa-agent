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
