use nessa_sdk::domain::agent_execution::executions::*;
use nessa_sdk::domain::agent_execution::permissions::*;
use nessa_sdk::domain::agent_execution::sessions::*;
use nessa_sdk::domain::agent_execution::tools::*;
use nessa_sdk::domain::agent_execution::ExecutionError;

fn session() -> ExecutionSession {
    ExecutionSession::new(ExecutionSessionId::new("context").unwrap())
}
fn execution(value: &str) -> ExecutionId {
    ExecutionId::new(value).unwrap()
}
fn update(title: Option<&str>, content: Option<Vec<ToolContent>>) -> ToolCallUpdate {
    ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        title.map(str::to_owned),
        None,
        None,
        None,
        content,
    )
}
fn request() -> PermissionRequest {
    request_for("first")
}
fn request_for(execution_id: &str) -> PermissionRequest {
    PermissionRequest::new(
        PermissionId::new("review").unwrap(),
        execution(execution_id),
        ToolCallId::new("tool").unwrap(),
        PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow once",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap(),
    )
}
fn active_session() -> ExecutionSession {
    let mut session = session();
    session.begin_execution(execution("first")).unwrap();
    session
        .observe_tool(&execution("first"), update(Some("Write file"), None))
        .unwrap();
    session
}

#[test]
fn a_session_admits_sequential_executions_and_cannot_reopen_after_close() {
    assert!(ExecutionSessionId::new(" \n").is_err());
    let mut session = active_session();
    assert_eq!(session.id().as_str(), "context");
    assert_eq!(
        session.begin_execution(execution("second")),
        Err(ExecutionError::SessionBusy)
    );
    assert_eq!(
        session.finish_execution(&execution("wrong"), Ok(ExecutionOutcome::Completed)),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(session.active_execution(), Some(&execution("first")));
    session
        .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert_eq!(session.tool_count(), 0);
    assert!(session.tool(&ToolCallId::new("tool").unwrap()).is_none());
    session.begin_execution(execution("second")).unwrap();
    assert_eq!(session.tool_count(), 0);
    assert!(session
        .close(PermissionCancellationReason::session_closed())
        .unwrap()
        .into_parts()
        .1
        .is_empty());
    assert!(session.is_closed());
    session
        .finish_execution(&execution("second"), Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert_eq!(
        session.begin_execution(execution("third")),
        Err(ExecutionError::SessionClosed)
    );
    assert!(session
        .close(PermissionCancellationReason::session_closed())
        .unwrap()
        .into_parts()
        .1
        .is_empty());
}

#[test]
fn tool_observation_reuses_entity_merging_and_isolates_sessions_and_executions() {
    let mut first = active_session();
    let second = active_session();
    let patch = update(None, Some(vec![]));
    assert_eq!(
        first.observe_tool(&execution("other"), patch.clone()),
        Err(ExecutionError::DifferentExecution)
    );
    first.observe_tool(&execution("first"), patch).unwrap();
    let id = ToolCallId::new("tool").unwrap();
    assert_eq!(
        first.tool(&id).unwrap().observation().title().as_deref(),
        Some("Write file")
    );
    assert_eq!(
        first.tool(&id).unwrap().observation().content(),
        &Some(vec![])
    );
    assert_eq!(second.tool(&id).unwrap().observation().content(), &None);
}

#[test]
fn permissions_require_an_observed_tool_and_invalid_answers_leave_them_pending() {
    let mut session = session();
    session.begin_execution(execution("first")).unwrap();
    assert_eq!(
        session.request_permission(request()),
        Err(ExecutionError::UnknownTool)
    );
    session
        .observe_tool(&execution("first"), update(None, None))
        .unwrap();
    session.request_permission(request()).unwrap();
    assert_eq!(
        session.request_permission(request()),
        Err(ExecutionError::DuplicatePermission)
    );
    let id = PermissionId::new("review").unwrap();
    let option = PermissionOptionId::new("allow").unwrap();
    assert_eq!(
        session.answer_permission(&execution("wrong"), &id, &option),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(
        session.answer_permission(
            &execution("first"),
            &id,
            &PermissionOptionId::new("other").unwrap()
        ),
        Err(ExecutionError::UnknownPermissionOption)
    );
    let resolved = session
        .answer_permission(&execution("first"), &id, &option)
        .unwrap();
    assert!(matches!(
        resolved.state(),
        PermissionStateView::Answered { .. }
    ));
    assert_eq!(
        session.request_permission(resolved),
        Err(ExecutionError::PermissionResolved)
    );
    assert_eq!(
        session.answer_permission(&execution("first"), &id, &option),
        Err(ExecutionError::UnknownPermission)
    );
}

#[test]
fn finishing_or_closing_cancels_permissions_before_the_next_execution() {
    for close in [false, true] {
        let mut session = active_session();
        session.request_permission(request()).unwrap();
        let cancelled = if close {
            session
                .close(PermissionCancellationReason::session_closed())
                .unwrap()
                .into_parts()
                .1
        } else {
            session
                .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
                .unwrap()
                .1
        };
        assert_eq!(cancelled.len(), 1);
        assert_eq!(
            cancelled[0].state(),
            PermissionStateView::Cancelled {
                reason: &if close {
                    PermissionCancellationReason::session_closed()
                } else {
                    PermissionCancellationReason::execution_finished()
                }
            }
        );
        let repeated = session.cancel_permissions(
            &execution("first"),
            PermissionCancellationReason::execution_failed(),
        );
        if close {
            assert_eq!(
                repeated,
                Err(ExecutionError::InvalidPermissionCancellationReason)
            );
        } else {
            assert_eq!(repeated, Err(ExecutionError::DifferentExecution));
        }
        let id = PermissionId::new("review").unwrap();
        let option = PermissionOptionId::new("allow").unwrap();
        if close {
            assert_eq!(
                session.request_permission(request()),
                Err(ExecutionError::SessionClosed)
            );
            assert_eq!(
                session.answer_permission(&execution("first"), &id, &option),
                Err(ExecutionError::SessionClosed)
            );
        } else {
            session.begin_execution(execution("second")).unwrap();
            assert_eq!(
                session.request_permission(request()),
                Err(ExecutionError::DifferentExecution)
            );
            assert_eq!(
                session.answer_permission(&execution("second"), &id, &option),
                Err(ExecutionError::UnknownPermission)
            );
        }
    }
}

#[test]
fn answered_and_cancelled_permission_identities_cannot_be_reopened_with_replayed_requests() {
    for answer in [false, true] {
        let mut session = active_session();
        let pending = request();
        session.request_permission(request()).unwrap();
        if answer {
            session
                .answer_permission(
                    &execution("first"),
                    pending.id(),
                    &PermissionOptionId::new("allow").unwrap(),
                )
                .unwrap();
        } else {
            assert_eq!(
                session
                    .cancel_permissions(
                        &execution("first"),
                        PermissionCancellationReason::provider_withdrawal()
                    )
                    .unwrap()
                    .len(),
                1
            );
        }
        assert_eq!(
            session.request_permission(pending),
            Err(ExecutionError::DuplicatePermission)
        );
        assert_eq!(session.permission_count(), 1);
        assert!(session
            .cancel_permissions(
                &execution("first"),
                PermissionCancellationReason::provider_withdrawal()
            )
            .unwrap()
            .is_empty());

        // Permission identities belong to an execution, so a later one can reuse them.
        session
            .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
            .unwrap();
        assert_eq!(session.permission_count(), 0);
        session.begin_execution(execution("second")).unwrap();
        session
            .observe_tool(&execution("second"), update(None, None))
            .unwrap();
        session.request_permission(request_for("second")).unwrap();
        let resolved = session
            .answer_permission(
                &execution("second"),
                &PermissionId::new("review").unwrap(),
                &PermissionOptionId::new("allow").unwrap(),
            )
            .unwrap();
        assert!(matches!(
            resolved.state(),
            PermissionStateView::Answered { .. }
        ));
    }
}

#[test]
fn cancelling_one_permission_preserves_other_reviews_and_remembers_cancelled_identity() {
    let mut session = active_session();
    let pending = request();
    session.request_permission(request()).unwrap();
    let other = PermissionRequest::new(
        PermissionId::new("other").unwrap(),
        execution("first"),
        pending.tool_id().clone(),
        pending.options().clone(),
    );
    session
        .request_permission(PermissionRequest::new(
            other.id().clone(),
            other.execution_id().clone(),
            other.tool_id().clone(),
            other.options().clone(),
        ))
        .unwrap();
    assert!(session
        .cancel_permission(
            &execution("first"),
            &PermissionId::new("unknown").unwrap(),
            PermissionCancellationReason::provider_withdrawal()
        )
        .unwrap()
        .is_none());
    let cancelled = session
        .cancel_permission(
            &execution("first"),
            pending.id(),
            PermissionCancellationReason::provider_withdrawal(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        cancelled.state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::provider_withdrawal()
        }
    );
    assert!(session
        .cancel_permission(
            &execution("first"),
            pending.id(),
            PermissionCancellationReason::provider_withdrawal()
        )
        .unwrap()
        .is_none());
    assert_eq!(session.permission_count(), 2);
    assert_eq!(
        session.request_permission(request()),
        Err(ExecutionError::DuplicatePermission)
    );
    let option = PermissionOptionId::new("allow").unwrap();
    assert_eq!(
        session.answer_permission(&execution("first"), pending.id(), &option),
        Err(ExecutionError::UnknownPermission)
    );
    session
        .answer_permission(&execution("first"), other.id(), &option)
        .unwrap();
    assert!(session
        .cancel_permission(
            &execution("first"),
            other.id(),
            PermissionCancellationReason::provider_withdrawal()
        )
        .unwrap()
        .is_none());
}

#[test]
fn bulk_cancellation_keeps_the_custom_guard_cause_for_every_review() {
    let mut session = active_session();
    let first = request();
    session.request_permission(request()).unwrap();
    session
        .request_permission(PermissionRequest::new(
            PermissionId::new("second").unwrap(),
            execution("first"),
            first.tool_id().clone(),
            first.options().clone(),
        ))
        .unwrap();
    let reason = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::new(
            "guard.workspace_changed",
            "The workspace guard no longer permits these reviews.",
        )
        .unwrap(),
    );
    let cancelled = session
        .cancel_permissions(&execution("first"), reason.clone())
        .unwrap();
    assert_eq!(cancelled.len(), 2);
    for request in cancelled {
        assert_eq!(
            request.state(),
            PermissionStateView::Cancelled {
                reason: &reason.clone()
            }
        );
    }
    assert_eq!(session.permission_count(), 2);
    assert!(session
        .close(PermissionCancellationReason::session_closed())
        .unwrap()
        .into_parts()
        .1
        .is_empty());
}

#[test]
fn session_cleanup_preserves_its_supplied_cause_without_rewriting_prior_cancellations() {
    let mut session = active_session();
    session.request_permission(request()).unwrap();
    let cancelled = session
        .close(PermissionCancellationReason::deadline_exceeded())
        .unwrap()
        .into_parts()
        .1;
    assert_eq!(cancelled.len(), 1);
    assert_eq!(
        cancelled[0].state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::deadline_exceeded(),
        }
    );
    assert!(session
        .close(PermissionCancellationReason::session_closed())
        .unwrap()
        .into_parts()
        .1
        .is_empty());
    assert!(session
        .finish_execution(
            &execution("first"),
            Err(PermissionCancellationReason::deadline_exceeded())
        )
        .unwrap()
        .1
        .is_empty());
    assert_eq!(
        cancelled[0].state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::deadline_exceeded(),
        }
    );
    assert!(session.is_closed());
}

