use super::*;
use crate::desktop_runtime::{
    application::{restore_retirement, retire, RetirementResult},
    domain::RunningRuntime,
};
use crate::{
    conversation::application::{ConversationCaller, ConversationLimits},
    conversation::domain::ConversationId,
    conversation_test_support::fixture,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::agents::AgentError;
use std::io::Write;

#[test]
fn private_requests_require_exact_validated_shape() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let path = files.directory.join("request.json");
    for value in [
        json!({"requestId":"invalid","runningGeneration":"c".repeat(64),"targetGeneration":"d".repeat(64),"runningInstance":INSTANCE,"targetFingerprint":"a".repeat(64)}),
        json!({"requestId":Uuid::new_v4().to_string(),"runningGeneration":"c".repeat(64),"targetGeneration":"d".repeat(64),"runningInstance":INSTANCE,"targetFingerprint":"a".repeat(64),"extra":true}),
        json!({"requestId":Uuid::new_v4().to_string(),"runningGeneration":"c".repeat(64),"targetGeneration":"d".repeat(64),"runningInstance":INSTANCE,"targetFingerprint":"A".repeat(64)}),
    ] {
        write(&files.directory, "request.json", &value).unwrap();
        assert!(files.request().is_err());
    }
    let id = Uuid::new_v4().to_string();
    write(
        &files.directory,
        "request.json",
        &json!({"requestId":id,"runningGeneration":"c".repeat(64),"targetGeneration":"d".repeat(64),"runningInstance":INSTANCE,"targetFingerprint":"b".repeat(64)}),
    )
    .unwrap();
    assert_eq!(files.request().unwrap().id(), id);
    let mut file = open(&path, OpenMode::ReadWrite).unwrap();
    file.write_all(&vec![b'x'; 4097]).unwrap();
    assert!(files.request().is_err());
}

#[tokio::test]
async fn retirement_acknowledges_correlated_durable_evidence_even_without_agents() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let request = RetirementRequest::new(
        Uuid::new_v4().to_string(),
        "b".repeat(64),
        INSTANCE.into(),
        "c".repeat(64),
        "d".repeat(64),
    )
    .unwrap();
    let result = retire(
        request.clone(),
        RunningRuntime::new("a".repeat(64), INSTANCE.into(), 123, "c".repeat(64)).unwrap(),
        None,
        &PresentData,
        &files,
    )
    .await;
    assert!(result.retired);
    files.result(&result).unwrap();
    let value: Value = serde_json::from_reader(
        open(&files.directory.join("result.json"), OpenMode::Read).unwrap(),
    )
    .unwrap();
    assert_eq!(value["requestId"], request.id());
    assert_eq!(value["runningFingerprint"], "a".repeat(64));
    assert_eq!(value["targetFingerprint"], "b".repeat(64));
    let audit_path = std::fs::read_dir(files.directory.join("audit"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let audit: Value = serde_json::from_reader(std::fs::File::open(audit_path).unwrap()).unwrap();
    assert_eq!(audit["initiator"], "gateway");
    assert_eq!(audit["cause"], "gateway_upgrade");
    assert_eq!(audit["retirementRequestId"], request.id());
}

struct RejectingAudit;
impl RetirementAudit for RejectingAudit {
    fn record(
        &self,
        _: RetirementRecord,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(async { Err("audit refused".into()) })
    }
}

#[tokio::test]
async fn audit_rejects_invalid_lifecycle_correlation_instead_of_erasing_attribution() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let record = RetirementRecord {
        request: request(INSTANCE),
        running: running(INSTANCE, "a", "c"),
        cleanup_error: Some("different cause".into()),
        retirement_cause: Some(
            nessa_sdk::application::agent_execution::permissions::ActionContext::new(
                "gateway",
                "server_shutdown",
                "not-a-uuid",
            )
            .unwrap(),
        ),
    };
    assert!(files.record(record).await.is_err());
    assert_eq!(
        std::fs::read_dir(files.directory.join("audit"))
            .unwrap()
            .count(),
        0
    );
}
#[tokio::test]
async fn audit_failure_never_acknowledges_retirement() {
    let result = retire(
        RetirementRequest::new(
            Uuid::new_v4().to_string(),
            "b".repeat(64),
            INSTANCE.into(),
            "c".repeat(64),
            "d".repeat(64),
        )
        .unwrap(),
        RunningRuntime::new("a".repeat(64), INSTANCE.into(), 123, "c".repeat(64)).unwrap(),
        None,
        &PresentData,
        &RejectingAudit,
    )
    .await;
    assert!(!result.retired);
    assert!(result.cleanup_error.is_none());
    assert_eq!(result.audit_error.as_deref(), Some("audit refused"));
}

