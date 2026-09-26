//! Projections are bounded display state, not permission or scheduling authority.
use super::{
    projection::Projection, ConversationCapabilities, ConversationMessageStatus,
    ConversationPendingMode,
};
use nessa_sdk::{
    application::agent_execution::{
        agents::AgentError,
        executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode},
        permissions::ActionContext,
        providers::ProviderIdentity,
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
        questions::{AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId},
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
        },
        None,
    )
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

fn asked(execution: &str, question: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        ExecutionId::new(execution).unwrap(),
        ExecutionUpdate::QuestionAsked {
            id: QuestionId::new(question).unwrap(),
            question: AgentQuestion::new(
                "Which environment?",
                vec![Question::new(
                    "question_0",
                    "Which environment?",
                    None,
                    AnswerShape::One,
                    vec![AnswerOption::new("staging", "Staging", None).unwrap()],
                    None,
                    false,
                )
                .unwrap()],
            )
            .unwrap(),
        },
    )
}
fn closed(execution: &str, question: &str) -> ExecutionEvent {
    ExecutionEvent::new(
        ExecutionId::new(execution).unwrap(),
        ExecutionUpdate::QuestionClosed {
            id: QuestionId::new(question).unwrap(),
        },
    )
}

#[test]
fn a_closed_ask_stays_closed_when_it_is_replayed() {
    // The review reproduced a closure recorded as (question, question) instead
    // of (execution, question): a replayed ask missed its tombstone and became
    // answerable again. The execution and question ids differ here, which is
    // exactly the case that exposed it.
    let mut projection = projection();
    projection.event(&asked("execution", "1"));
    assert_eq!(projection.read().questions.len(), 1);
    projection.event(&closed("execution", "1"));
    assert!(projection.read().questions.is_empty());
    projection.event(&asked("execution", "1"));
    assert!(
        projection.read().questions.is_empty(),
        "a closed ask reopened on replay"
    );
}

#[test]
fn closing_one_executions_ask_leaves_anothers_with_the_same_id_open() {
    // Removal matched on the question id alone, so closing one execution's ask
    // took another execution's same-numbered ask with it.
    let mut projection = projection();
    projection.event(&asked("first", "1"));
    projection.event(&asked("second", "1"));
    projection.event(&closed("first", "1"));
    let view = projection.read();
    assert_eq!(view.questions.len(), 1);
    assert_eq!(view.questions[0].execution_id, "second");
}

/// The view agrees with what the client checks before it will show it.
///
/// Every ask and review belongs to a message that is running, and everything
/// pending to one that is queued. The client refuses the whole view otherwise,
/// so the projection has to agree with it rather than hope the two never meet.
/// Returns how many asks are offered.
fn offers_only_running_asks(projection: &mut Projection) -> usize {
    let view = projection.read();
    let status = |execution: &str| {
        view.messages
            .iter()
            .find(|message| message.execution_id == execution)
            .map(|message| message.status)
    };
    for execution in view
        .questions
        .iter()
        .map(|question| &question.execution_id)
        .chain(
            view.permissions
                .iter()
                .map(|permission| &permission.execution_id),
        )
    {
        assert_eq!(status(execution), Some(ConversationMessageStatus::Running));
    }
    for item in &view.pending {
        assert_eq!(
            status(&item.execution_id),
            Some(ConversationMessageStatus::Queued)
        );
    }
    view.questions.len()
}

#[test]
fn an_ask_whose_closure_never_reached_storage_is_not_offered_after_restart() {
    // The gateway stopped with an ask open, so the ask was saved and its
    // closure was not. Restored, the message is unresolved; offering the ask
    // beside it made the client refuse the view on every restart.
    let snapshot = review_snapshot(vec![asked("execution", "1")]);
    let mut restored = Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
        },
        Some(&snapshot),
    );
    assert_eq!(offers_only_running_asks(&mut restored), 0);
}