#[test]
fn permission_admission_can_be_checked_before_observing_its_tool_without_mutating_state() {
    let mut session = session();
    assert_eq!(
        session.validate_permission_admission(&request()),
        Err(ExecutionError::DifferentExecution)
    );
    session.begin_execution(execution("first")).unwrap();
    assert_eq!(session.validate_permission_admission(&request()), Ok(()));
    assert_eq!(session.tool_count(), 0);
    assert_eq!(session.permission_count(), 0);
    session
        .observe_tool(&execution("first"), update(None, None))
        .unwrap();
    session.request_permission(request()).unwrap();
    assert_eq!(
        session.validate_permission_admission(&request()),
        Err(ExecutionError::DuplicatePermission)
    );
    let cancelled = session
        .cancel_permission(
            &execution("first"),
            request().id(),
            PermissionCancellationReason::provider_withdrawal(),
        )
        .unwrap()
        .unwrap();
    assert_eq!(
        session.validate_permission_admission(&cancelled),
        Err(ExecutionError::PermissionResolved)
    );
    assert_eq!(
        session.validate_permission_admission(&request()),
        Err(ExecutionError::DuplicatePermission)
    );
    assert!(session
        .close(PermissionCancellationReason::session_closed())
        .unwrap()
        .into_parts()
        .1
        .is_empty());
    assert_eq!(
        session.validate_permission_admission(&request()),
        Err(ExecutionError::SessionClosed)
    );
}

#[test]
fn local_session_ids_are_distinct_portable_validated_keys() {
    assert_eq!(
        SessionId::new("conversation_1-a").unwrap().as_str(),
        "conversation_1-a"
    );
    for invalid in ["", " ", "../other", "a/b", "a\\b", "é"] {
        assert_eq!(
            SessionId::new(invalid),
            Err(ExecutionError::InvalidSessionId)
        );
    }
    assert!(SessionId::new("a".repeat(128)).is_ok());
    assert_eq!(
        SessionId::new("a".repeat(129)),
        Err(ExecutionError::InvalidSessionId)
    );
}