/// ADR 221: the file carries a refusal only when there is one, the reader
/// holds the writer's rule, and an audit failure alone is never `data_missing`.
#[tokio::test]
async fn a_refused_retirement_names_why_and_the_file_carries_it() {
    struct MissingData;
    impl crate::desktop_runtime::application::ConversationData for MissingData {
        fn missing(&self) -> bool {
            true
        }
    }
    let request = || {
        RetirementRequest::new(
            Uuid::new_v4().to_string(),
            "b".repeat(64),
            INSTANCE.into(),
            "c".repeat(64),
            "d".repeat(64),
        )
        .unwrap()
    };
    let running =
        || RunningRuntime::new("a".repeat(64), INSTANCE.into(), 123, "c".repeat(64)).unwrap();

    // The retirement audit failing says nothing about what was stopped.
    let refused = retire(request(), running(), None, &MissingData, &RejectingAudit).await;
    assert!(!refused.retired);
    assert_eq!(refused.refusal, Some(RetirementRefusal::NotConfirmed));

    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let read = |files: &RetirementFiles| -> serde_json::Value {
        serde_json::from_slice(&std::fs::read(files.directory.join("result.json")).unwrap())
            .unwrap()
    };

    let retired = retire(request(), running(), None, &MissingData, &files).await;
    assert!(retired.retired);
    assert_eq!(retired.refusal, None);
    files.result(&retired).unwrap();
    // A retired result is exactly what earlier gateways and hosts wrote.
    assert!(read(&files).get("refusal").is_none());

    let mut data_missing = refused.clone();
    data_missing.refusal = Some(RetirementRefusal::DataMissing);
    let other_root = tempfile::tempdir().unwrap();
    let other = RetirementFiles::new(other_root.path(), Arc::new(TestClock)).unwrap();
    other.result(&data_missing).unwrap();
    assert_eq!(read(&other)["refusal"], "data_missing");
    assert!(other.evidence().is_ok());

    let mut contradictory = refused.clone();
    contradictory.refusal = None;
    assert!(other.result(&contradictory).is_err());
    let mut contradictory = retired.clone();
    contradictory.refusal = Some(RetirementRefusal::NotConfirmed);
    assert!(files.result(&contradictory).is_err());

    // What the writer refuses to write, the reader refuses to read.
    for refusal in [json!("not_confirmed"), json!("stopped_for_fun")] {
        let mut stored = read(&files);
        stored["refusal"] = refusal;
        std::fs::remove_file(files.directory.join("result.json")).unwrap();
        write(&files.directory, "result.json", &stored).unwrap();
        assert!(files.evidence().is_err());
    }
}

struct TestClock;
impl Clock for TestClock {
    fn unix_seconds(&self) -> u64 {
        123
    }
    fn unix_milliseconds(&self) -> u64 {
        123_456
    }
}

const INSTANCE: &str = "b4a38c5b-cf70-4d90-9059-7d9a3a51c658";

fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: "create".into(),
    }
}
fn conversation_id() -> ConversationId {
    ConversationId::new(&Uuid::new_v4().to_string()).unwrap()
}
fn running(instance: &str, fingerprint: &str, generation: &str) -> RunningRuntime {
    RunningRuntime::new(
        fingerprint.repeat(64),
        instance.into(),
        123,
        generation.repeat(64),
    )
    .unwrap()
}
fn request(instance: &str) -> RetirementRequest {
    RetirementRequest::new(
        Uuid::new_v4().to_string(),
        "b".repeat(64),
        instance.into(),
        "c".repeat(64),
        "d".repeat(64),
    )
    .unwrap()
}
#[tokio::test]
async fn wrong_instance_or_generation_cannot_close_admission() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    service
        .create(conversation_id(), caller(), None)
        .await
        .unwrap();
    let other = Uuid::new_v4().to_string();
    for identity in [running(&other, "a", "c"), running(INSTANCE, "a", "e")] {
        let result = retire(
            request(INSTANCE),
            identity,
            Some(&service),
            &PresentData,
            &files,
        )
        .await;
        assert!(!result.retired);
        assert!(result.retirement_request_id().is_none());
        assert!(service.retirement_cause().is_none());
        assert_eq!(
            provider
                .close_calls
                .load(std::sync::atomic::Ordering::SeqCst),
            0
        );
        service
            .create(conversation_id(), caller(), None)
            .await
            .unwrap();
    }
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn durable_fence_restores_only_the_admitted_generation_and_preserves_original_correlation() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let original_request = request(INSTANCE);
    let result = retire(
        original_request.clone(),
        running(INSTANCE, "a", "c"),
        None,
        &PresentData,
        &files,
    )
    .await;
    files.result(&result).unwrap();
    let fence = files.fence().unwrap().unwrap();
    let restarted_instance = Uuid::new_v4().to_string();
    let restarted = running(&restarted_instance, "a", "c");
    let (service, _, _, _) = fixture(ConversationLimits::default());
    restore_retirement(Some(&fence), &restarted, Some(&service))
        .await
        .unwrap();
    assert!(service
        .create(conversation_id(), caller(), None)
        .await
        .is_err());
    assert_eq!(
        service.retirement_cause().unwrap().request_id(),
        original_request.id()
    );
    let retry = retire(
        request(&restarted_instance),
        restarted.clone(),
        Some(&service),
        &PresentData,
        &files,
    )
    .await;
    assert!(retry.retired);
    assert_eq!(retry.retirement_request_id(), Some(original_request.id()));
    files.result(&retry).unwrap();
    assert_eq!(
        files.fence().unwrap().unwrap().retirement_request_id(),
        original_request.id()
    );

    // Both a runtime update and a configuration-only update have their own generation.
    for replacement in [
        running(&restarted_instance, "b", "d"),
        running(&restarted_instance, "a", "d"),
    ] {
        let (replacement_service, _, _, _) = fixture(ConversationLimits::default());
        restore_retirement(Some(&fence), &replacement, Some(&replacement_service))
            .await
            .unwrap();
        replacement_service
            .create(conversation_id(), caller(), None)
            .await
            .unwrap();
        replacement_service.shutdown().await.unwrap();
    }
    let wrong = retire(
        request(INSTANCE),
        restarted,
        Some(&service),
        &PresentData,
        &files,
    )
    .await;
    assert!(!wrong.retired);
    assert!(files.result(&wrong).is_err());
    assert_eq!(
        files.fence().unwrap().unwrap().retirement_request_id(),
        original_request.id()
    );
}

#[test]
fn restored_evidence_rejects_contradictory_or_missing_fields() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let valid = json!({"requestId":INSTANCE,"retirementRequestId":INSTANCE,"retirementCause":{"principalId":"gateway","surfaceId":"gateway_upgrade","requestId":INSTANCE},"requestedInstance":INSTANCE,"runningInstance":INSTANCE,"runningFingerprint":"a".repeat(64),"targetFingerprint":"b".repeat(64),"runningGeneration":"c".repeat(64),"requestedRunningGeneration":"c".repeat(64),"targetGeneration":"d".repeat(64),"retired":true,"cleanupError":null,"auditError":null});
    for key in [
        "auditError",
        "cleanupError",
        "retirementRequestId",
        "retirementCause",
        "requestedInstance",
        "runningInstance",
        "runningGeneration",
        "requestedRunningGeneration",
        "targetGeneration",
    ] {
        let mut missing = valid.clone();
        missing.as_object_mut().unwrap().remove(key);
        write(&files.directory, "result.json", &missing).unwrap();
        assert!(files.fence().is_err(), "{key}");
    }
    let mut cause_free_admitted = valid.clone();
    cause_free_admitted["retirementRequestId"] = Value::Null;
    cause_free_admitted["retirementCause"] = Value::Null;
    cause_free_admitted["retired"] = json!(false);
    cause_free_admitted["cleanupError"] = json!("cleanup failed");
    write(&files.directory, "result.json", &cause_free_admitted).unwrap();
    assert!(files.fence().is_err());

    for (field, value) in [
        ("principalId", json!("system-supervisor")),
        ("surfaceId", json!("server_shutdown")),
    ] {
        let mut wrong_authority = valid.clone();
        wrong_authority["retirementCause"][field] = value;
        write(&files.directory, "result.json", &wrong_authority).unwrap();
        assert!(files.fence().is_err());

        wrong_authority["retired"] = json!(false);
        wrong_authority["cleanupError"] = json!("closed by another lifecycle cause");
        write(&files.directory, "result.json", &wrong_authority).unwrap();
        assert!(files.fence().unwrap().is_some());
    }

    let mut contradicted = valid;
    contradicted["auditError"] = json!("unacknowledged");
    write(&files.directory, "result.json", &contradicted).unwrap();
    assert!(files.fence().is_err());

    let failure_without_error = json!({"requestId":INSTANCE,"retirementRequestId":INSTANCE,"retirementCause":{"principalId":"gateway","surfaceId":"gateway_upgrade","requestId":INSTANCE},"requestedInstance":INSTANCE,"runningInstance":INSTANCE,"runningFingerprint":"a".repeat(64),"targetFingerprint":"b".repeat(64),"runningGeneration":"c".repeat(64),"requestedRunningGeneration":"c".repeat(64),"targetGeneration":"d".repeat(64),"retired":false,"cleanupError":null,"auditError":null});
    write(&files.directory, "result.json", &failure_without_error).unwrap();
    assert!(files.fence().is_err());
}

