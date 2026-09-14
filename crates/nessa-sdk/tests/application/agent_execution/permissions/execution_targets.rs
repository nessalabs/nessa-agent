//! Delayed callbacks cannot mutate or label a later execution with reused IDs.
use super::*;

#[derive(Clone, Copy, Debug)]
enum Callback {
    Tool,
    Review,
    CancelReview,
    Message,
    TerminalProjection,
    Finish,
    Close,
}

fn update(title: Option<&str>) -> ToolCallUpdate {
    ToolCallUpdate::new(
        ToolCallId::new("reused-tool").unwrap(),
        title.map(str::to_owned),
        None,
        None,
        None,
        None,
    )
}

#[test]
fn stale_callbacks_preserve_current_tools_reviews_and_execution_evidence() {
    for callback in [
        Callback::Tool,
        Callback::Review,
        Callback::CancelReview,
        Callback::Message,
        Callback::TerminalProjection,
        Callback::Finish,
        Callback::Close,
    ] {
        let first = ExecutionId::new("first").unwrap();
        let second = ExecutionId::new("second").unwrap();
        let review = PermissionId::new("reused-review").unwrap();
        let options = pending_permission().options().clone();
        let mut controller = ExecutionController::new(ExecutionSessionId::new("context").unwrap());
        controller.begin_execution(first.clone()).unwrap();
        controller
            .request_permission(
                &first,
                review.clone(),
                update(Some("first tool")),
                review_input(),
                options.clone(),
            )
            .unwrap();
        controller
            .finish_execution(&first, Ok(ExecutionOutcome::Completed))
            .unwrap();
        controller.begin_execution(second.clone()).unwrap();
        let original_input = ToolReviewInput {
            name: "second input".into(),
            arguments_json: "{\"second\":true}".into(),
        };
        controller
            .request_permission(
                &second,
                review.clone(),
                update(Some("second tool")),
                original_input.clone(),
                options.clone(),
            )
            .unwrap();
        let rejected = match callback {
            Callback::Tool => controller
                .tool_event(&first, update(Some("stale replacement")))
                .map(|_| ()),
            Callback::Review => controller
                .request_permission(
                    &first,
                    PermissionId::new("stale-new").unwrap(),
                    update(Some("stale request")),
                    review_input(),
                    options.clone(),
                )
                .map(|_| ()),
            Callback::CancelReview => controller
                .cancel_permission(
                    &first,
                    &review,
                    PermissionCancellationReason::provider_withdrawal(),
                    CancellationOrigin::Provider,
                )
                .map(|_| ()),
            Callback::Message => controller
                .message_event(&first, MessageChunk::text("stale message"))
                .map(|_| ()),
            Callback::TerminalProjection => controller
                .finished_event(&first, ExecutionOutcome::Refused)
                .map(|_| ()),
            Callback::Finish => controller
                .finish_execution(
                    &first,
                    Err(PermissionCancellationReason::execution_failed()),
                )
                .map(|_| ()),
            Callback::Close => controller
                .close_execution(
                    &first,
                    PermissionCancellationReason::execution_failed(),
                    CancellationOrigin::Runtime,
                )
                .map(|_| ()),
        };
        match callback {
            Callback::CancelReview | Callback::Finish | Callback::Close => {
                assert!(
                    matches!(rejected, Err(AgentError::Protocol(_))),
                    "{callback:?}: {rejected:?}"
                );
            }
            _ => {
                assert!(
                    matches!(rejected, Err(AgentError::InvalidInput(_))),
                    "{callback:?}: {rejected:?}"
                );
            }
        }
        assert_eq!(controller.active_execution_id(), Some(&second));
        assert_eq!(
            controller
                .message_event(&second, MessageChunk::text("current"))
                .unwrap()
                .execution_id(),
            &second
        );
        // A sparse request reads the retained tool snapshot. It must still hold
        // B's original title; the stale update/request cannot replace it.
        let event = controller
            .request_permission(
                &second,
                PermissionId::new("second-review").unwrap(),
                update(None),
                review_input(),
                options,
            )
            .unwrap();
        let ExecutionUpdate::PermissionRequested { observation, .. } = event.update() else {
            panic!("missing review event")
        };
        assert_eq!(observation.title().as_deref(), Some("second tool"));
        let cancellation = controller
            .cancel_permission(
                &second,
                &review,
                PermissionCancellationReason::provider_withdrawal(),
                CancellationOrigin::Provider,
            )
            .unwrap()
            .unwrap();
        assert_eq!(cancellation.request().execution_id(), &second);
        assert_eq!(cancellation.request().id(), &review);
        assert_eq!(cancellation.input(), &original_input);
        let records = controller
            .finish_execution(&second, Ok(ExecutionOutcome::Completed))
            .unwrap();
        assert_eq!(records.len(), 2);
        let ExecutionAuditRecord::Cancelled(remaining) = &records[1] else {
            panic!("missing remaining review")
        };
        assert_eq!(remaining.request().execution_id(), &second);
        assert_eq!(remaining.request().id().as_str(), "second-review");
        assert_eq!(controller.active_execution_id(), None);
    }
}