#[test]
fn closing_retains_execution_identity_and_observations_until_matching_finish() {
    let mut session = active_session();
    session.request_permission(request()).unwrap();
    let cancelled = session
        .close(PermissionCancellationReason::deadline_exceeded())
        .unwrap()
        .into_parts()
        .1;
    assert_eq!(session.active_execution(), Some(&execution("first")));
    assert_eq!(session.tool_count(), 1);
    assert_eq!(session.permission_count(), 1);
    assert_eq!(
        session.finish_execution(&execution("other"), Ok(ExecutionOutcome::Completed)),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(session.tool_count(), 1);
    assert_eq!(session.permission_count(), 1);
    assert_eq!(
        session.begin_execution(execution("new")),
        Err(ExecutionError::SessionClosed)
    );
    assert!(session
        .finish_execution(
            &execution("first"),
            Err(PermissionCancellationReason::deadline_exceeded())
        )
        .unwrap()
        .1
        .is_empty());
    assert!(session.active_execution().is_none());
    assert_eq!(session.tool_count(), 0);
    assert_eq!(session.permission_count(), 0);
    assert!(session.tool(&ToolCallId::new("tool").unwrap()).is_none());
    assert_eq!(
        session.cancel_permissions(
            &execution("first"),
            PermissionCancellationReason::execution_failed()
        ),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].execution_id(), &execution("first"));
    assert_eq!(
        cancelled[0].state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::deadline_exceeded()
        }
    );
    assert_eq!(
        session.begin_execution(execution("new")),
        Err(ExecutionError::SessionClosed)
    );
}

#[test]
fn delayed_cancellation_cannot_cancel_reused_identity_in_a_later_execution() {
    let mut session = active_session();
    session.request_permission(request()).unwrap();
    session
        .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
        .unwrap();
    session.begin_execution(execution("second")).unwrap();
    session
        .observe_tool(&execution("second"), update(Some("Second tool"), None))
        .unwrap();
    let second = request_for("second");
    session.request_permission(request_for("second")).unwrap();
    assert_eq!(
        session.cancel_permission(
            &execution("first"),
            second.id(),
            PermissionCancellationReason::provider_withdrawal()
        ),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(
        session
            .answer_permission(
                &execution("second"),
                second.id(),
                &PermissionOptionId::new("allow").unwrap()
            )
            .unwrap()
            .execution_id(),
        &execution("second")
    );
    session
        .finish_execution(&execution("second"), Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert_eq!(
        session.cancel_permission(
            &execution("second"),
            second.id(),
            PermissionCancellationReason::provider_withdrawal()
        ),
        Err(ExecutionError::DifferentExecution)
    );
}

#[test]
fn failed_finish_atomically_preserves_failure_reason_and_releases_execution() {
    let mut session = active_session();
    session.request_permission(request()).unwrap();
    let cancelled = session
        .finish_execution(
            &execution("first"),
            Err(PermissionCancellationReason::execution_failed()),
        )
        .unwrap()
        .1;
    assert_eq!(cancelled.len(), 1);
    assert_eq!(cancelled[0].execution_id(), &execution("first"));
    assert_eq!(
        cancelled[0].state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::execution_failed()
        }
    );
    assert!(session.active_execution().is_none());
    session.begin_execution(execution("second")).unwrap();
}

#[test]
fn close_retains_once_only_session_evidence_even_when_idle() {
    for active in [false, true] {
        let mut session = if active { active_session() } else { session() };
        let (closure, permissions) = session
            .close(PermissionCancellationReason::deadline_exceeded())
            .unwrap()
            .into_parts();
        assert!(permissions.is_empty());
        let closure = closure.unwrap();
        assert_eq!(closure.session_id(), session.id());
        assert_eq!(
            closure.execution_id(),
            active.then(|| execution("first")).as_ref()
        );
        assert_eq!(
            closure.reason(),
            &PermissionCancellationReason::deadline_exceeded()
        );
        let (repeated, permissions) = session
            .close(PermissionCancellationReason::session_closed())
            .unwrap()
            .into_parts();
        assert!(repeated.is_none());
        assert!(permissions.is_empty());
        assert_eq!(
            closure.reason(),
            &PermissionCancellationReason::deadline_exceeded()
        );
    }
}

#[test]
fn execution_finish_preserves_once_only_evidence_without_permissions() {
    for result in [
        Ok(ExecutionOutcome::Completed),
        Ok(ExecutionOutcome::OutputLimit),
        Ok(ExecutionOutcome::Cancelled),
        Err(PermissionCancellationReason::execution_failed()),
    ] {
        let mut session = active_session();
        let (finished, permissions) = session
            .finish_execution(&execution("first"), result.clone())
            .unwrap();
        assert!(permissions.is_empty());
        assert_eq!(finished.session_id(), session.id());
        assert_eq!(finished.execution_id(), &execution("first"));
        assert_eq!(finished.result(), &result);
        assert_eq!(session.active_execution(), None);
        assert_eq!(
            session.finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed)),
            Err(ExecutionError::DifferentExecution)
        );
        session.begin_execution(execution("second")).unwrap();
        assert_eq!(finished.execution_id(), &execution("first"));
    }
}

#[test]
fn delayed_bulk_cancellation_cannot_drain_a_later_execution() {
    let mut session = active_session();
    session.request_permission(request()).unwrap();
    let (_, first_evidence) = session
        .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
        .unwrap();
    session.begin_execution(execution("second")).unwrap();
    session
        .observe_tool(&execution("second"), update(None, None))
        .unwrap();
    session.request_permission(request_for("second")).unwrap();

    assert_eq!(
        session.cancel_permissions(
            &execution("first"),
            PermissionCancellationReason::deadline_exceeded()
        ),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(session.active_execution(), Some(&execution("second")));
    assert_eq!(session.permission_count(), 1);
    assert_eq!(session.tool_count(), 1);
    let second = session
        .answer_permission(
            &execution("second"),
            &PermissionId::new("review").unwrap(),
            &PermissionOptionId::new("allow").unwrap(),
        )
        .unwrap();
    assert_eq!(second.execution_id(), &execution("second"));
    assert_eq!(first_evidence[0].execution_id(), &execution("first"));
    assert_eq!(
        first_evidence[0].state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::execution_finished(),
        }
    );
}

#[test]
fn invalid_execution_failure_causes_preserve_pending_state_and_evidence() {
    for reason in [
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new(
                "guard.rejected",
                "Only this pending permission was rejected.",
            )
            .unwrap(),
        ),
    ] {
        let mut session = active_session();
        session.request_permission(request()).unwrap();
        assert_eq!(
            session.finish_execution(&execution("first"), Err(reason)),
            Err(ExecutionError::InvalidExecutionFailureReason)
        );
        assert_eq!(session.active_execution(), Some(&execution("first")));
        assert_eq!(session.tool_count(), 1);
        assert_eq!(session.permission_count(), 1);
        assert!(!session.is_closed());
        assert_eq!(
            session.request_permission(request()),
            Err(ExecutionError::DuplicatePermission)
        );
        let (finished, pending) = session
            .finish_execution(
                &execution("first"),
                Err(PermissionCancellationReason::execution_failed()),
            )
            .unwrap();
        assert_eq!(
            finished.result(),
            &Err(PermissionCancellationReason::execution_failed())
        );
        assert_eq!(pending.len(), 1);
        assert_eq!(
            pending[0].state(),
            PermissionStateView::Cancelled {
                reason: &PermissionCancellationReason::execution_failed(),
            }
        );
    }
}