#[test]
fn result_writer_rejects_admitted_failure_without_a_failure_reason() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let request = request(INSTANCE);
    let result = RetirementResult {
        retirement_cause: Some(
            nessa_sdk::application::agent_execution::permissions::ActionContext::new(
                "gateway",
                "gateway_upgrade",
                request.id(),
            )
            .unwrap(),
        ),
        request,
        running: running(INSTANCE, "a", "c"),
        retired: false,
        cleanup_error: None,
        audit_error: None,
        refusal: Some(RetirementRefusal::NotConfirmed),
    };
    assert!(files.result(&result).is_err());
}

#[test]
fn result_writer_validates_the_complete_retirement_tuple() {
    use nessa_sdk::application::agent_execution::permissions::ActionContext;

    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let make = |request: RetirementRequest,
                running: RunningRuntime,
                retirement_cause: Option<ActionContext>,
                retired: bool,
                cleanup_error: Option<String>,
                audit_error: Option<String>| RetirementResult {
        request,
        running,
        retired,
        retirement_cause,
        cleanup_error,
        audit_error,
        refusal: (!retired).then_some(RetirementRefusal::NotConfirmed),
    };

    let admitted = request(INSTANCE);
    let cause = ActionContext::new("gateway", "gateway_upgrade", admitted.id()).unwrap();
    assert!(files
        .result(&make(
            admitted.clone(),
            running(INSTANCE, "a", "c"),
            None,
            true,
            None,
            None,
        ))
        .is_err());
    let wrong_authority =
        ActionContext::new("system-supervisor", "gateway_upgrade", admitted.id()).unwrap();
    assert!(files
        .result(&make(
            admitted.clone(),
            running(INSTANCE, "a", "c"),
            Some(wrong_authority.clone()),
            true,
            None,
            None,
        ))
        .is_err());
    assert!(files
        .result(&make(
            admitted.clone(),
            running(INSTANCE, "a", "c"),
            Some(wrong_authority),
            false,
            Some("closed by another lifecycle cause".into()),
            None,
        ))
        .is_ok());
    assert!(files
        .result(&make(
            admitted.clone(),
            running(INSTANCE, "a", "c"),
            Some(ActionContext::new("gateway", "gateway_upgrade", "not-a-uuid").unwrap()),
            true,
            None,
            None,
        ))
        .is_err());
    assert!(files
        .result(&make(
            admitted.clone(),
            running(INSTANCE, "a", "c"),
            Some(cause.clone()),
            true,
            Some("cleanup uncertain".into()),
            None,
        ))
        .is_err());
    assert!(files
        .result(&make(
            admitted.clone(),
            running(&Uuid::new_v4().to_string(), "a", "c"),
            Some(cause.clone()),
            false,
            Some("wrong instance".into()),
            None,
        ))
        .is_err());
    assert!(files
        .result(&make(
            admitted,
            running(INSTANCE, "a", "e"),
            Some(cause),
            false,
            Some("wrong generation".into()),
            None,
        ))
        .is_err());
}

