//! Projections are bounded display state, not permission or scheduling authority.
use super::{
    projection::Projection, ConversationAgentFeatures, ConversationCapabilities,
    ConversationMessageStatus, ConversationPendingMode, PermissionDenialSupport,
};
use nessa_sdk::{
    application::agent_execution::{
        executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
        permissions::ActionContext,
        providers::{OperationCapabilities, ProviderIdentity},
        sessions::{InvocationRecord, SessionSnapshot},
        tools::ToolReviewInput,
    },
    domain::agent_execution::{
        executions::{ExecutionId, ExecutionOutcome, MessageChunk, MessageId},
        permissions::{
            PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy,
            PermissionOption, PermissionOptionId, PermissionOptions, PermissionScope,
        },
        prompts::{PromptText, UserMessage},
        sessions::{ExecutionSessionId, SessionId},
        tools::{ToolCallId, ToolCallUpdate, ToolContent, ToolObservation, ToolStatus},
    },
};
fn said(text: &str) -> UserMessage {
    UserMessage::text_only(PromptText::new(text).unwrap())
}
fn projection() -> Projection {
    Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
            agent_features: OperationCapabilities::default().into(),
        },
        None,
    )
}

#[test]
fn application_absences_project_as_not_implemented() {
    let value = serde_json::to_value(ConversationAgentFeatures::from(
        OperationCapabilities::default(),
    ))
    .unwrap();
    assert_eq!(value["permissionDenial"], "unknown");
    assert_eq!(value["nativeHookSuppression"], "unknown");
    assert_eq!(value["compactionReporting"], "unsupported_not_implemented");
    assert_eq!(value["modelSwitchReporting"], "unsupported_not_implemented");
    assert_eq!(value["permissionDeferral"], "unsupported_not_implemented");
    assert_eq!(value["preToolPolicy"], "unsupported_not_implemented");
    assert_eq!(value["policyEndTurn"], "unsupported_not_implemented");
    assert_eq!(value["policyCloseSession"], "unsupported_not_implemented");
    assert_eq!(value["incomingElicitation"], "unsupported_not_implemented");
}

