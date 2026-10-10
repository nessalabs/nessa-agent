//! Current mode intent, terminal and retirement publication through public service APIs.
use super::*;
use crate::conversation_test_support::{
    fixture, mode_fixture, mode_fixture_with_summaries, MemoryRepository, MemorySummaries,
    ModeReadbackFault, ModeRecordChange, ModeTerminalFault, ProviderFactory,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use std::future::{poll_fn, Future};
use std::task::Poll;
use std::time::Duration;
use tokio::sync::oneshot;

fn caller() -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "desktop".into(),
        action_id: Uuid::new_v4().to_string(),
    }
}
async fn current(
    service: &ConversationService,
    id: ConversationId,
    caller: ConversationCaller,
) -> Result<ConversationView, ConversationError> {
    service
        .read_at(id, caller, ReadOpening::LiveOnly)
        .await
        .map(|(view, _)| view)
}
fn unchanged_view_except_selection_and_revision(
    before: &ConversationView,
    after: &ConversationView,
) {
    let mut before = serde_json::to_value(before).unwrap();
    let mut after = serde_json::to_value(after).unwrap();
    before.as_object_mut().unwrap().remove("revision");
    after.as_object_mut().unwrap().remove("revision");
    assert_eq!(
        before, after,
        "a mode change needs no transcript or lifecycle side effect"
    );
}