#[cfg(unix)]
#[test]
fn retirement_reads_reject_fifos_without_waiting_for_a_writer() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    for name in ["request.json", "result.json"] {
        let path = files.directory.join(name);
        assert!(std::process::Command::new("mkfifo")
            .arg(&path)
            .status()
            .unwrap()
            .success());
        let result = if name == "request.json" {
            files.request().map(|_| ())
        } else {
            files.evidence().map(|_| ())
        };
        assert!(result.is_err(), "{name} must reject a FIFO");
        std::fs::remove_file(path).unwrap();
    }
}

struct StalledAudit;
impl RetirementAudit for StalledAudit {
    fn record(
        &self,
        _: RetirementRecord,
    ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
        Box::pin(std::future::pending())
    }
}
#[tokio::test(start_paused = true)]
async fn stalled_audit_refuses_acknowledgement_and_a_later_request_can_progress() {
    let request = request(INSTANCE);
    let result = retire(
        request.clone(),
        running(INSTANCE, "a", "c"),
        None,
        &PresentData,
        &StalledAudit,
    )
    .await;
    assert!(!result.retired);
    assert!(result.cleanup_error.is_none());
    assert_eq!(
        result.audit_error.as_deref(),
        Some("retirement audit acknowledgement deadline elapsed")
    );
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let retry = retire(
        request,
        running(INSTANCE, "a", "c"),
        None,
        &PresentData,
        &files,
    )
    .await;
    assert!(retry.retired);
}

#[tokio::test]
async fn another_lifecycle_cause_cannot_be_relabelled_as_successful_upgrade() {
    let (service, _, _, _) = fixture(ConversationLimits::default());
    let original = Uuid::new_v4().to_string();
    service
        .retire_with_context(
            nessa_sdk::application::agent_execution::permissions::ActionContext::new(
                "system-supervisor",
                "server_shutdown",
                &original,
            )
            .unwrap(),
        )
        .await
        .unwrap();
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let runtime = running(INSTANCE, "a", "c");
    let result = retire(
        request(INSTANCE),
        runtime.clone(),
        Some(&service),
        &PresentData,
        &files,
    )
    .await;
    assert!(!result.retired);
    assert_eq!(
        service.retirement_cause().unwrap().surface_id(),
        "server_shutdown"
    );
    assert_eq!(result.retirement_request_id(), Some(original.as_str()));
    let audit_path = std::fs::read_dir(files.directory.join("audit"))
        .unwrap()
        .next()
        .unwrap()
        .unwrap()
        .path();
    let audit: Value = serde_json::from_reader(std::fs::File::open(audit_path).unwrap()).unwrap();
    assert_eq!(audit["initiator"], "system-supervisor");
    assert_eq!(audit["cause"], "server_shutdown");
    assert_eq!(audit["retirementRequestId"], original);
    assert_eq!(audit["before"], "retirement_requested");
    assert_eq!(audit["after"], "retirement_unconfirmed");
    files.result(&result).unwrap();
    let reloaded = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let fence = reloaded.fence().unwrap().unwrap();
    assert_eq!(fence.cause().principal_id(), "system-supervisor");
    assert_eq!(fence.cause().surface_id(), "server_shutdown");
    assert_eq!(fence.cause().request_id(), original);

    let restarted = running(&Uuid::new_v4().to_string(), "a", "c");
    let (restarted_service, _, _, _) = fixture(ConversationLimits::default());
    restore_retirement(Some(&fence), &restarted, Some(&restarted_service))
        .await
        .unwrap();
    assert_eq!(
        restarted_service.retirement_cause().unwrap(),
        nessa_sdk::application::agent_execution::permissions::ActionContext::new(
            "system-supervisor",
            "server_shutdown",
            original,
        )
        .unwrap()
    );
}

#[tokio::test]
async fn wrong_generation_rejection_can_be_read_after_restart_and_does_not_poison_retry() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let (service, _, _, _) = fixture(ConversationLimits::default());
    // Requested c -> d, but the receiving process already runs generation d.
    let rejected = retire(
        request(INSTANCE),
        running(INSTANCE, "a", "d"),
        Some(&service),
        &PresentData,
        &files,
    )
    .await;
    assert!(!rejected.retired);
    assert!(rejected.retirement_request_id().is_none());
    files.result(&rejected).unwrap();
    let restarted_reader = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    assert!(restarted_reader.fence().unwrap().is_none());
    assert!(restarted_reader.evidence().unwrap().is_none());
    service
        .create(conversation_id(), caller(), None)
        .await
        .unwrap();
    let admitted = RetirementRequest::new(
        Uuid::new_v4().to_string(),
        "b".repeat(64),
        INSTANCE.into(),
        "d".repeat(64),
        "e".repeat(64),
    )
    .unwrap();
    let result = retire(
        admitted.clone(),
        running(INSTANCE, "a", "d"),
        Some(&service),
        &PresentData,
        &restarted_reader,
    )
    .await;
    assert!(result.retired);
    assert_eq!(result.retirement_request_id(), Some(admitted.id()));
    restarted_reader.result(&result).unwrap();
    assert!(restarted_reader
        .fence()
        .unwrap()
        .unwrap()
        .applies_to(&running(INSTANCE, "a", "d")));
}

