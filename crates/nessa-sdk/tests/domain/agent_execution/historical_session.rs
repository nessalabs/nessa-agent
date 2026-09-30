//! The session owns reversible historical tools, review reservations and allocation slots.
use super::*;
use crate::domain::agent_execution::{
    permissions::{
        PermissionDecision, PermissionEffect, PermissionOfferPolicy, PermissionOption,
        PermissionOptions, PermissionScope,
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