#[tokio::test]
async fn confirmed_mode_changes_revise_current_reads_for_normal_and_recovered_commits() {
    for lose_ack in [false, true] {
        let (service, provider, repository, _, _) = mode_fixture();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(id.clone(), caller(), RequestedConversation::default())
            .await
            .unwrap();
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
            .await
            .unwrap();
        let before = service.read(id.clone(), caller()).await.unwrap();
        assert!(before.messages.is_empty());
        assert_eq!(
            current(&service, id.clone(), caller())
                .await
                .unwrap()
                .revision,
            before.revision
        );
        let updates = provider.mode_updates.lock().unwrap().len();
        repository
            .lose_mode_commit_ack
            .store(lose_ack, Ordering::SeqCst);
        let actor = caller();
        service
            .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
            .await
            .unwrap();
        let after = service.read(id.clone(), caller()).await.unwrap();
        let follow_view = current(&service, id.clone(), caller()).await.unwrap();
        assert_eq!(
            after.selection.as_ref().unwrap().approval_mode,
            ConversationApprovalMode::Auto
        );
        assert_eq!(
            follow_view.selection.as_ref().unwrap().approval_mode,
            ConversationApprovalMode::Auto
        );
        assert_eq!(follow_view.revision, after.revision);
        unchanged_view_except_selection_and_revision(&before, &after);
        assert_ne!(
            after.revision, before.revision,
            "confirmed selection must revise its publication (lost ack: {lose_ack})"
        );
        assert_eq!(provider.mode_updates.lock().unwrap().len(), updates + 1);
        service
            .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
            .await
            .unwrap();
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
            .await
            .unwrap();
        let repeated = service.read(id.clone(), caller()).await.unwrap();
        assert_eq!(repeated.revision, after.revision);
        assert_eq!(
            current(&service, id, caller()).await.unwrap().revision,
            after.revision
        );
        assert_eq!(provider.mode_updates.lock().unwrap().len(), updates + 1);
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn rejected_and_unconfirmed_mode_changes_do_not_publish_requested_selection() {
    for fail in ["availability", "intent", "effect", "commit"] {
        let (service, provider, repository) = if fail == "availability" {
            let (service, provider, repository, _) = fixture(ConversationLimits::default());
            (service, provider, repository)
        } else {
            let (service, provider, repository, _, _) = mode_fixture();
            (service, provider, repository)
        };
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(id.clone(), caller(), RequestedConversation::default())
            .await
            .unwrap();
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
            .await
            .unwrap();
        let before = service.read(id.clone(), caller()).await.unwrap();
        match fail {
            "availability" => (),
            "intent" => repository
                .lose_mode_intent_ack
                .store(true, Ordering::SeqCst),
            "effect" => {
                *provider.mode_failure.lock().unwrap() =
                    Some(AgentError::Protocol("unconfirmed".into()))
            }
            "commit" => repository
                .refuse_mode_commit_once
                .store(true, Ordering::SeqCst),
            _ => unreachable!(),
        }
        let error = service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
            .await
            .unwrap_err();
        if fail == "availability" {
            assert!(matches!(error, ConversationError::ApprovalModeUnavailable));
        } else {
            assert!(matches!(error, ConversationError::ApprovalModeUncertain));
        }
        let after = service.read(id.clone(), caller()).await.unwrap();
        let follow_view = current(&service, id, caller()).await.unwrap();
        assert_eq!(
            after.selection.as_ref().unwrap().approval_mode,
            ConversationApprovalMode::Ask,
            "{fail}"
        );
        assert_eq!(
            follow_view.selection.as_ref().unwrap().approval_mode,
            ConversationApprovalMode::Ask,
            "{fail}"
        );
        assert!(after.messages.is_empty());
        if fail == "availability" {
            assert_eq!(after.revision, before.revision);
        }
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn admitted_mode_change_outlives_caller_and_revises_only_after_acknowledgement() {
    let (service, provider, _, _, _) = mode_fixture();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .unwrap();
    let before = service.read(id.clone(), caller()).await.unwrap();
    let updates = provider.mode_updates.lock().unwrap().len();
    let (release, gate) = oneshot::channel();
    *provider.mode_gate.lock().unwrap() = Some(gate);
    let changer = service.clone();
    let changing_id = id.clone();
    let actor = caller();
    let retry_actor = actor.clone();
    let change = tokio::spawn(async move {
        changer
            .set_approval_mode(changing_id, actor, ConversationApprovalMode::Auto)
            .await
    });
    provider.mode_started.notified().await;
    let reading = current(&service, id.clone(), caller());
    tokio::pin!(reading);
    assert!(poll_fn(|cx| Poll::Ready(reading.as_mut().poll(cx)))
        .await
        .is_pending());
    change.abort();
    release.send(()).unwrap();
    let after = tokio::time::timeout(Duration::from_secs(3), reading)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(
        after.selection.as_ref().unwrap().approval_mode,
        ConversationApprovalMode::Auto
    );
    assert_ne!(after.revision, before.revision);
    unchanged_view_except_selection_and_revision(&before, &after);
    service
        .set_approval_mode(id.clone(), retry_actor, ConversationApprovalMode::Auto)
        .await
        .unwrap();
    assert_eq!(
        current(&service, id, caller()).await.unwrap().revision,
        after.revision
    );
    assert_eq!(provider.mode_updates.lock().unwrap().len(), updates + 1);
    service.shutdown().await.unwrap();
}

async fn ready_mode_case() -> (
    ConversationService,
    Arc<ProviderFactory>,
    Arc<MemoryRepository>,
    ConversationId,
    ConversationView,
    ConversationCaller,
) {
    let (service, provider, repository, _, _) = mode_fixture();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .unwrap();
    let before = service.read(id.clone(), caller()).await.unwrap();
    (service, provider, repository, id, before, caller())
}
async fn terminal_publication(
    service: &ConversationService,
    repository: &MemoryRepository,
    id: &ConversationId,
    mode: ConversationApprovalMode,
) -> ConversationView {
    let after = service.read(id.clone(), caller()).await.unwrap();
    let follow_view = current(service, id.clone(), caller()).await.unwrap();
    assert_eq!(after.selection.as_ref().unwrap().approval_mode, mode);
    assert_eq!(follow_view.selection.as_ref().unwrap().approval_mode, mode);
    assert_eq!(follow_view.revision, after.revision);
    assert_eq!(
        repository.load(id).await.unwrap().unwrap().approval_mode(),
        mode
    );
    assert!(after.messages.is_empty());
    after
}
#[tokio::test]
async fn terminal_committed_invocation_and_poll_panics_reconcile_before_publication() {
    for fault in [ModeTerminalFault::InvokeAfter, ModeTerminalFault::PollAfter] {
        let (service, provider, repository, id, before, actor) = ready_mode_case().await;
        let mutations = provider.mode_updates.lock().unwrap().len();
        *repository.mode_terminal_fault.lock().unwrap() = Some(fault);
        assert_eq!(
            service
                .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
                .await
                .unwrap(),
            ConversationApprovalMode::Auto
        );
        let after =
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
        assert_ne!(after.revision, before.revision);
        unchanged_view_except_selection_and_revision(&before, &after);
        assert_eq!(
            service
                .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                .await
                .unwrap(),
            ConversationApprovalMode::Auto
        );
        assert_eq!(
            service.read(id, caller()).await.unwrap().revision,
            after.revision
        );
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
        service.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn terminal_uncommitted_panics_recover_prior_and_allow_a_later_mode() {
    for fault in [
        ModeTerminalFault::InvokeBefore,
        ModeTerminalFault::PollBefore,
    ] {
        let (service, provider, repository, id, _, actor) = ready_mode_case().await;
        let mutations = provider.mode_updates.lock().unwrap().len();
        *repository.mode_terminal_fault.lock().unwrap() = Some(fault);
        assert!(matches!(
            service
                .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
                .await,
            Err(ConversationError::ApprovalModeUncertain)
        ));
        let recovered =
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
        assert!(recovered.approval_mode_change.is_none());
        assert!(matches!(
            service
                .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                .await,
            Err(ConversationError::ApprovalModeNotApplied)
        ));
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
            .await
            .unwrap();
        let after =
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
        assert_ne!(after.revision, recovered.revision);
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 2);
        service.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn terminal_unreadable_readback_retires_the_held_owner_before_recovery() {
    for fault in [
        ModeReadbackFault::Refuse,
        ModeReadbackFault::Absent,
        ModeReadbackFault::InvokePanic,
        ModeReadbackFault::PollPanic,
    ] {
        for committed in [false, true] {
            let (service, provider, repository, id, before, actor) = ready_mode_case().await;
            let mutations = provider.mode_updates.lock().unwrap().len();
            if committed {
                repository
                    .lose_mode_commit_ack
                    .store(true, Ordering::SeqCst);
            } else {
                *repository.mode_terminal_fault.lock().unwrap() =
                    Some(ModeTerminalFault::PollBefore);
            }
            *repository.mode_readback_fault.lock().unwrap() = Some(fault);
            assert!(
                matches!(
                    service
                        .set_approval_mode(
                            id.clone(),
                            actor.clone(),
                            ConversationApprovalMode::Auto
                        )
                        .await,
                    Err(ConversationError::ApprovalModeUncertain)
                ),
                "{fault:?} committed={committed}"
            );
            let expected = if committed {
                ConversationApprovalMode::Auto
            } else {
                ConversationApprovalMode::Ask
            };
            let after = terminal_publication(&service, &repository, &id, expected).await;
            if committed {
                assert_ne!(after.revision, before.revision);
                assert!(service
                    .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                    .await
                    .is_ok());
            } else {
                assert!(matches!(
                    service
                        .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                        .await,
                    Err(ConversationError::ApprovalModeNotApplied)
                ));
            }
            assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
            service.shutdown().await.unwrap();
        }
    }
}
#[tokio::test]
async fn terminal_each_record_fact_is_correlated_on_write_and_readback() {
    for change in ModeRecordChange::ALL {
        for readback in [false, true] {
            let (service, provider, repository, id, before, actor) = ready_mode_case().await;
            let mutations = provider.mode_updates.lock().unwrap().len();
            if readback {
                repository
                    .lose_mode_commit_ack
                    .store(true, Ordering::SeqCst);
                *repository.mode_readback_fault.lock().unwrap() =
                    Some(ModeReadbackFault::Record(change));
            } else {
                *repository.mode_terminal_fault.lock().unwrap() =
                    Some(ModeTerminalFault::Record(change));
            }
            assert!(
                matches!(
                    service
                        .set_approval_mode(
                            id.clone(),
                            actor.clone(),
                            ConversationApprovalMode::Auto
                        )
                        .await,
                    Err(ConversationError::ApprovalModeUncertain)
                ),
                "{change:?} readback={readback}"
            );
            let after =
                terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto)
                    .await;
            assert_ne!(after.revision, before.revision);
            assert!(service
                .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                .await
                .is_ok());
            assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
            service.shutdown().await.unwrap();
        }
    }
}
#[tokio::test]
async fn terminal_retry_rejects_each_foreign_query_identity_before_effects() {
    for change in [ModeRecordChange::Conversation, ModeRecordChange::Request] {
        let (service, provider, repository, id, _, actor) = ready_mode_case().await;
        service
            .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
            .await
            .unwrap();
        let before =
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
        let mutations = provider.mode_updates.lock().unwrap().len();
        *repository.mode_readback_fault.lock().unwrap() = Some(ModeReadbackFault::Record(change));
        assert!(
            matches!(
                service
                    .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
                    .await,
                Err(ConversationError::RequestConflict)
            ),
            "{change:?}"
        );
        assert_eq!(
            service.read(id.clone(), caller()).await.unwrap().revision,
            before.revision
        );
        assert!(service
            .set_approval_mode(id, actor, ConversationApprovalMode::Auto)
            .await
            .is_ok());
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations);
        service.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn terminal_committed_panic_settles_after_the_caller_is_dropped() {
    let (service, provider, repository, id, before, actor) = ready_mode_case().await;
    let mutations = provider.mode_updates.lock().unwrap().len();
    *repository.mode_terminal_fault.lock().unwrap() = Some(ModeTerminalFault::PollAfter);
    let began = Arc::new(Notify::new());
    let (release, gate) = oneshot::channel();
    *repository.mode_terminal_gate.lock().unwrap() = Some((began.clone(), gate));
    let changer = service.clone();
    let changing_id = id.clone();
    let dropped_actor = actor.clone();
    let change = tokio::spawn(async move {
        changer
            .set_approval_mode(changing_id, dropped_actor, ConversationApprovalMode::Auto)
            .await
    });
    began.notified().await;
    change.abort();
    release.send(()).unwrap();
    let after = tokio::time::timeout(
        Duration::from_secs(3),
        terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto),
    )
    .await
    .unwrap();
    assert_ne!(after.revision, before.revision);
    assert!(service
        .set_approval_mode(id, actor, ConversationApprovalMode::Auto)
        .await
        .is_ok());
    assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_admitted_intent_cannot_be_replaced_before_provider_effects() {
    for change in ModeRecordChange::ALL {
        let (service, provider, repository, id, _, actor) = ready_mode_case().await;
        let mutations = provider.mode_updates.lock().unwrap().len();
        *repository.mode_intent_change.lock().unwrap() = Some(change);
        assert!(
            service
                .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                .await
                .is_err(),
            "{change:?}"
        );
        terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
        assert_eq!(
            provider.mode_updates.lock().unwrap().len(),
            mutations,
            "{change:?}"
        );
        service.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn terminal_inverse_intent_cannot_claim_ask_over_an_actual_auto_provider() {
    let (service, provider, repository, id, _, _) = ready_mode_case().await;
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
        .await
        .unwrap();
    let mutations = provider.mode_updates.lock().unwrap().len();
    *repository.mode_intent_change.lock().unwrap() = Some(ModeRecordChange::Prior);
    assert!(service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .is_err());
    terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
    assert_eq!(
        service
            .resolve(&id, &caller())
            .await
            .unwrap()
            .agent
            .approval_mode(),
        Some(ProviderApprovalMode::Auto)
    );
    assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations);
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .unwrap();
    terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
    assert_eq!(
        service
            .resolve(&id, &caller())
            .await
            .unwrap()
            .agent
            .approval_mode(),
        Some(ProviderApprovalMode::Ask)
    );
    assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn terminal_application_audit_keeps_original_intent_and_observed_effect() {
    for change in ModeRecordChange::ALL {
        let (service, provider, repository, _, audit) = mode_fixture();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(id.clone(), caller(), RequestedConversation::default())
            .await
            .unwrap();
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
            .await
            .unwrap();
        let before = service.read(id.clone(), caller()).await.unwrap();
        let actor = caller();
        let mutations = provider.mode_updates.lock().unwrap().len();
        *repository.mode_application_change.lock().unwrap() = Some(change);
        service
            .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
            .await
            .unwrap();
        let after =
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
        assert_ne!(after.revision, before.revision);
        let request = {
            let records = audit.records.lock().unwrap();
            records
                .iter()
                .find(|(request, phase)| {
                    request.request_id == actor.action_id
                        && *phase == ConversationModeAuditPhase::Application
                })
                .unwrap()
                .0
                .clone()
        };
        let terminal = repository
            .mode_requests
            .lock()
            .unwrap()
            .get(&(id.clone(), actor.action_id))
            .unwrap()
            .clone();
        assert_eq!(
            request,
            ConversationModeRequest {
                state: ConversationModeRequestState::Pending,
                ..terminal
            }
        );
        assert_eq!(
            request.application,
            Some(ConversationModeApplication::Applied)
        );
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
        service.shutdown().await.unwrap();
    }
}
#[tokio::test]
async fn terminal_initial_query_rejects_borrowed_receipt_before_any_effect() {
    for change in [ModeRecordChange::Conversation, ModeRecordChange::Request] {
        let (service, provider, repository, id, before, actor) = ready_mode_case().await;
        let receipt = ConversationModeRequest {
            conversation_id: id.clone(),
            organization_id: actor.organization_id.clone(),
            request_id: actor.action_id.clone(),
            initiator_principal_id: actor.principal_id.clone(),
            initiator_surface_id: actor.surface_id.clone(),
            prior: ConversationApprovalMode::Ask,
            requested: ConversationApprovalMode::Auto,
            state: ConversationModeRequestState::Applied,
            application: Some(ConversationModeApplication::Applied),
            requested_at_ms: 1,
        };
        *repository.mode_lookup_reply.lock().unwrap() = Some(change.changed(receipt));
        let mutations = provider.mode_updates.lock().unwrap().len();
        assert!(
            matches!(
                service
                    .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                    .await,
                Err(ConversationError::RequestConflict)
            ),
            "{change:?}"
        );
        let after =
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
        assert_eq!(after.revision, before.revision);
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations);
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_rejected_intent_stays_blocked_until_opening_read_and_survives_close() {
    let (service, provider, repository, id, _, actor) = ready_mode_case().await;
    let mutations = provider.mode_updates.lock().unwrap().len();
    *repository.mode_intent_change.lock().unwrap() = Some(ModeRecordChange::Prior);
    assert!(matches!(
        service
            .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
            .await,
        Err(ConversationError::ApprovalModeUncertain)
    ));
    let pending = current(&service, id.clone(), caller()).await.unwrap();
    assert_eq!(
        pending.selection.as_ref().unwrap().approval_mode,
        ConversationApprovalMode::Ask
    );
    assert!(matches!(
        pending.approval_mode_change.unwrap().status,
        ConversationApprovalModeChangeStatus::Changing
    ));
    assert!(!pending.capabilities.queue);
    assert!(matches!(
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
            .await,
        Err(ConversationError::ApprovalModeUncertain)
    ));
    let recovered =
        terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
    assert!(recovered.approval_mode_change.is_none());
    assert!(matches!(
        service
            .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
            .await,
        Err(ConversationError::ApprovalModeNotApplied)
    ));
    assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations);
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
        .await
        .unwrap();
    *repository.mode_intent_change.lock().unwrap() = Some(ModeRecordChange::Prior);
    assert!(service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .is_err());
    service.close(id.clone(), caller()).await.unwrap();
    assert!(repository.pending_mode_change(&id).await.unwrap().is_some());
    let saved = current(&service, id.clone(), caller()).await.unwrap();
    assert_eq!(
        saved.selection.as_ref().unwrap().approval_mode,
        ConversationApprovalMode::Auto
    );
    assert!(saved.approval_mode_change.is_some());
    assert!(!saved.capabilities.queue);
    let recovered =
        terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
    assert!(recovered.approval_mode_change.is_none());
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .unwrap();
    terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
    assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 2);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn terminal_unconfirmed_cleanup_keeps_admission_closed_until_confirmed_close() {
    let (service, provider, repository, id, _, actor) = ready_mode_case().await;
    let mutations = provider.mode_updates.lock().unwrap().len();
    let opens = provider.open_calls.load(Ordering::SeqCst);
    repository
        .lose_mode_commit_ack
        .store(true, Ordering::SeqCst);
    *repository.mode_readback_fault.lock().unwrap() = Some(ModeReadbackFault::Refuse);
    *provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
    assert!(matches!(
        service
            .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
            .await,
        Err(ConversationError::ApprovalModeUncertain)
    ));
    assert!(service
        .inner
        .conversations
        .lock()
        .await
        .get(&id)
        .unwrap()
        .stopping
        .load(Ordering::SeqCst));
    assert!(service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .is_err());
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
    assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
    assert!(service
        .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
        .await
        .is_ok());
    assert!(matches!(
        service
            .submit(
                id.clone(),
                caller(),
                "after-uncertain-retirement".into(),
                SubmittedMessage {
                    text: "not admitted".into(),
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue
            )
            .await,
        Err(ConversationError::Agent(AgentError::Closed))
    ));
    *provider.close_failure.lock().unwrap() = None;
    service.close(id.clone(), caller()).await.unwrap();
    terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .unwrap();
    terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
    assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 2);
    service.shutdown().await.unwrap();
}
#[tokio::test]
async fn terminal_saved_state_without_application_is_not_claimed_as_a_receipt() {
    for (state, application) in [
        (ConversationModeRequestState::Applied, None),
        (
            ConversationModeRequestState::Applied,
            Some(ConversationModeApplication::Uncertain),
        ),
        (ConversationModeRequestState::NotApplied, None),
    ] {
        let (service, provider, repository, id, before, actor) = ready_mode_case().await;
        *repository.mode_lookup_reply.lock().unwrap() = Some(ConversationModeRequest {
            conversation_id: id.clone(),
            organization_id: actor.organization_id.clone(),
            request_id: actor.action_id.clone(),
            initiator_principal_id: actor.principal_id.clone(),
            initiator_surface_id: actor.surface_id.clone(),
            prior: ConversationApprovalMode::Ask,
            requested: ConversationApprovalMode::Auto,
            state,
            application,
            requested_at_ms: 1,
        });
        let mutations = provider.mode_updates.lock().unwrap().len();
        assert!(matches!(
            service
                .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                .await,
            Err(ConversationError::ApprovalModeUncertain)
        ));
        let after =
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
        assert_eq!(after.revision, before.revision);
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations);
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_recovery_audits_cannot_borrow_replacement_metadata_facts() {
    for change in ModeRecordChange::ALL {
        let (service, provider, repository, _, audit) = mode_fixture();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(id.clone(), caller(), RequestedConversation::default())
            .await
            .unwrap();
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
            .await
            .unwrap();
        service.read(id.clone(), caller()).await.unwrap();
        let actor = caller();
        let mutations = provider.mode_updates.lock().unwrap().len();
        repository
            .lose_mode_intent_ack
            .store(true, Ordering::SeqCst);
        assert!(matches!(
            service
                .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
                .await,
            Err(ConversationError::ApprovalModeUncertain)
        ));
        let original = repository
            .mode_requests
            .lock()
            .unwrap()
            .get(&(id.clone(), actor.action_id.clone()))
            .unwrap()
            .clone();
        *repository.mode_application_change.lock().unwrap() = Some(change);
        terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
        let expected = ConversationModeRequest {
            application: Some(ConversationModeApplication::Uncertain),
            ..original
        };
        {
            let records = audit.records.lock().unwrap();
            for phase in [
                ConversationModeAuditPhase::Application,
                ConversationModeAuditPhase::RecoveryRestored,
            ] {
                let (record, _) = records
                    .iter()
                    .find(|(record, found)| record.request_id == actor.action_id && *found == phase)
                    .unwrap();
                assert_eq!(record, &expected, "{change:?} {phase:?}");
            }
        }
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations);
        assert!(matches!(
            service
                .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                .await,
            Err(ConversationError::ApprovalModeNotApplied)
        ));
        service
            .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
            .await
            .unwrap();
        terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
        assert_eq!(provider.mode_updates.lock().unwrap().len(), mutations + 1);
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn terminal_pending_query_is_correlated_before_cleanup_and_publication() {
    for field in [
        ModeRecordChange::Conversation,
        ModeRecordChange::Organization,
        ModeRecordChange::Principal,
        ModeRecordChange::Prior,
        ModeRecordChange::State,
    ] {
        for persistent in [false, true] {
            let (service, provider, repository, _, audit) = mode_fixture();
            let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
            service
                .create(id.clone(), caller(), RequestedConversation::default())
                .await
                .unwrap();
            service
                .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
                .await
                .unwrap();
            service.read(id.clone(), caller()).await.unwrap();
            let opens = provider.open_calls.load(Ordering::SeqCst);
            let closes = provider.close_calls.load(Ordering::SeqCst);
            let audits = audit.records.lock().unwrap().len();
            repository
                .lose_mode_intent_ack
                .store(true, Ordering::SeqCst);
            assert!(service
                .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
                .await
                .is_err());
            *repository.mode_pending_change.lock().unwrap() = Some((field, persistent));
            let reading = service.read(id.clone(), caller()).await;
            if persistent {
                assert!(
                    matches!(reading, Err(ConversationError::ApprovalModeUncertain)),
                    "{field:?}"
                );
            } else {
                assert!(reading.unwrap().approval_mode_change.is_some());
            }
            assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
            assert_eq!(provider.close_calls.load(Ordering::SeqCst), closes);
            assert_eq!(audit.records.lock().unwrap().len(), audits);
            if persistent {
                assert!(
                    matches!(
                        current(&service, id.clone(), caller()).await,
                        Err(ConversationError::ApprovalModeUncertain)
                    ),
                    "{field:?}"
                );
            }
            service.close(id.clone(), caller()).await.unwrap();
            assert_eq!(provider.close_calls.load(Ordering::SeqCst), closes + 1);
            *repository.mode_pending_change.lock().unwrap() = None;
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
            service
                .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
                .await
                .unwrap();
            terminal_publication(&service, &repository, &id, ConversationApprovalMode::Auto).await;
            service.shutdown().await.unwrap();
        }
    }
}
#[tokio::test]
async fn terminal_pending_recovery_keeps_historical_actor_time_and_action() {
    let (service, provider, repository, _, audit) = mode_fixture();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    let actor = ConversationCaller {
        surface_id: "previous-window".into(),
        ..caller()
    };
    repository
        .lose_mode_intent_ack
        .store(true, Ordering::SeqCst);
    assert!(service
        .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
        .await
        .is_err());
    let original = {
        let mut requests = repository.mode_requests.lock().unwrap();
        let old = requests
            .get(&(id.clone(), actor.action_id.clone()))
            .unwrap()
            .clone();
        let historical = ConversationModeRequest {
            requested_at_ms: 1,
            ..old
        };
        requests.insert((id.clone(), actor.action_id.clone()), historical.clone());
        historical
    };
    terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
    {
        let records = audit.records.lock().unwrap();
        let expected = ConversationModeRequest {
            application: Some(ConversationModeApplication::Uncertain),
            ..original
        };
        for phase in [
            ConversationModeAuditPhase::Application,
            ConversationModeAuditPhase::RecoveryRestored,
        ] {
            assert!(records
                .iter()
                .any(|(record, found)| *found == phase && record == &expected));
        }
    }
    assert!(provider.mode_updates.lock().unwrap().is_empty());
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn terminal_loaded_conversation_must_be_the_pending_query_target() {
    let (service, provider, repository, id, _, _) = ready_mode_case().await;
    repository
        .lose_mode_intent_ack
        .store(true, Ordering::SeqCst);
    assert!(service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
        .await
        .is_err());
    let opens = provider.open_calls.load(Ordering::SeqCst);
    let closes = provider.close_calls.load(Ordering::SeqCst);
    *repository.mode_loaded_reply.lock().unwrap() = Some(
        Conversation::new(
            ConversationId::new(&Uuid::from_u128(2).to_string()).unwrap(),
            OrganizationId::new("org").unwrap(),
            PrincipalId::new("person").unwrap(),
            "desktop".into(),
            "other-create".into(),
            1,
            AgentId::Claude,
            ConversationModelId::new("test").unwrap(),
            ConversationApprovalMode::Ask,
        )
        .unwrap(),
    );
    let blocked = service.read(id.clone(), caller()).await.unwrap();
    assert!(blocked.approval_mode_change.is_some());
    assert!(!blocked.capabilities.queue);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), closes);
    terminal_publication(&service, &repository, &id, ConversationApprovalMode::Ask).await;
    service.shutdown().await.unwrap();
}

#[tokio::test]
async fn retained_stopping_publication_refuses_current_reads_until_confirmed_cleanup() {
    for committed in [false, true] {
        for fault in [
            ModeReadbackFault::Refuse,
            ModeReadbackFault::InvokePanic,
            ModeReadbackFault::PollPanic,
        ] {
            let (service, provider, repository, id, _, actor) = ready_mode_case().await;
            let opens = provider.open_calls.load(Ordering::SeqCst);
            let changes = provider.mode_updates.lock().unwrap().len();
            if committed {
                repository
                    .lose_mode_commit_ack
                    .store(true, Ordering::SeqCst);
            } else {
                *repository.mode_terminal_fault.lock().unwrap() =
                    Some(ModeTerminalFault::PollBefore);
            }
            *repository.mode_readback_fault.lock().unwrap() = Some(fault);
            *provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
            let mut live_changes = service.live_changes(&id);
            assert!(matches!(
                service
                    .set_approval_mode(id.clone(), actor.clone(), ConversationApprovalMode::Auto)
                    .await,
                Err(ConversationError::ApprovalModeUncertain)
            ));
            live_changes.borrow_and_update();
            // No private strong inspection handle pins the lease across cleanup.
            for opening in [ReadOpening::Open, ReadOpening::LiveOnly] {
                let read = service.read_at(id.clone(), caller(), opening).await;
                match read {
                    Err(ConversationError::Agent(AgentError::Closed)) => {}
                    Ok((view, _)) if !committed => {
                        assert!(view.approval_mode_change.is_some());
                        assert!(!view.capabilities.queue && !view.capabilities.steer);
                    }
                    other => {
                        panic!("{fault:?} committed={committed} opening={opening:?}: {other:?}")
                    }
                }
            }
            let follow_view = current(&service, id.clone(), caller()).await;
            match follow_view {
                Err(ConversationError::Agent(AgentError::Closed)) => {}
                Ok(follow_view) if !committed => {
                    assert!(follow_view.approval_mode_change.is_some());
                    assert!(!follow_view.capabilities.queue && !follow_view.capabilities.steer);
                }
                _ => panic!("current follow published an unmarked stopped owner"),
            }
            if committed {
                service
                    .set_approval_mode(id.clone(), actor, ConversationApprovalMode::Auto)
                    .await
                    .unwrap();
                assert!(matches!(
                    service.read(id.clone(), caller()).await,
                    Err(ConversationError::Agent(AgentError::Closed))
                ));
            }
            assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
            assert_eq!(provider.mode_updates.lock().unwrap().len(), changes + 1);
            *provider.close_failure.lock().unwrap() = None;
            service.close(id.clone(), caller()).await.unwrap();
            assert!(live_changes.has_changed().unwrap());
            let expected = if committed {
                ConversationApprovalMode::Auto
            } else {
                ConversationApprovalMode::Ask
            };
            terminal_publication(&service, &repository, &id, expected).await;
            service.shutdown().await.unwrap();
        }
    }
}

#[tokio::test(start_paused = true)]
async fn retained_stopping_publication_rechecks_owner_after_summary_wait() {
    for confirmed in [false, true] {
        let summaries = Arc::new(MemorySummaries::default());
        let (service, provider, _, _, _) = mode_fixture_with_summaries(summaries.clone());
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(id.clone(), caller(), RequestedConversation::default())
            .await
            .unwrap();
        service.read(id.clone(), caller()).await.unwrap();
        let (entered, waiting) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        *summaries.load_gate.lock().unwrap() = Some((entered, gate));
        let reader = service.clone();
        let reading_id = id.clone();
        let reading = tokio::spawn(async move { current(&reader, reading_id, caller()).await });
        waiting.await.unwrap();
        if !confirmed {
            *provider.close_failure.lock().unwrap() = Some(AgentError::CleanupUncertain);
        }
        let mut changes = service.live_changes(&id);
        changes.borrow_and_update();
        let close = service.shutdown().await;
        assert!(matches!(
            close,
            Err(ConversationError::RetirementAdmission { .. })
        ));
        if confirmed {
            assert!(
                changes.has_changed().unwrap(),
                "confirmed release wakes the existing follow"
            );
        }
        release.send(()).unwrap();
        assert!(matches!(
            reading.await.unwrap(),
            Err(ConversationError::Agent(AgentError::Closed))
        ));
        *provider.close_failure.lock().unwrap() = None;
        service.stop_active_agents().await.unwrap();
    }
}
fn foreign_metadata() -> Conversation {
    Conversation::new(
        ConversationId::new(&Uuid::from_u128(2).to_string()).unwrap(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
        "historical-window".into(),
        "foreign-create".into(),
        1,
        AgentId::Claude,
        ConversationModelId::new("foreign-model").unwrap(),
        ConversationApprovalMode::Auto,
    )
    .unwrap()
}
#[tokio::test]
async fn metadata_query_target_fences_effect_entry_paths() {
    let (service, provider, repository, _, audit) = mode_fixture();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller(), RequestedConversation::default())
        .await
        .unwrap();
    service
        .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Ask)
        .await
        .unwrap();
    let opens = provider.open_calls.load(Ordering::SeqCst);
    let closes = provider.close_calls.load(Ordering::SeqCst);
    let updates = provider.mode_updates.lock().unwrap().len();
    let audited = audit.records.lock().unwrap().len();
    let mut accepted_foreign = Vec::new();
    for entry in [
        "create",
        "recover",
        "mode",
        "close",
        "archive",
        "delete",
        "live-only",
    ] {
        *repository.mode_loaded_reply.lock().unwrap() = Some(foreign_metadata());
        let result = match entry {
            "create" => service
                .create(id.clone(), caller(), RequestedConversation::default())
                .await
                .map(|_| ()),
            "recover" => service.read(id.clone(), caller()).await.map(|_| ()),
            "mode" => service
                .set_approval_mode(id.clone(), caller(), ConversationApprovalMode::Auto)
                .await
                .map(|_| ()),
            "close" => service.close(id.clone(), caller()).await,
            "archive" => service
                .archive(id.clone(), caller(), true)
                .await
                .map(|_| ()),
            "delete" => service.delete(id.clone(), caller()).await.map(|_| ()),
            "live-only" => service
                .read_at(id.clone(), caller(), ReadOpening::LiveOnly)
                .await
                .map(|_| ()),
            _ => unreachable!(),
        };
        let refused = if entry == "delete" {
            matches!(result, Err(ConversationError::Metadata))
        } else {
            matches!(
                result,
                Err(ConversationError::Storage(StorageError::IdentityMismatch))
            )
        };
        if !refused {
            accepted_foreign.push(entry);
        }
    }
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), opens);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), closes);
    assert_eq!(provider.mode_updates.lock().unwrap().len(), updates);
    assert_eq!(audit.records.lock().unwrap().len(), audited);
    assert!(repository
        .records
        .lock()
        .unwrap()
        .get(&id)
        .unwrap()
        .deletion()
        .is_none());
    service.shutdown().await.unwrap();
    assert!(
        accepted_foreign.is_empty(),
        "foreign metadata authorized {accepted_foreign:?}"
    );
}

#[tokio::test]
async fn metadata_query_target_fences_publication_after_summary_await() {
    for opening in [ReadOpening::Open, ReadOpening::LiveOnly] {
        let summaries = Arc::new(MemorySummaries::default());
        let (service, _, repository, _, _) = mode_fixture_with_summaries(summaries.clone());
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(id.clone(), caller(), RequestedConversation::default())
            .await
            .unwrap();
        service.read(id.clone(), caller()).await.unwrap();
        let (entered, waiting) = oneshot::channel();
        let (release, gate) = oneshot::channel();
        *summaries.load_gate.lock().unwrap() = Some((entered, gate));
        let reader = service.clone();
        let target = id.clone();
        let reading = tokio::spawn(async move { reader.read_at(target, caller(), opening).await });
        waiting.await.unwrap();
        *repository.mode_loaded_reply.lock().unwrap() = Some(foreign_metadata());
        release.send(()).unwrap();
        assert!(matches!(
            reading.await.unwrap(),
            Err(ConversationError::Storage(StorageError::IdentityMismatch))
        ));
        assert_eq!(
            service
                .read_at(id.clone(), caller(), opening)
                .await
                .unwrap()
                .0
                .conversation_id,
            id.to_string()
        );
        service.shutdown().await.unwrap();
    }
}