#[tokio::test]
async fn admitted_failure_survives_rejected_requests_and_successful_retry_keeps_original_cause() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let (service, _, _, _) = fixture(ConversationLimits::default());
    service
        .create(conversation_id(), caller(), None)
        .await
        .unwrap();
    let original = request(INSTANCE);
    let failed = retire(
        original.clone(),
        running(INSTANCE, "a", "c"),
        Some(&service),
        &PresentData,
        &RejectingAudit,
    )
    .await;
    assert!(!failed.retired);
    assert_eq!(failed.retirement_request_id(), Some(original.id()));
    files.result(&failed).unwrap();
    let evidence = files.fence().unwrap().unwrap();
    assert!(!evidence.confirmed());
    assert_eq!(evidence.retirement_request_id(), original.id());
    let (restarted_service, _, _, _) = fixture(ConversationLimits::default());
    restore_retirement(
        Some(&evidence),
        &running(&Uuid::new_v4().to_string(), "a", "c"),
        Some(&restarted_service),
    )
    .await
    .unwrap();
    assert!(restarted_service
        .create(conversation_id(), caller(), None)
        .await
        .is_err());
    assert_eq!(
        restarted_service.retirement_cause().unwrap().request_id(),
        original.id()
    );

    let wrong_instance = request(&Uuid::new_v4().to_string());
    let rejected = retire(
        wrong_instance,
        running(INSTANCE, "a", "c"),
        Some(&service),
        &PresentData,
        &files,
    )
    .await;
    assert!(rejected.retirement_request_id().is_none());
    assert!(files.result(&rejected).is_err());
    assert_eq!(
        files.evidence().unwrap().unwrap().retirement_request_id(),
        original.id()
    );
    assert!(service
        .create(conversation_id(), caller(), None)
        .await
        .is_err());

    let retried = retire(
        request(INSTANCE),
        running(INSTANCE, "a", "c"),
        Some(&service),
        &PresentData,
        &files,
    )
    .await;
    assert!(retried.retired);
    assert_eq!(retried.retirement_request_id(), Some(original.id()));
    files.result(&retried).unwrap();
    assert_eq!(
        files.fence().unwrap().unwrap().retirement_request_id(),
        original.id()
    );
}

#[tokio::test]
async fn cleanup_failure_restores_admission_fence_with_original_correlation() {
    let root = tempfile::tempdir().unwrap();
    let files = RetirementFiles::new(root.path(), Arc::new(TestClock)).unwrap();
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    service
        .create(conversation_id(), caller(), None)
        .await
        .unwrap();
    *provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    let original = request(INSTANCE);
    let failed = retire(
        original.clone(),
        running(INSTANCE, "a", "c"),
        Some(&service),
        &PresentData,
        &files,
    )
    .await;
    assert!(!failed.retired);
    assert!(failed.cleanup_error.is_some());
    assert!(failed.audit_error.is_none());
    files.result(&failed).unwrap();

    let fence = files.fence().unwrap().unwrap();
    assert!(!fence.confirmed());
    assert_eq!(fence.retirement_request_id(), original.id());
    let (restarted_service, _, _, _) = fixture(ConversationLimits::default());
    restore_retirement(
        Some(&fence),
        &running(&Uuid::new_v4().to_string(), "a", "c"),
        Some(&restarted_service),
    )
    .await
    .unwrap();
    assert!(restarted_service
        .create(conversation_id(), caller(), None)
        .await
        .is_err());
    assert_eq!(
        restarted_service.retirement_cause().unwrap().request_id(),
        original.id()
    );
}

/// The conversation store is where it was opened.
struct PresentData;
impl crate::desktop_runtime::application::ConversationData for PresentData {
    fn missing(&self) -> bool {
        false
    }
}
