use super::*;
use crate::application::agent_execution::{
    executions::{ExecutionAuditRecord, ExecutionController, ExecutionUpdate},
    permissions::{ActionContext, CancellationOrigin},
    tools::ToolReviewInput,
};
use crate::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, MessageChunk},
    permissions::{
        CustomPermissionCancellationReason, PermissionCancellationReason, PermissionDecision,
        PermissionEffect, PermissionId, PermissionOfferPolicy, PermissionOption,
        PermissionOptionId, PermissionOptions, PermissionRequest, PermissionScope,
    },
    sessions::ExecutionSessionId,
    tools::{ToolCallId, ToolCallUpdate, ToolContent, ToolKind, ToolStatus},
};

fn text_event() -> ExecutionEvent {
    let mut text = String::with_capacity(1024);
    text.push('x');
    ExecutionEvent::new(
        ExecutionId::new("execution").unwrap(),
        ExecutionUpdate::Message(MessageChunk::text(text)),
    )
}
fn budget(bytes: usize) -> EventQueueBudget {
    EventQueueBudget {
        permits: Arc::new(Semaphore::new(bytes)),
    }
}

#[tokio::test]
async fn exact_byte_budget_releases_on_dequeue_even_when_the_consumer_retains_the_event() {
    let event = text_event();
    let charge = event_bytes(&event);
    assert_eq!(charge, 1 + "execution".len() + size_of::<QueuedEvent>());
    let budget = budget(charge);
    let (sender, mut receiver) = budget.channel(4096);
    assert_eq!(sender.try_send(event), Ok(()));
    assert_eq!(budget.permits.available_permits(), 0);
    assert_eq!(sender.try_send(text_event()), Err(QueueError::Full));
    let retained_by_consumer = receiver.recv().await.unwrap();
    assert_eq!(budget.permits.available_permits(), charge);
    assert_eq!(sender.try_send(text_event()), Ok(()));
    assert!(matches!(
        retained_by_consumer.update(),
        ExecutionUpdate::Message(_)
    ));
}

#[tokio::test]
async fn failed_count_enqueue_and_receiver_drop_never_leak_byte_permits() {
    let charge = event_bytes(&text_event());
    let budget = budget(2 * charge);
    let (sender, receiver) = budget.channel(1);
    assert_eq!(sender.try_send(text_event()), Ok(()));
    assert_eq!(sender.try_send(text_event()), Err(QueueError::Full));
    assert_eq!(budget.permits.available_permits(), charge);
    drop(receiver);
    sender.closed().await;
    assert_eq!(budget.permits.available_permits(), 2 * charge);
    assert_eq!(sender.try_send(text_event()), Err(QueueError::Closed));
    assert_eq!(budget.permits.available_permits(), 2 * charge);
}

#[tokio::test]
async fn old_and_restored_generation_queues_share_one_budget() {
    let charge = event_bytes(&text_event());
    let budget = budget(2 * charge);
    let (old_sender, mut old_receiver) = budget.channel(4096);
    let (new_sender, new_receiver) = budget.channel(4096);
    assert_eq!(old_sender.try_send(text_event()), Ok(()));
    drop(old_sender);
    assert_eq!(new_sender.try_send(text_event()), Ok(()));
    assert_eq!(new_sender.try_send(text_event()), Err(QueueError::Full));
    assert!(old_receiver.recv().await.is_some());
    assert_eq!(new_sender.try_send(text_event()), Ok(()));
    drop(new_receiver);
    assert_eq!(budget.permits.available_permits(), 2 * charge);
}

#[test]
fn actual_payload_capacity_and_a_single_oversized_event_are_charged() {
    let event = text_event();
    let charge = event_bytes(&event);
    let budget = budget(charge - 1);
    let (sender, _receiver) = budget.channel(4096);
    assert_eq!(sender.try_send(event), Err(QueueError::Full));
    assert_eq!(budget.permits.available_permits(), charge - 1);

    let mut content = Vec::with_capacity(128);
    content.push(ToolContent::text(String::with_capacity(2048)));
    let tool = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        None,
        Some(content),
    );
    let expected = tool.payload_bytes();
    assert_eq!(expected, 128 * size_of::<ToolContent>());
    assert!(
        event_bytes(&ExecutionEvent::new(
            ExecutionId::new("execution").unwrap(),
            ExecutionUpdate::Tool(tool),
        )) >= expected + size_of::<QueuedEvent>()
    );
    let terminal = ExecutionEvent::new(
        ExecutionId::new("execution").unwrap(),
        ExecutionUpdate::Finished(ExecutionOutcome::Completed),
    );
    assert_eq!(
        event_bytes(&terminal),
        size_of::<QueuedEvent>() + "execution".len()
    );
}

#[test]
fn review_snapshots_charge_accumulated_observations_and_cancellation_evidence() {
    let mut controller = ExecutionController::new(ExecutionSessionId::new("session").unwrap());
    controller
        .begin_execution(ExecutionId::new("execution").unwrap())
        .unwrap();
    let tool_id = ToolCallId::new("tool").unwrap();
    controller
        .tool_event(
            &ExecutionId::new("execution").unwrap(),
            ToolCallUpdate::new(
                tool_id.clone(),
                Some("t".repeat(8192)),
                Some(ToolKind::Read),
                Some(ToolStatus::Pending),
                None,
                Some(vec![ToolContent::text("c".repeat(8192))]),
            ),
        )
        .unwrap();
    let options = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("deny").unwrap(),
            "Deny",
            PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap();
    let option_bytes = options.payload_bytes();
    let request = controller
        .request_permission(
            &ExecutionId::new("execution").unwrap(),
            PermissionId::new("review").unwrap(),
            ToolCallUpdate::new(tool_id, None, None, None, None, None),
            ToolReviewInput {
                name: "Read".into(),
                arguments_json: "{}".into(),
            },
            options,
        )
        .unwrap();
    // A tiny sparse request can capture a much larger accumulated observation.
    assert!(event_bytes(&request) >= 16 * 1024 + option_bytes + size_of::<QueuedEvent>());
    let reason = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::new("guard", "Withdraw this review").unwrap(),
    );
    let actor = ActionContext::new("principal", "surface", "action").unwrap();
    let cancellation = controller
        .cancel_permission(
            &ExecutionId::new("execution").unwrap(),
            &PermissionId::new("review").unwrap(),
            reason,
            CancellationOrigin::Client(actor),
        )
        .unwrap()
        .unwrap();
    let expected = size_of::<QueuedEvent>()
        + "execution".len()
        + "session".len()
        + size_of::<PermissionRequest>()
        + 2 * size_of::<usize>()
        + cancellation.request().payload_bytes()
        + cancellation.input().name.capacity()
        + cancellation.input().arguments_json.capacity()
        + "principal".len()
        + "surface".len()
        + "action".len();
    let cancellation = ExecutionEvent::new(
        ExecutionId::new("execution").unwrap(),
        ExecutionUpdate::PermissionCancelled(cancellation),
    );
    assert_eq!(event_bytes(&cancellation), expected);
    let records = controller
        .close(
            PermissionCancellationReason::session_closed(),
            CancellationOrigin::Runtime,
        )
        .unwrap();
    assert!(matches!(
        records.as_slice(),
        [ExecutionAuditRecord::SessionClosed(_)]
    ));
}
