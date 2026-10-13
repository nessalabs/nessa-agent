//! The session owns reversible historical tools, review reservations and allocation slots.
use super::*;
use crate::domain::agent_execution::{
    permissions::{
        CustomPermissionCancellationReason, PermissionDecision, PermissionEffect,
        PermissionOfferPolicy, PermissionOption, PermissionOptions, PermissionScope,
        RefusalOutcome,
    },
    tools::{FileLocation, FilePath, ToolContent, ToolContentView, ToolKind, ToolStatus},
};
use std::panic::{catch_unwind, AssertUnwindSafe};

fn execution() -> ExecutionId {
    ExecutionId::new("execution").unwrap()
}
fn tool_id() -> ToolCallId {
    ToolCallId::new("tool").unwrap()
}
fn update(title: &str) -> ToolCallUpdate {
    ToolCallUpdate::new(
        tool_id(),
        Some(title.into()),
        Some(ToolKind::Read),
        Some(ToolStatus::Running),
        Some(vec![FileLocation::new(
            FilePath::new("source.rs").unwrap(),
            Some(7),
        )]),
        Some(vec![ToolContent::text("retained content")]),
    )
}
fn request(name: &str) -> PermissionRequest {
    PermissionRequest::new(
        PermissionId::new(name).unwrap(),
        execution(),
        tool_id(),
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
fn active() -> ExecutionSession {
    let mut session = ExecutionSession::new(ExecutionSessionId::new("context").unwrap());
    session.begin_execution(execution()).unwrap();
    session
}

#[test]
fn historical_tool_rollback_restores_sparse_fields_and_removes_new_entities() {
    let mut session = active();
    let insertion = session
        .observe_tool_reversible(&execution(), update("original"))
        .unwrap();
    let observation = session.tool(&tool_id()).unwrap().observation();
    let title_pointer = observation.title().as_ref().unwrap().as_ptr();
    let ToolContentView::Text(text) = observation.content().as_ref().unwrap()[0].view() else {
        panic!("fixture contains text");
    };
    let text_pointer = text.as_ptr();
    let before = observation.clone();
    let bytes = session.tool_payload_bytes(&tool_id());
    let replacement = ToolCallUpdate::new(
        tool_id(),
        Some(String::new()),
        Some(ToolKind::Edit),
        Some(ToolStatus::Completed),
        Some(Vec::new()),
        Some(Vec::new()),
    );
    let predicted = session
        .tool_payload_bytes_after(&execution(), &replacement)
        .unwrap();
    let undo = session
        .observe_tool_reversible(&execution(), replacement)
        .unwrap();
    assert_eq!(session.tool_payload_bytes(&tool_id()), predicted);
    assert_eq!(
        session.tool(&tool_id()).unwrap().observation().kind(),
        &Some(ToolKind::Edit)
    );
    assert_eq!(
        session
            .observe_tool_reversible(&ExecutionId::new("foreign").unwrap(), update("refused"))
            .map(drop),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(session.tool_payload_bytes(&tool_id()), predicted);
    session.restore_historical_tool(undo);
    assert_eq!(session.tool(&tool_id()).unwrap().observation(), &before);
    assert_eq!(session.tool_payload_bytes(&tool_id()), bytes);
    let restored = session.tool(&tool_id()).unwrap().observation();
    assert_eq!(restored.title().as_ref().unwrap().as_ptr(), title_pointer);
    let ToolContentView::Text(text) = restored.content().as_ref().unwrap()[0].view() else {
        panic!("rollback restores text");
    };
    assert_eq!(text.as_ptr(), text_pointer);
    assert_eq!(
        session.tool(&tool_id()).unwrap().execution_id(),
        &execution()
    );
    session.restore_historical_tool(insertion);
    assert_eq!(session.tool_count(), 0);
    assert_eq!(session.tool_payload_bytes(&tool_id()), 0);
    session.observe_tool(&execution(), update("retry")).unwrap();
    assert_eq!(
        session
            .tool(&tool_id())
            .unwrap()
            .observation()
            .title()
            .as_deref(),
        Some("retry")
    );
}

#[test]
fn historical_permission_rollback_releases_only_the_new_reserved_identity() {
    let mut session = active();
    session
        .observe_tool(&execution(), update("original"))
        .unwrap();
    for name in ["old", "staged"] {
        session.request_permission(request(name)).unwrap();
        let answered = session
            .answer_permission(
                &execution(),
                &PermissionId::new(name).unwrap(),
                &PermissionOptionId::new("allow").unwrap(),
            )
            .unwrap();
        assert_eq!(answered.id().as_str(), name);
        assert_eq!(answered.execution_id(), &execution());
        assert_eq!(answered.tool_id(), &tool_id());
    }
    assert_eq!(
        session.request_permission(request("staged")),
        Err(ExecutionError::DuplicatePermission)
    );
    let retained = session.historical_retained_bytes();
    session.restore_historical_permission_identity(
        &execution(),
        &PermissionId::new("staged").unwrap(),
    );
    assert_eq!(session.permission_count(), 1);
    assert_eq!(session.permission_payload_bytes(), "old".len());
    assert_eq!(
        session.historical_retained_bytes(),
        retained - "staged".len()
    );
    assert_eq!(
        session.request_permission(request("old")),
        Err(ExecutionError::DuplicatePermission)
    );
    session.request_permission(request("staged")).unwrap();
    assert_eq!(session.permission_count(), 2);
    assert!(session
        .permission_authority()
        .unwrap()
        .pending(&PermissionId::new("staged").unwrap())
        .unwrap());
}

#[test]
fn historical_accounting_retains_collection_capacity_and_reports_poisoned_owner() {
    let idle = ExecutionSession::new(ExecutionSessionId::new("context").unwrap());
    assert_eq!(idle.historical_retained_bytes(), "context".len());
    let mut session = active();
    session
        .observe_tool(&execution(), update("original"))
        .unwrap();
    session.request_permission(request("review")).unwrap();
    let before = session.historical_retained_bytes();
    let active = session.active_execution.as_ref().unwrap();
    let expected = "context".len()
        + "execution".len() * 2
        + session.seen_execution_ids.capacity() * (size_of::<ExecutionId>() + size_of::<usize>())
        + active.tools.capacity() * (size_of::<(ToolCallId, ToolCall)>() + size_of::<usize>())
        + active.seen_permission_ids.capacity() * (size_of::<PermissionId>() + size_of::<usize>())
        + "review".len()
        + size_of::<Mutex<HashMap<PermissionId, PermissionRequest>>>()
        + 2 * size_of::<usize>()
        + active.pending_permissions.lock().unwrap().capacity()
            * (size_of::<(PermissionId, PermissionRequest)>() + size_of::<usize>());
    assert_eq!(before, expected);
    session
        .answer_permission(
            &execution(),
            &PermissionId::new("review").unwrap(),
            &PermissionOptionId::new("allow").unwrap(),
        )
        .unwrap();
    assert_eq!(session.historical_retained_bytes(), before);
    let pending = session
        .active_execution
        .as_ref()
        .unwrap()
        .pending_permissions
        .clone();
    assert!(catch_unwind(AssertUnwindSafe(|| {
        let _guard = pending.lock().unwrap();
        panic!("poison historical owner");
    }))
    .is_err());
    assert_eq!(session.historical_retained_bytes(), usize::MAX);
}

#[test]
fn a_hook_refusal_code_withdraws_only_the_review_it_was_issued_for() {
    let mut session = active();
    session
        .observe_tool(&execution(), update("original"))
        .unwrap();
    for name in ["refused", "other"] {
        session.request_permission(request(name)).unwrap();
    }
    let refused = PermissionId::new("refused").unwrap();
    let other = PermissionId::new("other").unwrap();
    let issued = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::hook_refusal(
            &execution(),
            &refused,
            RefusalOutcome::EndTurn,
            "hook refused",
        )
        .unwrap(),
    );
    // A copy of the recorded reason, as evidence exposes it, cannot withdraw
    // another review, nor every review at once.
    assert_eq!(
        session
            .cancel_permission(&execution(), &other, issued.clone())
            .unwrap_err(),
        ExecutionError::ReservedCancellationCode
    );
    assert_eq!(
        session
            .cancel_permissions(&execution(), issued.clone())
            .unwrap_err(),
        ExecutionError::ReservedCancellationCode
    );
    // A saved reason is rebuilt with no review, so it withdraws none.
    let saved = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::restore("hook-refusal.end_turn", "hook refused")
            .unwrap(),
    );
    assert_eq!(saved, issued);
    assert_eq!(
        session
            .cancel_permission(&execution(), &refused, saved)
            .unwrap_err(),
        ExecutionError::ReservedCancellationCode
    );
    assert_eq!(session.permission_count(), 2, "both reviews stay pending");
    assert!(session
        .cancel_permission(&execution(), &refused, issued)
        .unwrap()
        .is_some());
    assert!(session
        .cancel_permission(
            &execution(),
            &other,
            PermissionCancellationReason::custom(
                CustomPermissionCancellationReason::new("guard", "guard withdrew").unwrap(),
            ),
        )
        .unwrap()
        .is_some());
}

#[test]
fn a_saved_hook_refusal_code_must_be_one_the_runner_writes() {
    for name in ["continue", "end_turn", "close_session"] {
        CustomPermissionCancellationReason::restore(format!("hook-refusal.{name}"), "saved")
            .unwrap();
    }
    // The runner's own reason is held to the same bounds.
    assert_eq!(
        CustomPermissionCancellationReason::hook_refusal(
            &execution(),
            &PermissionId::new("review").unwrap(),
            RefusalOutcome::Continue,
            " ",
        )
        .unwrap_err(),
        ExecutionError::EmptyValue("cancellation reason explanation")
    );
    for code in [
        "hook-refusal.",
        "hook-refusal.retry",
        "hook-refusal.end_turn.x",
    ] {
        assert_eq!(
            CustomPermissionCancellationReason::restore(code, "saved").unwrap_err(),
            ExecutionError::ReservedCancellationCode
        );
    }
}

#[test]
fn a_hook_refusal_code_does_not_withdraw_a_review_with_the_same_id_in_a_later_turn() {
    let mut session = active();
    session
        .observe_tool(&execution(), update("original"))
        .unwrap();
    session.request_permission(request("review")).unwrap();
    let review = PermissionId::new("review").unwrap();
    let issued = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::hook_refusal(
            &execution(),
            &review,
            RefusalOutcome::Continue,
            "hook refused",
        )
        .unwrap(),
    );
    assert!(session
        .cancel_permission(&execution(), &review, issued.clone())
        .unwrap()
        .is_some());
    session
        .finish_execution(&execution(), Ok(ExecutionOutcome::Completed))
        .unwrap();
    // A provider may reuse a review id in the next turn.
    let later = ExecutionId::new("later").unwrap();
    session.begin_execution(later.clone()).unwrap();
    session.observe_tool(&later, update("again")).unwrap();
    session
        .request_permission(PermissionRequest::new(
            review.clone(),
            later.clone(),
            tool_id(),
            request("unused").options().clone(),
        ))
        .unwrap();
    assert_eq!(
        session
            .cancel_permission(&later, &review, issued)
            .unwrap_err(),
        ExecutionError::ReservedCancellationCode
    );
    assert_eq!(
        session.permission_count(),
        1,
        "the later review stays pending"
    );
}
