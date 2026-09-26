//! Deterministic admission/retirement ordering without timing sleeps.
use super::*;
use crate::conversation_test_support::fixture;
use crate::desktop_runtime::{
    application::{retire, RetirementAudit, RetirementRecord},
    domain::{RetirementRequest, RunningRuntime},
};
use nessa_sdk::application::agent_execution::providers::SessionCloseRequest;
use std::{future::Future, pin::Pin};

#[tokio::test]
async fn retirement_joins_admitted_commands_and_never_reopens_admission() {
    let (service, _, _, _) = fixture(ConversationLimits::default());
    let admitted = service.admit().await.unwrap();
    let retiring = service.clone();
    let task = tokio::spawn(async move { retiring.retire("gateway_upgrade", "upgrade-one").await });
    while service.inner.retirement.get().is_none() {
        tokio::task::yield_now().await;
    }
    assert!(!task.is_finished());
    drop(admitted);
    task.await.unwrap().unwrap();
    assert!(matches!(
        service.admit().await,
        Err(ConversationError::Unavailable)
    ));
    service
        .retire("gateway_upgrade", "upgrade-one")
        .await
        .unwrap();
}

#[tokio::test(start_paused = true)]
async fn unsettled_admission_refuses_retirement_instead_of_claiming_cleanup() {
    let (service, _, _, _) = fixture(ConversationLimits::default());
    let admitted = service.admit().await.unwrap();
    assert!(matches!(
        service.retire("gateway_upgrade", "upgrade-one").await,
        Err(ConversationError::RetirementAdmission {
            cleanup_error: None
        })
    ));
    drop(admitted);
    service
        .retire("gateway_upgrade", "upgrade-one")
        .await
        .unwrap();
}

#[tokio::test]
async fn retirement_keeps_failed_owner_and_audit_errors_separate() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            ConversationCaller {
                organization_id: OrganizationId::new("org").unwrap(),
                principal_id: PrincipalId::new("person").unwrap(),
                surface_id: "panel".into(),
                action_id: "create".into(),
            },
            None,
        )
        .await
        .unwrap();
    *provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    let error = service
        .retire("gateway_upgrade", "upgrade-one")
        .await
        .unwrap_err();
    assert!(
        matches!(error, ConversationError::Retirement(ref errors) if errors.len() == 1 && errors[0].0 == id.to_string())
    );
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    *provider.close_failure.lock().unwrap() = None;
    service
        .retire("gateway_upgrade", "upgrade-one")
        .await
        .unwrap();
    assert!(service.admit().await.is_err());
}

#[tokio::test]
async fn failed_audit_does_not_prevent_cleanup_and_both_failures_are_returned() {
    struct Refuse;
    impl RetirementAudit for Refuse {
        fn record(
            &self,
            record: RetirementRecord,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            assert!(record.cleanup_error.is_some());
            Box::pin(async { Err("audit unavailable".into()) })
        }
    }
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    service
        .create(
            ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
            ConversationCaller {
                organization_id: OrganizationId::new("org").unwrap(),
                principal_id: PrincipalId::new("person").unwrap(),
                surface_id: "panel".into(),
                action_id: "create".into(),
            },
            None,
        )
        .await
        .unwrap();
    *provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
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
        Some(&service),
        &PresentData,
        &Refuse,
    )
    .await;
    assert!(!result.retired);
    assert!(result.cleanup_error.is_some());
    assert_eq!(result.audit_error.as_deref(), Some("audit unavailable"));
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    let expected = SessionCloseRequest::Explicit(
        ActionContext::new("gateway", "gateway_upgrade", request.id()).unwrap(),
    );
    assert_eq!(provider.close_requests.lock().unwrap()[0], expected);
    *provider.close_failure.lock().unwrap() = None;
    service
        .retire("gateway_upgrade", request.id())
        .await
        .unwrap();
    assert!(provider
        .close_requests
        .lock()
        .unwrap()
        .iter()
        .all(|actual| actual == &expected));
}

#[tokio::test(start_paused = true)]
async fn process_shutdown_still_attempts_cleanup_when_admission_cannot_drain() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    service
        .create(
            ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
            ConversationCaller {
                organization_id: OrganizationId::new("org").unwrap(),
                principal_id: PrincipalId::new("person").unwrap(),
                surface_id: "panel".into(),
                action_id: "create".into(),
            },
            None,
        )
        .await
        .unwrap();
    let admitted = service.admit().await.unwrap();
    assert!(matches!(
        service.shutdown().await,
        Err(ConversationError::RetirementAdmission {
            cleanup_error: None
        })
    ));
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    drop(admitted);
}

#[tokio::test]
async fn retirement_joins_an_opening_owner_even_after_its_caller_disconnects() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let (release, held) = tokio::sync::oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(held);
    let creating = service.clone();
    let create = tokio::spawn(async move {
        creating
            .create(
                ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
                ConversationCaller {
                    organization_id: OrganizationId::new("org").unwrap(),
                    principal_id: PrincipalId::new("person").unwrap(),
                    surface_id: "panel".into(),
                    action_id: "create".into(),
                },
                None,
            )
            .await
    });
    provider.opening.notified().await;
    create.abort();
    let retiring = service.clone();
    let retirement =
        tokio::spawn(async move { retiring.retire("gateway_upgrade", "upgrade-one").await });
    while service.inner.retirement.get().is_none() {
        tokio::task::yield_now().await;
    }
    assert!(!retirement.is_finished());
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    release.send(()).unwrap();
    retirement.await.unwrap().unwrap();
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    assert!(service.admit().await.is_err());
}