#[test]
fn execution_cleanup_causes_preserve_terminal_and_permission_correlation() {
    for reason in [
        PermissionCancellationReason::execution_failed(),
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::session_failed(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
    ] {
        let mut session = active_session();
        session.request_permission(request()).unwrap();
        let closes_attachment = matches!(
            reason.view(),
            PermissionCancellationReasonView::SessionClosed
                | PermissionCancellationReasonView::SessionFailed
                | PermissionCancellationReasonView::DeadlineExceeded
                | PermissionCancellationReasonView::EventConsumerDropped
                | PermissionCancellationReasonView::SessionHandlesDropped
        );
        let mut pending = if closes_attachment {
            session
                .close_execution(&execution("first"), reason.clone())
                .unwrap()
                .into_parts()
                .1
        } else {
            Vec::new()
        };
        let (finished, remaining) = session
            .finish_execution(&execution("first"), Err(reason.clone()))
            .unwrap();
        pending.extend(remaining);
        assert_eq!(finished.result(), &Err(reason.clone()));
        assert_eq!(finished.execution_id(), &execution("first"));
        assert_eq!(pending.len(), 1);
        assert_eq!(pending[0].execution_id(), finished.execution_id());
        assert_eq!(
            pending[0].state(),
            PermissionStateView::Cancelled {
                reason: &reason.clone()
            }
        );
        assert_eq!(session.active_execution(), None);
        assert_eq!(session.permission_count(), 0);
        assert_eq!(session.tool_count(), 0);
        assert_eq!(session.is_closed(), closes_attachment);
        assert_eq!(
            session.begin_execution(execution("next")),
            if closes_attachment {
                Err(ExecutionError::SessionClosed)
            } else {
                Ok(())
            }
        );
    }
}

#[test]
fn closed_session_freezes_existing_and_new_tool_observations_until_finish() {
    let mut session = active_session();
    session.request_permission(request()).unwrap();
    let (closure, cancelled) = session
        .close(PermissionCancellationReason::session_closed())
        .unwrap()
        .into_parts();
    for incoming in [
        update(
            Some("changed after close"),
            Some(vec![ToolContent::text("late")]),
        ),
        ToolCallUpdate::new(
            ToolCallId::new("new-tool").unwrap(),
            None,
            None,
            None,
            None,
            None,
        ),
    ] {
        assert_eq!(
            session.observe_tool(&execution("first"), incoming),
            Err(ExecutionError::SessionClosed)
        );
        assert_eq!(session.tool_count(), 1);
        assert_eq!(
            session
                .tool(&ToolCallId::new("tool").unwrap())
                .unwrap()
                .observation()
                .title(),
            &Some("Write file".into())
        );
        assert!(session
            .tool(&ToolCallId::new("new-tool").unwrap())
            .is_none());
    }
    assert_eq!(
        closure.unwrap().reason(),
        &PermissionCancellationReason::session_closed()
    );
    assert_eq!(
        cancelled[0].state(),
        PermissionStateView::Cancelled {
            reason: &PermissionCancellationReason::session_closed()
        }
    );
    session
        .finish_execution(&execution("first"), Ok(ExecutionOutcome::Cancelled))
        .unwrap();
    assert_eq!(session.tool_count(), 0);
    assert_eq!(
        session.observe_tool(&execution("first"), update(None, None)),
        Err(ExecutionError::SessionClosed)
    );
}

#[test]
fn invalid_closure_causes_preserve_idle_active_and_already_closed_sessions() {
    for reason in [
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new("guard", "Only this review was cancelled")
                .unwrap(),
        ),
    ] {
        for initially_active in [false, true] {
            let mut session = if initially_active {
                active_session()
            } else {
                session()
            };
            if initially_active {
                session.request_permission(request()).unwrap();
            }
            assert!(matches!(
                session.close(reason.clone()),
                Err(ExecutionError::InvalidSessionClosureReason)
            ));
            assert!(!session.is_closed());
            assert_eq!(
                session.active_execution(),
                initially_active.then(|| execution("first")).as_ref()
            );
            assert_eq!(session.tool_count(), usize::from(initially_active));
            assert_eq!(session.permission_count(), usize::from(initially_active));
            let (closure, cancelled) = session
                .close(PermissionCancellationReason::deadline_exceeded())
                .unwrap()
                .into_parts();
            assert_eq!(cancelled.len(), usize::from(initially_active));
            if initially_active {
                assert_eq!(
                    cancelled[0].state(),
                    PermissionStateView::Cancelled {
                        reason: &PermissionCancellationReason::deadline_exceeded()
                    }
                );
            }
            assert!(matches!(
                session.close(reason.clone()),
                Err(ExecutionError::InvalidSessionClosureReason)
            ));
            assert!(session.is_closed());
            assert_eq!(session.tool_count(), usize::from(initially_active));
            assert_eq!(
                closure.unwrap().reason(),
                &PermissionCancellationReason::deadline_exceeded()
            );
            let (repeated, pending) = session
                .close(PermissionCancellationReason::session_closed())
                .unwrap()
                .into_parts();
            assert!(repeated.is_none());
            assert!(pending.is_empty());
        }
    }
}