#[test]
fn an_ask_left_open_by_a_failed_execution_is_not_offered_once_it_settles() {
    let mut projection = projection();
    projection.event(&asked("execution", "1"));
    assert_eq!(offers_only_running_asks(&mut projection), 1);
    let mut snapshot = review_snapshot(vec![asked("execution", "1")]);
    snapshot.invocations[0].result = Some(Err(AgentError::Closed));
    projection.settled("execution", Some(&snapshot));
    assert_eq!(offers_only_running_asks(&mut projection), 0);
}

#[test]
fn an_ask_is_not_offered_beside_a_receipt_that_failed() {
    // The receipt failed and storage held no result for the execution, so its
    // message is marked failed without a record passing through: the path the
    // settle-time rule never saw. A closure arriving after it is ignored.
    let mut projection = projection();
    projection.event(&asked("execution", "1"));
    projection.settled(
        "execution",
        Some(&review_snapshot(vec![asked("execution", "1")])),
    );
    projection.receipt_failed("execution");
    assert_eq!(offers_only_running_asks(&mut projection), 0);
    projection.event(&closed("execution", "1"));
    assert_eq!(offers_only_running_asks(&mut projection), 0);
}

#[test]
fn an_ask_recovered_after_lag_is_offered_by_a_running_message() {
    // Lag dropped the dispatch and the ask. Recovered from storage, the ask
    // proves the execution was dispatched and is waiting; hiding it would leave
    // the agent waiting on a question nobody can see, and showing it beside a
    // queued message made the client refuse the view.
    let mut projection = projection();
    projection.admitted(
        "execution",
        &said("message"),
        ConversationPendingMode::Queued,
    );
    projection.lagged();
    projection.recover_permissions(Some(&review_snapshot(vec![asked("execution", "1")])));
    assert_eq!(offers_only_running_asks(&mut projection), 1);
    projection.queue_order(&[]);
    assert_eq!(offers_only_running_asks(&mut projection), 1);
}

/// Saved turns in order, each settled as completed or left as the gateway
/// stopped it.
fn saved_turns(turns: Vec<(&str, Vec<ExecutionEvent>, bool)>) -> SessionSnapshot {
    let mut snapshot = review_snapshot(vec![]);
    let template = snapshot.invocations.remove(0);
    for (id, events, settled) in turns {
        let mut record = template.clone();
        record.request.execution_id = ExecutionId::new(id).unwrap();
        record.events = events;
        if settled {
            record.result = Some(Ok(ExecutionOutcome::Completed));
        }
        snapshot.invocations.push(record);
    }
    snapshot
}
fn restored(snapshot: &SessionSnapshot) -> Projection {
    Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
        },
        Some(snapshot),
    )
}

#[test]
fn recovery_does_not_bring_back_a_turn_from_before_a_restart() {
    // An ask left open when the gateway stopped, from a turn now older than
    // the view shows. Recovery recreated its message as running, offered an ask
    // nothing in this process could answer, and pushed a real message out.
    let names: Vec<String> = (0..24).map(|index| format!("done-{index}")).collect();
    let mut turns = vec![("stale", vec![asked("stale", "1")], false)];
    turns.extend(names.iter().map(|name| (name.as_str(), vec![], true)));
    let snapshot = saved_turns(turns);
    let mut projection = restored(&snapshot);
    let before: Vec<_> = projection
        .read()
        .messages
        .iter()
        .map(|message| message.execution_id.clone())
        .collect();
    projection.lagged();
    projection.recover_permissions(Some(&snapshot));
    let view = projection.read();
    assert!(view.questions.is_empty());
    assert_eq!(
        view.messages
            .iter()
            .map(|message| message.execution_id.clone())
            .collect::<Vec<_>>(),
        before,
        "no message recreated, none pushed out"
    );
}