#[test]
fn capability_changes_advance_the_replacement_revision_once() {
    let mut projection = projection();
    let before = projection.read();
    projection.capabilities(before.capabilities.clone());
    assert_eq!(projection.read().revision, before.revision);

    let mut changed = before.capabilities;
    changed.agent_features.permission_denial =
        PermissionDenialSupport::SupportedForOfferedPermissionReviews;
    projection.capabilities(changed.clone());
    let after = projection.read();
    assert_ne!(after.revision, before.revision);
    assert_eq!(after.capabilities, changed);

    projection.capabilities(changed);
    assert_eq!(projection.read().revision, after.revision);
}
fn event(update: ExecutionUpdate) -> ExecutionEvent {
    ExecutionEvent::new(ExecutionId::new("execution").unwrap(), update)
}
fn review(arguments: String) -> ExecutionEvent {
    event(ExecutionUpdate::PermissionRequested {
        id: PermissionId::new("permission").unwrap(),
        tool_id: ToolCallId::new("tool").unwrap(),
        observation: ToolObservation::default(),
        input: ToolReviewInput {
            name: "write_file".into(),
            arguments_json: arguments,
        },
        options: PermissionOptions::new(
            vec![PermissionOption::new(
                PermissionOptionId::new("allow").unwrap(),
                "Allow once",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap()],
            &PermissionOfferPolicy::once_only(),
        )
        .unwrap(),
    })
}
fn review_snapshot(events: Vec<ExecutionEvent>) -> SessionSnapshot {
    SessionSnapshot {
        id: SessionId::new("conversation").unwrap(),
        provider: ProviderIdentity::new("fixture", "model", "configuration").unwrap(),
        provider_session_id: ExecutionSessionId::new("provider-session").unwrap(),
        queue_history: vec![],
        invocations: vec![InvocationRecord {
            target_event_offset: None,
            submission: SubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: ExecutionId::new("execution").unwrap(),
                user_message: said("message"),
                estimated_input_tokens: 10,
                reserved_output_tokens: 10,
            },
            actor: ActionContext::new("person", "panel", "send").unwrap(),
            events,
            scheduling: vec![],
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        }],
    }
}
#[test]
fn encoded_view_is_bounded_even_when_json_escaping_expands_text() {
    let mut projection = projection();
    for i in 0..40 {
        let id = format!("execution-{i}");
        projection.admitted(
            &id,
            &said(&"\u{0001}".repeat(8192)),
            ConversationPendingMode::Queued,
        );
        projection.event(&ExecutionEvent::new(
            ExecutionId::new(id).unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("\u{0001}".repeat(8192))),
        ));
    }
    let view = projection.read();
    assert!(view.truncated);
    assert!(serde_json::to_vec(&view).unwrap().len() <= 60_000);
}
#[test]
fn terminal_projection_does_not_reappend_buffered_output_or_reset_on_admission() {
    let mut projection = projection();
    projection.event(&event(ExecutionUpdate::Message(MessageChunk::text(
        "complete",
    ))));
    projection.event(&event(ExecutionUpdate::Finished(
        ExecutionOutcome::Completed,
    )));
    projection.event(&event(ExecutionUpdate::Message(MessageChunk::text(
        "complete",
    ))));
    projection.admitted(
        "execution",
        &said("question"),
        ConversationPendingMode::Queued,
    );
    let view = projection.read();
    assert_eq!(
        view.messages[0]
            .parts
            .iter()
            .filter(|part| part.kind == "text")
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "complete"
    );
    assert_eq!(
        view.messages[0].status,
        ConversationMessageStatus::Completed
    );
    assert!(view.pending.is_empty());
}
#[test]
fn actionable_reviews_preserve_original_input_and_oversized_or_invalid_reviews_have_no_choices() {
    let original = r#"{"path":"target.txt","content":"exact\ncontent"}"#;
    let mut valid = projection();
    valid.event(&review(original.into()));
    let view = valid.read();
    assert_eq!(view.permissions[0].arguments_json, original);
    assert_eq!(view.permissions[0].options[0].id, "allow");
    for input in [
        "not json".into(),
        serde_json::to_string(&"x".repeat(33000)).unwrap(),
    ] {
        let mut rejected = projection();
        rejected.event(&review(input));
        let view = rejected.read();
        assert!(view.permissions.is_empty());
        assert!(view.permission_view_error.is_some());
    }
}
#[test]
fn observation_overflow_recovers_only_a_still_current_review_across_repeated_reads() {
    let mut projection = projection();
    let pending = review("{}".into());
    projection.event(&pending);
    projection.lagged();
    assert!(projection.read().permissions.is_empty());
    assert!(projection.read().permission_view_error.is_some());
    let snapshot = review_snapshot(vec![pending]);
    projection.recover_permissions(Some(&snapshot));
    assert_eq!(projection.read().permissions.len(), 1);
    let revision = projection.read().revision;
    projection.recover_permissions(Some(&snapshot));
    assert_eq!(projection.read().permissions.len(), 1);
    assert_eq!(projection.read().revision, revision);

    projection.resolved_permission("execution", "permission");
    projection.recover_permissions(Some(&snapshot));
    projection.recover_permissions(Some(&snapshot));
    assert!(projection.read().permissions.is_empty());
}

#[test]
fn uncertain_answer_fences_the_review_from_stale_snapshot_recovery() {
    let mut projection = projection();
    let pending = review("{}".into());
    projection.event(&pending);
    projection.lagged();
    projection.uncertain_permission("execution", "permission");
    projection.recover_permissions(Some(&review_snapshot(vec![pending])));
    let view = projection.read();
    assert!(view.permissions.is_empty());
    assert!(view.permission_view_error.is_some());
}