#[test]
fn all_session_cleanup_causes_retain_once_only_closure_evidence() {
    for reason in [
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::session_failed(),
        PermissionCancellationReason::execution_failed(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
    ] {
        let mut session = active_session();
        session.request_permission(request()).unwrap();
        let (closure, cancelled) = session
            .close_execution(&execution("first"), reason.clone())
            .unwrap()
            .into_parts();
        let closure = closure.unwrap();
        assert_eq!(closure.execution_id(), Some(&execution("first")));
        assert_eq!(closure.reason(), &reason);
        assert_eq!(
            cancelled[0].state(),
            PermissionStateView::Cancelled {
                reason: &reason.clone()
            }
        );
        assert_eq!(session.active_execution(), Some(&execution("first")));
        let (repeated, pending) = session
            .close(PermissionCancellationReason::session_closed())
            .unwrap()
            .into_parts();
        assert!(repeated.is_none());
        assert!(pending.is_empty());
    }
}

#[test]
fn aggregate_tool_payload_includes_its_separate_key_through_updates_and_release() {
    let mut session = session();
    let execution = execution("run");
    let id = ToolCallId::new("t".repeat(8192)).unwrap();
    let update = ToolCallUpdate::new(id.clone(), Some("title".into()), None, None, None, None);
    assert_eq!(session.tool_payload_bytes(&id), 0);
    assert!(session
        .tool_payload_bytes_after(&execution, &update)
        .is_err());
    session.begin_execution(execution.clone()).unwrap();
    assert!(session
        .tool_payload_bytes_after(&ExecutionId::new("wrong").unwrap(), &update)
        .is_err());
    let expected = 3 + 2 * 8192 + 5;
    assert_eq!(
        session
            .tool_payload_bytes_after(&execution, &update)
            .unwrap(),
        expected
    );
    session.observe_tool(&execution, update).unwrap();
    assert_eq!(session.tool_payload_bytes(&id), expected);
    let sparse = ToolCallUpdate::new(id.clone(), None, None, None, None, None);
    assert_eq!(
        session
            .tool_payload_bytes_after(&execution, &sparse)
            .unwrap(),
        expected
    );
    let replacement = ToolCallUpdate::new(id.clone(), Some(String::new()), None, None, None, None);
    assert_eq!(
        session
            .tool_payload_bytes_after(&execution, &replacement)
            .unwrap(),
        expected - 5
    );
    session.observe_tool(&execution, replacement).unwrap();
    assert_eq!(session.tool_payload_bytes(&id), expected - 5);
    let _evidence = session
        .finish_execution(&execution, Ok(ExecutionOutcome::Completed))
        .unwrap();
    assert_eq!(session.tool_payload_bytes(&id), 0);
    let _evidence = session
        .close(PermissionCancellationReason::session_closed())
        .unwrap();
    assert_eq!(
        session.tool_payload_bytes_after(&execution, &sparse),
        Err(ExecutionError::SessionClosed)
    );
}

#[test]
fn execution_identity_cannot_be_reused_after_any_terminal_result() {
    for result in [
        Ok(ExecutionOutcome::Completed),
        Ok(ExecutionOutcome::Cancelled),
        Err(PermissionCancellationReason::execution_failed()),
    ] {
        let mut session = active_session();
        session.request_permission(request()).unwrap();
        let (finished, cancelled) = session
            .finish_execution(&execution("first"), result.clone())
            .unwrap();
        assert_eq!(
            session.begin_execution(execution("first")),
            Err(ExecutionError::DuplicateExecution)
        );
        assert_eq!(session.active_execution(), None);
        assert_eq!(session.tool_count(), 0);
        assert_eq!(session.permission_count(), 0);
        session.begin_execution(execution("second")).unwrap();
        session
            .observe_tool(&execution("second"), update(None, None))
            .unwrap();
        session.request_permission(request_for("second")).unwrap();
        assert_eq!(
            session.cancel_permissions(
                &execution("first"),
                PermissionCancellationReason::deadline_exceeded()
            ),
            Err(ExecutionError::DifferentExecution)
        );
        assert_eq!(
            session.observe_tool(&execution("first"), update(Some("stale"), None)),
            Err(ExecutionError::DifferentExecution)
        );
        session
            .answer_permission(
                &execution("second"),
                &PermissionId::new("review").unwrap(),
                &PermissionOptionId::new("allow").unwrap(),
            )
            .unwrap();
        assert_eq!(finished.result(), &result);
        assert_eq!(cancelled[0].execution_id(), &execution("first"));
        session
            .finish_execution(&execution("second"), Ok(ExecutionOutcome::Completed))
            .unwrap();
        assert_eq!(
            session.begin_execution(execution("first")),
            Err(ExecutionError::DuplicateExecution)
        );
        assert_eq!(
            session.begin_execution(execution("second")),
            Err(ExecutionError::DuplicateExecution)
        );
    }
}

#[test]
fn attachment_terminal_finish_requires_prior_correlated_closure_evidence() {
    for reason in [
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::session_failed(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
    ] {
        let mut session = active_session();
        session.request_permission(request()).unwrap();
        assert_eq!(
            session.finish_execution(&execution("first"), Err(reason.clone())),
            Err(ExecutionError::InvalidExecutionFailureReason)
        );
        assert!(!session.is_closed());
        assert_eq!(session.active_execution(), Some(&execution("first")));
        assert_eq!(session.permission_count(), 1);
        assert_eq!(session.tool_count(), 1);
        assert_eq!(
            session.begin_execution(execution("second")),
            Err(ExecutionError::SessionBusy)
        );
        let (closure, cancelled) = session
            .close_execution(&execution("first"), reason.clone())
            .unwrap()
            .into_parts();
        let closure = closure.unwrap();
        assert_eq!(closure.execution_id(), Some(&execution("first")));
        assert_eq!(cancelled.len(), 1);
        let (finished, remaining) = session
            .finish_execution(&execution("first"), Err(reason.clone()))
            .unwrap();
        assert_eq!(finished.execution_id(), closure.execution_id().unwrap());
        assert_eq!(finished.result(), &Err(reason.clone()));
        assert!(remaining.is_empty());
        assert_eq!(session.active_execution(), None);
        assert!(session.is_closed());
    }
}

#[test]
fn closed_attachment_does_not_make_permission_only_finish_causes_valid() {
    for reason in [
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new("guard", "Permission withdrawn").unwrap(),
        ),
    ] {
        let mut session = active_session();
        session.request_permission(request()).unwrap();
        let (closure, cancelled) = session
            .close(PermissionCancellationReason::event_consumer_dropped())
            .unwrap()
            .into_parts();
        assert_eq!(
            session.finish_execution(&execution("first"), Err(reason)),
            Err(ExecutionError::InvalidExecutionFailureReason)
        );
        assert!(session.is_closed());
        assert_eq!(session.active_execution(), Some(&execution("first")));
        assert_eq!(session.tool_count(), 1);
        assert_eq!(session.permission_count(), 1);
        assert_eq!(
            closure.unwrap().reason(),
            &PermissionCancellationReason::event_consumer_dropped()
        );
        assert_eq!(
            cancelled[0].state(),
            PermissionStateView::Cancelled {
                reason: &PermissionCancellationReason::event_consumer_dropped()
            }
        );
        let (_, remaining) = session
            .finish_execution(
                &execution("first"),
                Err(PermissionCancellationReason::event_consumer_dropped()),
            )
            .unwrap();
        assert!(remaining.is_empty());
    }
}

#[test]
fn session_terminal_permission_cancellation_requires_close_without_consuming_evidence() {
    let reasons = [
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::session_failed(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
    ];
    for reason in reasons {
        for pending in [false, true] {
            for bulk in [false, true] {
                let mut session = active_session();
                if pending {
                    session.request_permission(request()).unwrap();
                }
                let bytes = session.permission_payload_bytes();
                let cancel = |session: &mut ExecutionSession, execution_id: &ExecutionId| {
                    if bulk {
                        session
                            .cancel_permissions(execution_id, reason.clone())
                            .map(|r| r.len())
                    } else {
                        session
                            .cancel_permission(
                                execution_id,
                                &PermissionId::new("review").unwrap(),
                                reason.clone(),
                            )
                            .map(|r| usize::from(r.is_some()))
                    }
                };
                assert_eq!(
                    cancel(&mut session, &execution("wrong")),
                    Err(ExecutionError::DifferentExecution)
                );
                assert_eq!(
                    cancel(&mut session, &execution("first")),
                    Err(ExecutionError::InvalidPermissionCancellationReason)
                );
                assert!(!session.is_closed());
                assert_eq!(session.active_execution(), Some(&execution("first")));
                assert_eq!(session.permission_payload_bytes(), bytes);
                assert_eq!(session.permission_count(), usize::from(pending));
                let (closure, reviews) = session
                    .close_execution(&execution("first"), reason.clone())
                    .unwrap()
                    .into_parts();
                assert!(closure.is_some());
                assert_eq!(reviews.len(), usize::from(pending));
                for review in reviews {
                    assert_eq!(
                        review.state(),
                        PermissionStateView::Cancelled {
                            reason: &reason.clone()
                        }
                    );
                }
                assert_eq!(cancel(&mut session, &execution("first")), Ok(0));
                assert_eq!(
                    session.permission_payload_bytes(),
                    if pending { "review".len() } else { 0 }
                );
                session
                    .finish_execution(&execution("first"), Err(reason.clone()))
                    .unwrap();
                assert_eq!(session.permission_payload_bytes(), 0);
                assert_eq!(
                    cancel(&mut session, &execution("first")),
                    Err(ExecutionError::DifferentExecution)
                );
            }
        }
    }
}

#[test]
fn permission_payload_measurements_include_scoped_options_duplicate_keys_and_release() {
    let scopes = [
        PermissionScope::request(),
        PermissionScope::session(
            PermissionApplicationId::new("app").unwrap(),
            PermissionSessionId::new("surface").unwrap(),
        ),
        PermissionScope::application(PermissionApplicationId::new("app").unwrap()),
    ];
    for (scope, scoped_bytes) in scopes.into_iter().zip([0, 10, 3]) {
        for resolution in 0..5 {
            let mut session = active_session();
            assert_eq!(session.permission_payload_bytes(), 0);
            let decision = PermissionDecision::new(PermissionEffect::Allow, scope.clone());
            let mut label = String::with_capacity(4096);
            label.push_str("Allow");
            let mut choices = Vec::with_capacity(100);
            choices.push(
                PermissionOption::new(
                    PermissionOptionId::new("yes").unwrap(),
                    label,
                    decision.clone(),
                )
                .unwrap(),
            );
            let options = PermissionOptions::new(
                choices,
                &PermissionOfferPolicy::new(vec![decision]).unwrap(),
            )
            .unwrap();
            let option_bytes = std::mem::size_of::<PermissionOption>() + 3 + 5 + scoped_bytes;
            assert_eq!(options.payload_bytes(), option_bytes);
            let request = PermissionRequest::new(
                PermissionId::new("review").unwrap(),
                execution("first"),
                ToolCallId::new("tool").unwrap(),
                options,
            );
            let request_bytes = 6 + 5 + 4 + option_bytes;
            assert_eq!(request.payload_bytes(), request_bytes);
            let predicted = request_bytes + 2 * 6;
            assert_eq!(
                session.permission_payload_bytes_after(&request),
                Ok(predicted)
            );
            assert_eq!(session.permission_payload_bytes(), 0);
            session.request_permission(request).unwrap();
            assert_eq!(session.permission_payload_bytes(), predicted);
            assert_eq!(
                session.permission_payload_bytes_after(&request_for("first")),
                Err(ExecutionError::DuplicatePermission)
            );
            let evidence = match resolution {
                0 => session
                    .answer_permission(
                        &execution("first"),
                        &PermissionId::new("review").unwrap(),
                        &PermissionOptionId::new("yes").unwrap(),
                    )
                    .unwrap(),
                1 => session
                    .cancel_permission(
                        &execution("first"),
                        &PermissionId::new("review").unwrap(),
                        PermissionCancellationReason::provider_withdrawal(),
                    )
                    .unwrap()
                    .unwrap(),
                2 => session
                    .cancel_permissions(
                        &execution("first"),
                        PermissionCancellationReason::custom(
                            CustomPermissionCancellationReason::new("guard", "explanation")
                                .unwrap(),
                        ),
                    )
                    .unwrap()
                    .remove(0),
                3 => session
                    .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
                    .unwrap()
                    .1
                    .remove(0),
                _ => session
                    .close(PermissionCancellationReason::session_closed())
                    .unwrap()
                    .into_parts()
                    .1
                    .remove(0),
            };
            let extra = match resolution {
                0 => 3 + scoped_bytes,
                2 => 5 + 11,
                _ => 0,
            };
            assert_eq!(evidence.payload_bytes(), request_bytes + extra);
            assert_eq!(
                session.permission_payload_bytes(),
                if resolution == 3 { 0 } else { 6 }
            );
            if resolution != 3 {
                assert_eq!(
                    session.permission_payload_bytes_after(&evidence),
                    if resolution == 4 {
                        Err(ExecutionError::SessionClosed)
                    } else {
                        Err(ExecutionError::PermissionResolved)
                    }
                );
                session
                    .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
                    .unwrap();
            }
            assert_eq!(session.permission_payload_bytes(), 0);
            assert_eq!(
                session.permission_payload_bytes_after(&request_for("first")),
                if resolution == 4 {
                    Err(ExecutionError::SessionClosed)
                } else {
                    Err(ExecutionError::DifferentExecution)
                }
            );
        }
    }
}

#[test]
fn local_permission_cancellation_causes_preserve_open_attachment_and_reserved_ids() {
    let reasons = [
        PermissionCancellationReason::provider_withdrawal(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::custom(
            CustomPermissionCancellationReason::new("guard", "policy").unwrap(),
        ),
    ];
    for reason in reasons {
        for bulk in [false, true] {
            let mut session = active_session();
            session.request_permission(request()).unwrap();
            let requests = if bulk {
                session
                    .cancel_permissions(&execution("first"), reason.clone())
                    .unwrap()
            } else {
                vec![session
                    .cancel_permission(
                        &execution("first"),
                        &PermissionId::new("review").unwrap(),
                        reason.clone(),
                    )
                    .unwrap()
                    .unwrap()]
            };
            assert_eq!(
                requests[0].state(),
                PermissionStateView::Cancelled {
                    reason: &reason.clone()
                }
            );
            assert!(!session.is_closed());
            assert_eq!(session.permission_payload_bytes(), 6);
            assert_eq!(
                session.request_permission(request()),
                Err(ExecutionError::DuplicatePermission)
            );
        }
    }
}

#[test]
fn permission_budget_prediction_is_additive_and_does_not_admit_unknown_tools() {
    let mut session = session();
    session.begin_execution(execution("first")).unwrap();
    let bytes = session.permission_payload_bytes_after(&request()).unwrap();
    assert_eq!(
        session.request_permission(request()),
        Err(ExecutionError::UnknownTool)
    );
    assert_eq!(session.permission_payload_bytes(), 0);
    session
        .observe_tool(&execution("first"), update(None, None))
        .unwrap();
    session.request_permission(request()).unwrap();
    let second = PermissionRequest::new(
        PermissionId::new("second").unwrap(),
        execution("first"),
        ToolCallId::new("tool").unwrap(),
        request().options().clone(),
    );
    let predicted = bytes + second.payload_bytes() + 2 * "second".len();
    assert_eq!(
        session.permission_payload_bytes_after(&second),
        Ok(predicted)
    );
    session.request_permission(second).unwrap();
    assert_eq!(session.permission_payload_bytes(), predicted);
    session
        .answer_permission(
            &execution("first"),
            &PermissionId::new("review").unwrap(),
            &PermissionOptionId::new("allow").unwrap(),
        )
        .unwrap();
    assert_eq!(session.permission_payload_bytes(), bytes + "review".len());
    let (closure, remaining) = session
        .close(PermissionCancellationReason::deadline_exceeded())
        .unwrap()
        .into_parts();
    assert!(closure.is_some());
    assert_eq!(remaining.len(), 1);
    assert_eq!(remaining[0].id().as_str(), "second");
    assert_eq!(session.permission_payload_bytes(), 12);
    session
        .finish_execution(
            &execution("first"),
            Err(PermissionCancellationReason::deadline_exceeded()),
        )
        .unwrap();
    assert_eq!(session.permission_payload_bytes(), 0);
}

#[test]
fn idle_failure_closure_cannot_claim_a_failed_execution() {
    for settled_first in [false, true] {
        let mut session = session();
        if settled_first {
            session.begin_execution(execution("completed")).unwrap();
            session
                .finish_execution(&execution("completed"), Ok(ExecutionOutcome::Completed))
                .unwrap();
        }
        assert!(matches!(
            session.close(PermissionCancellationReason::execution_failed()),
            Err(ExecutionError::InvalidSessionClosureReason)
        ));
        assert!(!session.is_closed());
        assert_eq!(session.active_execution(), None);
        let (closure, cancelled) = session
            .close(PermissionCancellationReason::session_failed())
            .unwrap()
            .into_parts();
        let closure = closure.unwrap();
        assert_eq!(closure.execution_id(), None);
        assert_eq!(
            closure.reason(),
            &PermissionCancellationReason::session_failed()
        );
        assert!(cancelled.is_empty());
        assert!(matches!(
            session.close(PermissionCancellationReason::execution_failed()),
            Err(ExecutionError::InvalidSessionClosureReason)
        ));
    }
}

#[test]
fn session_failure_requires_prior_close_before_execution_or_permission_release() {
    let mut session = active_session();
    session.request_permission(request()).unwrap();
    assert!(matches!(
        session.finish_execution(
            &execution("first"),
            Err(PermissionCancellationReason::session_failed())
        ),
        Err(ExecutionError::InvalidExecutionFailureReason)
    ));
    assert!(matches!(
        session.cancel_permissions(
            &execution("first"),
            PermissionCancellationReason::session_failed()
        ),
        Err(ExecutionError::InvalidPermissionCancellationReason)
    ));
    assert_eq!(session.permission_count(), 1);
    let (closure, cancelled) = session
        .close(PermissionCancellationReason::session_failed())
        .unwrap()
        .into_parts();
    assert_eq!(closure.unwrap().execution_id(), Some(&execution("first")));
    assert_eq!(cancelled.len(), 1);
    let (finished, remaining) = session
        .finish_execution(
            &execution("first"),
            Err(PermissionCancellationReason::session_failed()),
        )
        .unwrap();
    assert_eq!(
        finished.result(),
        &Err(PermissionCancellationReason::session_failed())
    );
    assert!(remaining.is_empty());
    assert!(matches!(
        session.close(PermissionCancellationReason::execution_failed()),
        Err(ExecutionError::InvalidSessionClosureReason)
    ));
}

#[test]
fn execution_terminal_cancellation_requires_atomic_settlement() {
    for reason in [
        PermissionCancellationReason::execution_finished(),
        PermissionCancellationReason::execution_failed(),
    ] {
        for pending in [false, true] {
            for closed in [false, true] {
                for bulk in [false, true] {
                    let mut session = active_session();
                    if pending {
                        session.request_permission(request()).unwrap();
                    }
                    let closure = if closed {
                        Some(
                            session
                                .close(PermissionCancellationReason::session_closed())
                                .unwrap()
                                .into_parts(),
                        )
                    } else {
                        None
                    };
                    let bytes = session.permission_payload_bytes();
                    let cancel = |session: &mut ExecutionSession, id: &ExecutionId| {
                        if bulk {
                            session
                                .cancel_permissions(id, reason.clone())
                                .map(|reviews| reviews.len())
                        } else {
                            session
                                .cancel_permission(
                                    id,
                                    &PermissionId::new("review").unwrap(),
                                    reason.clone(),
                                )
                                .map(|review| usize::from(review.is_some()))
                        }
                    };
                    assert_eq!(
                        cancel(&mut session, &execution("wrong")),
                        Err(ExecutionError::DifferentExecution)
                    );
                    assert_eq!(
                        cancel(&mut session, &execution("first")),
                        Err(ExecutionError::InvalidPermissionCancellationReason)
                    );
                    assert_eq!(session.is_closed(), closed);
                    assert_eq!(session.active_execution(), Some(&execution("first")));
                    assert_eq!(session.permission_payload_bytes(), bytes);
                    assert_eq!(session.permission_count(), usize::from(pending));
                    let outcome = if reason == PermissionCancellationReason::execution_finished() {
                        Ok(ExecutionOutcome::Completed)
                    } else if closed {
                        Err(PermissionCancellationReason::session_closed())
                    } else {
                        Err(reason.clone())
                    };
                    let (finish, reviews) = session
                        .finish_execution(&execution("first"), outcome.clone())
                        .unwrap();
                    assert_eq!(finish.execution_id(), &execution("first"));
                    assert_eq!(finish.result(), &outcome);
                    assert_eq!(reviews.len(), usize::from(pending && !closed));
                    for review in reviews {
                        assert_eq!(
                            review.state(),
                            PermissionStateView::Cancelled {
                                reason: &reason.clone()
                            }
                        );
                    }
                    if let Some((closure, reviews)) = closure {
                        assert_eq!(
                            closure.unwrap().reason(),
                            &PermissionCancellationReason::session_closed()
                        );
                        for review in reviews {
                            assert_eq!(
                                review.state(),
                                PermissionStateView::Cancelled {
                                    reason: &PermissionCancellationReason::session_closed()
                                }
                            );
                        }
                    }
                    assert_eq!(session.permission_payload_bytes(), 0);
                    assert_eq!(session.active_execution(), None);
                    assert_eq!(
                        cancel(&mut session, &execution("first")),
                        Err(ExecutionError::DifferentExecution)
                    );
                }
            }
        }
    }
}

#[test]
fn first_closure_and_later_settlement_preserve_their_distinct_evidence() {
    let causes = [
        PermissionCancellationReason::execution_failed(),
        PermissionCancellationReason::session_failed(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::session_closed(),
        PermissionCancellationReason::event_consumer_dropped(),
        PermissionCancellationReason::session_handles_dropped(),
    ];
    let outcomes = [
        ExecutionOutcome::Completed,
        ExecutionOutcome::OutputLimit,
        ExecutionOutcome::RequestLimit,
        ExecutionOutcome::Refused,
        ExecutionOutcome::Cancelled,
    ];
    for cause in &causes {
        let results = outcomes
            .iter()
            .copied()
            .map(Ok)
            .chain(causes.iter().cloned().map(Err));
        for result in results {
            for pending in [false, true] {
                let mut session = active_session();
                assert!(session.closure().is_none());
                if pending {
                    session.request_permission(request()).unwrap();
                }
                let (closure, reviews) = session
                    .close_execution(&execution("first"), cause.clone())
                    .unwrap()
                    .into_parts();
                let closure = closure.unwrap();
                assert_eq!(session.closure(), Some(&closure));
                assert_eq!(closure.execution_id(), Some(&execution("first")));
                let repeated = session
                    .close(PermissionCancellationReason::session_closed())
                    .unwrap()
                    .into_parts();
                assert!(repeated.0.is_none());
                assert!(repeated.1.is_empty());
                assert_eq!(session.closure(), Some(&closure));
                for invalid in [
                    PermissionCancellationReason::provider_withdrawal(),
                    PermissionCancellationReason::execution_finished(),
                    PermissionCancellationReason::custom(
                        CustomPermissionCancellationReason::new("guard", "review only").unwrap(),
                    ),
                ] {
                    assert_eq!(
                        session.finish_execution(&execution("first"), Err(invalid)),
                        Err(ExecutionError::InvalidExecutionFailureReason)
                    );
                    assert_eq!(session.active_execution(), Some(&execution("first")));
                    assert_eq!(session.tool_count(), 1);
                    assert_eq!(session.permission_count(), usize::from(pending));
                    assert_eq!(session.closure(), Some(&closure));
                }
                let (finish, remaining) = session
                    .finish_execution(&execution("first"), result.clone())
                    .unwrap();
                assert_eq!(finish.result(), &result);
                assert_eq!(finish.session_id(), closure.session_id());
                assert_eq!(Some(finish.execution_id()), closure.execution_id());
                assert!(remaining.is_empty());
                assert_eq!(
                    session.begin_execution(execution("next")),
                    Err(ExecutionError::SessionClosed)
                );
                assert_eq!(session.active_execution(), None);
                assert_eq!(session.tool_count(), 0);
                assert_eq!(session.permission_count(), 0);
                assert_eq!(session.closure(), Some(&closure));
                assert_eq!(reviews.len(), usize::from(pending));
                for review in reviews {
                    assert_eq!(
                        review.state(),
                        PermissionStateView::Cancelled {
                            reason: &cause.clone()
                        }
                    );
                }
            }
        }
    }
}

#[test]
fn deadline_without_closure_cannot_release_execution_or_retained_resources() {
    for pending in [false, true] {
        let mut session = active_session();
        if pending {
            session.request_permission(request()).unwrap();
        }
        let tool_bytes = session.tool_payload_bytes(&ToolCallId::new("tool").unwrap());
        let review_bytes = session.permission_payload_bytes();
        assert_eq!(
            session.finish_execution(
                &execution("wrong"),
                Err(PermissionCancellationReason::deadline_exceeded())
            ),
            Err(ExecutionError::DifferentExecution)
        );
        assert_eq!(
            session.finish_execution(
                &execution("first"),
                Err(PermissionCancellationReason::deadline_exceeded())
            ),
            Err(ExecutionError::InvalidExecutionFailureReason)
        );
        assert_eq!(session.active_execution(), Some(&execution("first")));
        assert_eq!(session.tool_count(), 1);
        assert_eq!(
            session.tool_payload_bytes(&ToolCallId::new("tool").unwrap()),
            tool_bytes
        );
        assert_eq!(session.permission_payload_bytes(), review_bytes);
        assert_eq!(session.permission_count(), usize::from(pending));
        assert!(session.closure().is_none());
        assert_eq!(
            session.begin_execution(execution("next")),
            Err(ExecutionError::SessionBusy)
        );
        let (closure, reviews) = session
            .close(PermissionCancellationReason::deadline_exceeded())
            .unwrap()
            .into_parts();
        let closure = closure.unwrap();
        assert_eq!(closure.execution_id(), Some(&execution("first")));
        assert_eq!(
            closure.reason(),
            &PermissionCancellationReason::deadline_exceeded()
        );
        assert_eq!(reviews.len(), usize::from(pending));
        for review in reviews {
            assert_eq!(
                review.state(),
                PermissionStateView::Cancelled {
                    reason: &PermissionCancellationReason::deadline_exceeded()
                }
            );
        }
        assert_eq!(session.active_execution(), Some(&execution("first")));
        assert_eq!(session.tool_count(), 1);
        assert_eq!(session.permission_count(), usize::from(pending));
        assert_eq!(
            session.begin_execution(execution("next")),
            Err(ExecutionError::SessionClosed)
        );
        let (finish, remaining) = session
            .finish_execution(
                &execution("first"),
                Err(PermissionCancellationReason::deadline_exceeded()),
            )
            .unwrap();
        assert_eq!(
            finish.result(),
            &Err(PermissionCancellationReason::deadline_exceeded())
        );
        assert!(remaining.is_empty());
        assert_eq!(session.active_execution(), None);
        assert_eq!(session.tool_count(), 0);
        assert_eq!(session.permission_count(), 0);
        assert_eq!(session.closure(), Some(&closure));
        assert_eq!(
            session.begin_execution(execution("next")),
            Err(ExecutionError::SessionClosed)
        );
    }
}

#[test]
fn delayed_execution_closure_cannot_cancel_a_later_execution() {
    for reason in [
        PermissionCancellationReason::execution_failed(),
        PermissionCancellationReason::deadline_exceeded(),
        PermissionCancellationReason::session_closed(),
    ] {
        let mut session = active_session();
        session
            .finish_execution(&execution("first"), Ok(ExecutionOutcome::Completed))
            .unwrap();
        session.begin_execution(execution("second")).unwrap();
        session
            .observe_tool(&execution("second"), update(Some("Second tool"), None))
            .unwrap();
        session.request_permission(request_for("second")).unwrap();
        assert!(matches!(
            session.close_execution(&execution("first"), reason.clone()),
            Err(ExecutionError::DifferentExecution)
        ));
        assert!(!session.is_closed());
        assert_eq!(session.active_execution(), Some(&execution("second")));
        assert_eq!(session.permission_count(), 1);
        let (closure, cancelled) = session
            .close_execution(&execution("second"), reason.clone())
            .unwrap()
            .into_parts();
        assert_eq!(closure.unwrap().execution_id(), Some(&execution("second")));
        assert_eq!(cancelled.len(), 1);
        assert_eq!(cancelled[0].execution_id(), &execution("second"));
        assert_eq!(
            cancelled[0].state(),
            PermissionStateView::Cancelled {
                reason: &reason.clone()
            }
        );
        assert!(matches!(
            session.close_execution(&execution("first"), reason),
            Err(ExecutionError::DifferentExecution)
        ));
    }
}