#[test]
fn asks_nobody_can_answer_do_not_crowd_out_one_somebody_can() {
    // Eight unanswerable asks — from turns before a restart, or turns whose
    // receipts failed — held every open slot while hidden from view, so a live
    // ask was dropped and its agent waited with nothing on screen.
    let names: Vec<String> = (0..8).map(|index| format!("stale-{index}")).collect();
    let snapshot = saved_turns(
        names
            .iter()
            .map(|name| (name.as_str(), vec![asked(name, "1")], false))
            .collect(),
    );
    let mut after_restart = restored(&snapshot);
    after_restart.lagged();
    after_restart.recover_permissions(Some(&snapshot));
    after_restart.admitted("live", &said("go"), ConversationPendingMode::Queued);
    after_restart.event(&asked("live", "1"));
    assert_eq!(offers_only_running_asks(&mut after_restart), 1);

    let mut failed_receipts = projection();
    for name in &names {
        failed_receipts.event(&asked(name, "1"));
        failed_receipts.receipt_failed(name);
    }
    failed_receipts.event(&asked("live", "1"));
    assert_eq!(offers_only_running_asks(&mut failed_receipts), 1);
}

/// Twenty-four newer turns push a running turn's message out of the view. Its
/// review or ask is still waiting, and a truncated view still offers it.
fn evict_running_execution(projection: &mut Projection) {
    for index in 0..24 {
        projection.admitted(
            &format!("queued-{index}"),
            &said("later"),
            ConversationPendingMode::Queued,
        );
    }
    let view = projection.read();
    assert!(view
        .messages
        .iter()
        .all(|message| message.execution_id != "execution"));
    assert!(view.truncated);
}

#[test]
fn a_review_whose_message_was_pushed_out_survives_lag_recovery() {
    // Absent is not the same as before a restart: this turn began here and is
    // running. Recovery took its absence as death and dropped the review, and
    // with another turn's review recovered beside it, dropped it silently.
    let pending = review("{}".into());
    let mut alone = projection();
    alone.event(&pending);
    evict_running_execution(&mut alone);
    assert_eq!(alone.read().permissions.len(), 1);
    alone.lagged();
    alone.recover_permissions(Some(&review_snapshot(vec![pending.clone()])));
    let view = alone.read();
    assert_eq!(
        view.permissions.len(),
        1,
        "{:?}",
        view.permission_view_error
    );
    assert_eq!(view.permissions[0].execution_id, "execution");

    let mut beside = projection();
    beside.event(&pending);
    for index in 0..23 {
        beside.admitted(
            &format!("queued-{index}"),
            &said("later"),
            ConversationPendingMode::Queued,
        );
    }
    let other = ExecutionEvent::new(
        ExecutionId::new("queued-22").unwrap(),
        pending.update().clone(),
    );
    beside.event(&other);
    beside.admitted("queued-23", &said("later"), ConversationPendingMode::Queued);
    assert_eq!(beside.read().permissions.len(), 2);
    beside.lagged();
    let mut snapshot = review_snapshot(vec![pending]);
    let mut second = snapshot.invocations[0].clone();
    second.request.execution_id = ExecutionId::new("queued-22").unwrap();
    second.events = vec![other];
    snapshot.invocations.push(second);
    beside.recover_permissions(Some(&snapshot));
    assert_eq!(beside.read().permissions.len(), 2);
}

#[test]
fn an_ask_lag_dropped_from_a_turn_whose_message_was_pushed_out_is_recovered() {
    let mut projection = projection();
    projection.event(&event(ExecutionUpdate::Message(MessageChunk::text(
        "working",
    ))));
    evict_running_execution(&mut projection);
    // The ask is the event the lag dropped.
    projection.lagged();
    projection.recover_permissions(Some(&review_snapshot(vec![asked("execution", "1")])));
    let view = projection.read();
    assert_eq!(view.questions.len(), 1);
    assert_eq!(view.questions[0].execution_id, "execution");
}
