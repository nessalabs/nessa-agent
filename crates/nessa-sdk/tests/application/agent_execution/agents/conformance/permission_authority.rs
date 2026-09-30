//! Generation replacement cannot revive a stale custom backend owner.
use super::*;

#[tokio::test]
async fn permission_query_refuses_a_generation_replaced_during_acquisition() {
    let (agent, backend, _) = workflow().await;
    let execution = ExecutionId::new("old").unwrap();
    let permission = PermissionId::new("review").unwrap();
    let mut controller =
        ExecutionController::new(ExecutionSessionId::new("workflow-context").unwrap());
    controller.begin_execution(execution.clone()).unwrap();
    let options = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow").unwrap(),
            "Allow",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap();
    controller
        .request_permission(
            &execution,
            permission.clone(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            ToolReviewInput {
                name: "Write".into(),
                arguments_json: "{}".into(),
            },
            options,
        )
        .unwrap();
    let source = controller.permission_authority_source();
    let old_handle = source.read().unwrap().unwrap();
    *backend.permission_authority.lock().unwrap() = Some(source);
    let (_finish, held) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(held);
    let running = Mode::Direct.start(agent.clone(), request("old"));
    bounded(backend.dispatched.notified()).await;
    assert!(agent.pending_permission(&execution, &permission).unwrap());
    assert!(!agent
        .pending_permission(&ExecutionId::new("foreign").unwrap(), &permission)
        .unwrap());
    let (entered, observing) = std::sync::mpsc::channel();
    let (release, held) = std::sync::mpsc::channel();
    *backend.authority_gate.lock().unwrap() = Some((entered, held));
    let querying = tokio::task::spawn_blocking({
        let agent = agent.clone();
        let execution = execution.clone();
        let permission = permission.clone();
        move || agent.pending_permission(&execution, &permission)
    });
    bounded(tokio::task::spawn_blocking(move || {
        observing.recv().unwrap()
    }))
    .await
    .unwrap();
    bounded(agent.close(close_action())).await.unwrap();
    assert_eq!(
        bounded(running).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    let authorization = agent
        .authorize_attachment(AttachmentRequest::CallerRequested(close_action()))
        .unwrap();
    bounded(agent.start_attachment(authorization).unwrap().wait())
        .await
        .unwrap();
    let (_finish, held) = oneshot::channel();
    *backend.execution_gate.lock().unwrap() = Some(held);
    let next = Mode::Direct.start(agent.clone(), request("new"));
    bounded(backend.dispatched.notified()).await;
    assert!(
        old_handle.pending(&permission).unwrap(),
        "adversarial custom backend still owns its old collection"
    );
    release.send(()).unwrap();
    assert_eq!(bounded(querying).await.unwrap(), Ok(false));
    assert_eq!(
        agent.pending_permission(&ExecutionId::new("new").unwrap(), &permission),
        Err(PermissionAuthorityError::IdentityMismatch)
    );
    bounded(agent.close(close_action())).await.unwrap();
    assert_eq!(
        bounded(next).await.unwrap(),
        Ok(ExecutionOutcome::Completed)
    );
    drop(controller);
    assert_eq!(
        old_handle.pending(&permission),
        Err(PermissionAuthorityError::Unavailable)
    );
}