#[test]
fn lagged_terminal_evidence_never_revives_a_review() {
    let mut projection = projection();
    let pending = review("{}".into());
    projection.event(&pending);
    projection.lagged();
    projection.recover_permissions(Some(&review_snapshot(vec![
        pending,
        event(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)),
    ])));
    assert!(projection.read().permissions.is_empty());
}

#[test]
fn stale_preterminal_snapshot_cannot_revive_a_finished_review() {
    let mut projection = projection();
    let pending = review("{}".into());
    projection.event(&pending);
    projection.lagged();
    let stale = review_snapshot(vec![pending]);
    projection.event(&event(ExecutionUpdate::Finished(
        ExecutionOutcome::Cancelled,
    )));
    projection.recover_permissions(Some(&stale));
    projection.recover_permissions(Some(&stale));
    assert!(projection.read().permissions.is_empty());
}

fn text_parts(view: &super::ConversationView, id: &str) -> String {
    view.messages
        .iter()
        .find(|message| message.execution_id == id)
        .unwrap()
        .parts
        .iter()
        .filter(|part| part.kind == "text")
        .map(|part| part.text.as_str())
        .collect()
}
fn completed_snapshot(id: &str, events: Vec<ExecutionEvent>) -> SessionSnapshot {
    let mut snapshot = review_snapshot(events);
    snapshot.invocations[0].request.execution_id = ExecutionId::new(id).unwrap();
    snapshot.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    snapshot
}

#[test]
fn lagged_projection_rebuilds_saved_text_from_a_terminal_snapshot_exactly_once() {
    let mut projection = projection();
    let chunk = event(ExecutionUpdate::Message(MessageChunk::text("saved answer")));
    projection.event(&chunk);
    projection.lagged();
    // Ambiguous live text is fenced: it has no durable cursor.
    assert!(text_parts(&projection.read(), "execution").is_empty());

    let snapshot = completed_snapshot(
        "execution",
        vec![
            chunk.clone(),
            event(ExecutionUpdate::Finished(ExecutionOutcome::Completed)),
        ],
    );
    projection.settled("execution", Some(&snapshot));
    let view = projection.read();
    assert_eq!(text_parts(&view, "execution"), "saved answer");
    assert_eq!(
        view.messages[0].status,
        ConversationMessageStatus::Completed
    );

    // Buffered live chunks draining after settlement must not duplicate it.
    projection.event(&chunk);
    assert_eq!(text_parts(&projection.read(), "execution"), "saved answer");

    // The live fence still holds for a later invocation in the same projection,
    // and its own terminal snapshot recovers its text.
    let later = ExecutionId::new("later").unwrap();
    let chunk = ExecutionEvent::new(
        later.clone(),
        ExecutionUpdate::Message(MessageChunk::text("later answer")),
    );
    projection.admitted("later", &said("question"), ConversationPendingMode::Queued);
    projection.event(&chunk);
    assert!(text_parts(&projection.read(), "later").is_empty());
    projection.settled(
        "later",
        Some(&completed_snapshot(
            "later",
            vec![
                chunk,
                ExecutionEvent::new(
                    later,
                    ExecutionUpdate::Finished(ExecutionOutcome::Completed),
                ),
            ],
        )),
    );
    assert_eq!(text_parts(&projection.read(), "later"), "later answer");
}

#[test]
fn cancellation_does_not_hide_a_separate_receipt_failure() {
    let mut projection = projection();
    projection.event(&event(ExecutionUpdate::Finished(
        ExecutionOutcome::Cancelled,
    )));
    projection.receipt_failed("execution");
    let view = projection.read();
    assert_eq!(
        view.messages[0].status,
        ConversationMessageStatus::Cancelled
    );
    assert!(view.messages[0].error.is_some());
}