const INSTANCE: &str = "b4a38c5b-cf70-4d90-9059-7d9a3a51c658";

#[tokio::test(start_paused = true)]
async fn stalled_owner_cannot_starve_other_cleanup_and_retry_retains_original_cause() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let mut ids = Vec::new();
    for _ in 0..2 {
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        ids.push(id.to_string());
        service
            .create(
                id,
                ConversationCaller {
                    organization_id: OrganizationId::new("org").unwrap(),
                    principal_id: PrincipalId::new("person").unwrap(),
                    surface_id: "panel".into(),
                    action_id: "create".into(),
                },
                None,
            )
            .await
            .unwrap();
    }
    let (release, stalled) = tokio::sync::oneshot::channel();
    *provider.close_gate.lock().unwrap() = Some(stalled);
    let error = service
        .retire("gateway_upgrade", "original-upgrade")
        .await
        .unwrap_err();
    assert!(matches!(error, ConversationError::Retirement(ref errors)
        if errors.len() == 1 && ids.contains(&errors[0].0) && errors[0].1 == AgentError::Deadline));
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 2);
    release.send(()).unwrap();
    service
        .retire("gateway_upgrade", "retry-upgrade")
        .await
        .unwrap();
    assert_eq!(
        service.retirement_cause().unwrap().request_id(),
        "original-upgrade"
    );
    assert!(provider
        .close_requests
        .lock()
        .unwrap()
        .iter()
        .all(|request| request
            == &SessionCloseRequest::Explicit(
                ActionContext::new("gateway", "gateway_upgrade", "original-upgrade").unwrap()
            )));
}

#[tokio::test(start_paused = true)]
async fn stalled_opening_owner_cannot_prevent_another_owner_cleanup_or_later_retry() {
    let (service, provider, _, _) = fixture(ConversationLimits::default());
    let actor = ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: "create".into(),
    };
    service
        .create(
            ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
            actor.clone(),
            None,
        )
        .await
        .unwrap();
    provider.opening.notified().await;
    let blocked_id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let (release, stalled) = tokio::sync::oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(stalled);
    let creating = service.clone();
    let creating_id = blocked_id.clone();
    let create = tokio::spawn(async move { creating.create(creating_id, actor, None).await });
    provider.opening.notified().await;
    let error = service
        .retire("gateway_upgrade", "opening-upgrade")
        .await
        .unwrap_err();
    assert!(matches!(error, ConversationError::Retirement(ref errors)
        if errors.len() == 1 && errors[0].0 == blocked_id.to_string() && errors[0].1 == AgentError::Deadline));
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 1);
    release.send(()).unwrap();
    create.await.unwrap().unwrap();
    service
        .retire("gateway_upgrade", "later-request")
        .await
        .unwrap();
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 2);
    assert_eq!(
        service.retirement_cause().unwrap().request_id(),
        "opening-upgrade"
    );
}

/// The conversation store is where it was opened.
struct PresentData;
impl crate::desktop_runtime::application::ConversationData for PresentData {
    fn missing(&self) -> bool {
        false
    }
}

/// ADR 221: a gateway says its data is missing only when nothing it started
/// still holds resources, by the SDK's own cleanup fact, and its data is gone.
/// Which error a failed stop returned decides nothing: a provider may call an
/// unconfirmed cleanup an `AuditFailure`, and a released one may still fail.
#[tokio::test]
async fn data_missing_needs_released_resources_as_well_as_missing_data() {
    use crate::desktop_runtime::domain::RetirementRefusal;
    use nessa_sdk::application::agent_execution::providers::{
        CleanupReport, CloseOutcome, ResourceCleanup,
    };
    struct Accept;
    impl RetirementAudit for Accept {
        fn record(
            &self,
            _: RetirementRecord,
        ) -> Pin<Box<dyn Future<Output = Result<(), String>> + Send + '_>> {
            Box::pin(async { Ok(()) })
        }
    }
    struct Data(bool);
    impl crate::desktop_runtime::application::ConversationData for Data {
        fn missing(&self) -> bool {
            self.0
        }
    }
    let released = |error: AgentError| {
        CleanupReport::new(
            ResourceCleanup::Confirmed(CloseOutcome { forced: false }),
            Err(error),
        )
    };
    let held = |error: AgentError| CleanupReport::unconfirmed(error);

    for (report, missing, expected) in [
        (
            released(AgentError::AuditFailure),
            true,
            RetirementRefusal::DataMissing,
        ),
        (
            released(AgentError::AuditFailure),
            false,
            RetirementRefusal::NotConfirmed,
        ),
        (
            held(AgentError::CleanupUncertain),
            true,
            RetirementRefusal::NotConfirmed,
        ),
        // The same variant as the released case, with resources still held.
        (
            held(AgentError::AuditFailure),
            true,
            RetirementRefusal::NotConfirmed,
        ),
    ] {
        let (service, provider, _, _) = fixture(ConversationLimits::default());
        service
            .create(
                ConversationId::new(&Uuid::new_v4().to_string()).unwrap(),
                ConversationCaller {
                    organization_id: OrganizationId::new("org").unwrap(),
                    principal_id: PrincipalId::new("person").unwrap(),
                    surface_id: "panel".into(),
                    action_id: "create".into(),
                },
                None,
            )
            .await
            .unwrap();
        let described = format!("{report:?} missing={missing}");
        provider.close_reports.lock().unwrap().push_back(report);
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
            Some(&service),
            &Data(missing),
            &Accept,
        )
        .await;
        assert!(!result.retired, "{described}");
        assert_eq!(result.refusal, Some(expected), "{described}");
    }
}