#[test]
fn transcript_truncation_does_not_hide_complete_queue_but_queue_trimming_does() {
    let mut projection = projection();
    projection.view.truncated = true;
    projection.queue_order(&[]);
    assert!(projection.read().queue_complete);
    let mut order = Vec::new();
    for index in 0..64 {
        let id = format!("waiting-{index}");
        projection.admitted(
            &id,
            &said(&"x".repeat(1024)),
            ConversationPendingMode::Queued,
        );
        order.push(ExecutionId::new(id).unwrap());
    }
    projection.queue_order(&order);
    let view = projection.read();
    assert!(view.pending.len() < order.len());
    assert!(!view.queue_complete);
}

#[test]
fn pending_and_message_text_agree_through_the_eight_kibibyte_contract() {
    let mut projection = projection();
    let text = "😀".repeat(2048);

    projection.admitted("execution", &said(&text), ConversationPendingMode::Queued);

    let view = projection.read();
    assert_eq!(text.len(), 8192);
    assert_eq!(view.messages[0].user_text, text);
    assert_eq!(view.pending[0].text, view.messages[0].user_text);
    assert!(view.queue_complete);
    assert!(!view.truncated);
}

#[test]
fn tool_details_preserve_whitespace_sparse_updates_and_explicit_clear() {
    let mut projection = projection();
    let tool = ToolCallId::new("shell").unwrap();
    projection.event(&event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        tool.clone(),
        Some("Shell".into()),
        None,
        Some(ToolStatus::Running),
        None,
        Some(vec![ToolContent::text(
            "  output\n<script>literal</script>",
        )]),
    ))));
    projection.event(&event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        tool.clone(),
        None,
        None,
        Some(ToolStatus::Completed),
        None,
        None,
    ))));
    assert_eq!(
        projection.read().tools[0].details,
        "  output\n<script>literal</script>"
    );
    projection.event(&event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        tool,
        None,
        None,
        None,
        None,
        Some(vec![]),
    ))));
    assert_eq!(projection.read().tools[0].details, "");
}

#[test]
fn steering_during_review_keeps_permission_and_shared_response_target() {
    let mut projection = projection();
    projection.admitted(
        "execution",
        &said("Inspect desktop"),
        ConversationPendingMode::Queued,
    );
    projection.event(&review("{}".into()));
    for id in ["steer-one", "steer-two"] {
        projection.admitted(id, &said("Hello"), ConversationPendingMode::Steering);
        projection.injected(id, "execution");
    }
    let pending = projection.read();
    assert_eq!(pending.permissions.len(), 1);
    assert!(pending.messages[1..]
        .iter()
        .all(|message| message.steering_target.as_deref() == Some("execution")));
    projection.event(&event(ExecutionUpdate::Message(MessageChunk::text(
        "Answer including steering",
    ))));
    let view = projection.read();
    assert_eq!(
        view.messages[0]
            .parts
            .iter()
            .filter(|part| part.kind == "text")
            .map(|part| part.text.as_str())
            .collect::<String>(),
        "Answer including steering"
    );
    assert_eq!(view.messages[1].status, ConversationMessageStatus::Injected);
}

#[test]
fn ordered_parts_keep_message_identity_and_non_text_observation_offsets() {
    let mut projection = projection();
    projection.event(&event(ExecutionUpdate::Message(
        MessageChunk::text(" First ").with_message_id(MessageId::new("m1").unwrap()),
    )));
    projection.event(&review("{}".into()));
    projection.event(&event(ExecutionUpdate::Message(
        MessageChunk::text("reply.").with_message_id(MessageId::new("m1").unwrap()),
    )));
    projection.event(&event(ExecutionUpdate::Message(
        MessageChunk::text("Second reply.").with_message_id(MessageId::new("m2").unwrap()),
    )));
    let value = serde_json::to_value(projection.read()).unwrap();
    let message = &value["messages"][0];
    assert!(message.get("assistantText").is_none());
    assert_eq!(message["parts"][0]["text"], " First ");
    assert_eq!(message["parts"][1]["offset"], 2);
    assert_eq!(message["parts"][1]["messageId"], "m1");
    assert_eq!(message["parts"][2]["messageId"], "m2");
}
