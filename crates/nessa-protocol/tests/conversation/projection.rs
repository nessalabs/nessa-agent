//! Projections are bounded display state, not permission or scheduling authority.
//! The cases that drive `ConversationService` stay in the gateway's
//! `tests/conversation/projection.rs`.
use crate::conversation::domain::ConversationId;
use crate::conversation::projection::{bound_view, clipped, Projection, MAX_TEXT, MAX_VIEW_BYTES};
use crate::conversation::view::{ConversationPermissionOptionEffect, ConversationTranscriptState};
use crate::conversation::{
    projection::retained_view,
    view::{
        ConversationAgentFeatures, ConversationAttachmentEvidenceFailure,
        ConversationAttachmentEvidenceFailureCode, ConversationCapabilities, ConversationLifecycle,
        ConversationLifecyclePhase, ConversationMessageStatus, ConversationView,
        PermissionDenialSupport, MAX_STRUCTURED_CONTENT_BYTES,
    },
};
use nessa_sdk::application::agent_execution::agents::{AgentError, ProviderDiagnostic};
use nessa_sdk::application::agent_execution::executions::{
    ExecutionController, ExecutionEvent, ExecutionRequest, ExecutionUpdate,
    SubmissionMode as InvocationSubmissionMode,
};
use nessa_sdk::application::agent_execution::permissions::{ActionContext, CancellationOrigin};
use nessa_sdk::application::agent_execution::providers::{
    ExecutionReport, OperationCapabilities, ProviderIdentity, ProviderSessionState,
};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedCompleteness, CommittedFreshness, CommittedSession, CommittedStatus, InvocationRecord,
    InvocationSchedulingEvent, QueueHistoryRecord, SessionSnapshot, SubmissionAcknowledgement,
};
use nessa_sdk::application::agent_execution::tools::ToolReviewInput;
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, InvocationKind, InvocationStage, MessageChunk, MessageId,
    QueueMutation, SchedulingCause,
};
use nessa_sdk::domain::agent_execution::permissions::{
    PermissionApplicationId, PermissionCancellationReason, PermissionDecision, PermissionEffect,
    PermissionId, PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionScope, PermissionSessionId,
};
use nessa_sdk::domain::agent_execution::prompts::{PromptText, UserMessage};
use nessa_sdk::domain::agent_execution::questions::{
    AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId, MAX_OPEN_ASK_COST,
};
use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, SessionId,
};
use nessa_sdk::domain::agent_execution::tools::{
    McpTool, ToolCallId, ToolCallUpdate, ToolContent, ToolObservation, ToolStatus,
};
use uuid::Uuid;

fn said(text: &str) -> UserMessage {
    UserMessage::text_only(PromptText::new(text).unwrap())
}
pub(super) fn projection() -> Projection {
    projection_for("conversation")
}

fn projection_for(conversation: &str) -> Projection {
    Projection::new(
        conversation.into(),
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

fn committed(
    incarnation: &str,
    applied: u64,
    downloaded: u64,
    head: u64,
    snapshot: Option<&SessionSnapshot>,
) -> CommittedSession {
    let completeness = if applied < downloaded {
        CommittedCompleteness::Partial
    } else if snapshot.is_some() {
        CommittedCompleteness::Complete
    } else {
        CommittedCompleteness::CompleteEmpty
    };
    let freshness = if downloaded < head {
        CommittedFreshness::Stale
    } else {
        CommittedFreshness::Current
    };
    CommittedSession::new(
        SessionId::new("conversation").unwrap(),
        incarnation.into(),
        applied,
        downloaded,
        head,
        snapshot.cloned(),
        CommittedStatus::new(completeness, freshness),
    )
    .unwrap()
}

#[test]
fn incomplete_committed_view_suppresses_controls_even_after_capability_refresh() {
    let mut projection = projection();
    projection.replace_committed(&committed("incarnation", 1, 1, 1, None), &[], None);
    projection.transcript_state(ConversationTranscriptState::Stale);
    let available = ConversationCapabilities {
        queue: true,
        steer: true,
        resume: true,
        permissions: true,
        image_input: false,
        agent_features: OperationCapabilities::default().into(),
    };
    projection.capabilities(available.clone());
    let stale = projection.read();
    assert!(!stale.capabilities.queue);
    assert!(!stale.capabilities.steer);
    assert!(!stale.capabilities.permissions);
    assert!(!stale.queue_complete);

    projection.transcript_state(ConversationTranscriptState::CompleteEmpty);
    projection.capabilities(available);
    let complete = projection.read();
    assert!(complete.capabilities.queue);
    assert!(complete.capabilities.steer);
    assert!(complete.capabilities.permissions);
}

#[test]
fn older_committed_read_cannot_replace_a_newer_terminal_view() {
    let mut projection = projection();
    let newer = completed_snapshot(
        "execution",
        vec![event(ExecutionUpdate::Finished(
            ExecutionOutcome::Completed,
        ))],
    );
    projection.replace_committed(&committed("incarnation", 2, 2, 2, Some(&newer)), &[], None);
    let current = projection.read();
    assert_eq!(
        current.messages[0].status,
        ConversationMessageStatus::Completed
    );
    projection.replace_committed(&committed("incarnation", 1, 1, 1, None), &[], None);
    let after = projection.read();
    assert_eq!(after.revision, current.revision);
    assert_eq!(
        after.messages[0].status,
        ConversationMessageStatus::Completed
    );
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
    assert_eq!(value["incomingElicitation"], "unknown");
}

#[test]
fn capability_changes_advance_the_replacement_revision_once() {
    let mut projection = projection();
    projection.transcript_state(ConversationTranscriptState::CompleteEmpty);
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
pub(super) fn event(update: ExecutionUpdate) -> ExecutionEvent {
    ExecutionEvent::new(ExecutionId::new("execution").unwrap(), update)
}

#[test]
fn lifecycle_evidence_is_phase_independent_and_only_changes_revision_once() {
    let mut projection = projection();
    let initial = projection.read().revision;
    let lifecycle = ConversationLifecycle {
        phase: ConversationLifecyclePhase::Attached,
        failure: None,
        evidence_failure: Some(ConversationAttachmentEvidenceFailure {
            code: ConversationAttachmentEvidenceFailureCode::Audit,
            message: "attachment audit was not acknowledged".into(),
        }),
    };
    projection.lifecycle(lifecycle.clone());
    let changed = projection.read();
    assert_ne!(changed.revision, initial);
    assert_eq!(changed.lifecycle, lifecycle);

    projection.lifecycle(lifecycle);
    assert_eq!(projection.read().revision, changed.revision);
}

#[test]
fn lifecycle_diagnostics_are_clipped_at_a_utf8_boundary() {
    let exact = "😀".repeat(512);
    assert_eq!(clipped(&exact, 2048), exact);
    let oversized = "😀".repeat(513);
    let clipped = clipped(&oversized, 2048);
    assert_eq!(clipped, "😀".repeat(512));
    assert_eq!(clipped.len(), 2048);
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
        provider_context: ProviderContext::Recorded(
            ExecutionSessionId::new("provider-session").unwrap(),
        ),
        queue_history: vec![],
        invocations: vec![InvocationRecord {
            target_event_offset: None,
            submission: InvocationSubmissionMode::Immediate,
            request: ExecutionRequest {
                execution_id: ExecutionId::new("execution").unwrap(),
                user_message: said("message"),
                estimated_input_tokens: 10,
                reserved_output_tokens: 10,
            },
            actor: ActionContext::new("person", "panel", "send").unwrap(),
            acknowledgement: SubmissionAcknowledgement::Acknowledged,
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

pub(super) fn completed_snapshot(id: &str, events: Vec<ExecutionEvent>) -> SessionSnapshot {
    let mut snapshot = review_snapshot(events);
    snapshot.invocations[0].request.execution_id = ExecutionId::new(id).unwrap();
    snapshot.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    snapshot
}

#[test]
fn provider_diagnostic_and_independent_failure_are_both_visible() {
    let provider_error = AgentError::Provider {
        code: -32603,
        diagnostic: Some(ProviderDiagnostic::new("provider refused the prompt")),
    };
    let report = ExecutionReport::new(
        Some(Err(provider_error)),
        Some(AgentError::AuditFailure),
        ProviderSessionState::Usable,
    );
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations[0].result = Some(report.clone().into_result());
    snapshot.invocations[0].provider_report = Some(report);
    let restored = Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
            agent_features: OperationCapabilities::default().into(),
        },
        Some(&snapshot),
    );
    assert_eq!(
        restored.read().messages[0].error.as_deref(),
        Some(
            "The agent provider reported an error: provider refused the prompt. The turn could not complete all required work."
        )
    );
}

#[test]
fn successful_provider_result_with_later_failure_does_not_claim_provider_refusal() {
    let report = ExecutionReport::new(
        Some(Ok(ExecutionOutcome::Completed)),
        Some(AgentError::AuditFailure),
        ProviderSessionState::Usable,
    );
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations[0].result = Some(report.clone().into_result());
    snapshot.invocations[0].provider_report = Some(report);
    let restored = Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
            agent_features: OperationCapabilities::default().into(),
        },
        Some(&snapshot),
    );
    let error = restored.read().messages[0].error.clone().unwrap();
    assert_eq!(error, "The turn could not complete all required work.");
    assert!(!error.contains("provider refused"));
}

pub(super) fn committed_tool_view(events: &[ExecutionEvent]) -> ConversationView {
    let snapshot = completed_snapshot("execution", events.to_vec());
    let capabilities = projection().read().capabilities;
    bound_view(Projection::new("conversation".into(), capabilities, Some(&snapshot)).read())
}

#[test]
fn an_mcp_tool_carries_its_identity_and_structured_result_beside_its_text() {
    let mut events = Vec::new();
    let tool = ToolCallId::new("chart").unwrap();
    let json = r#"{"rows":[1,2]}"#;
    events.push(event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            tool.clone(),
            Some("mcp.charts.show".into()),
            None,
            None,
            None,
            None,
        )
        .with_mcp_tool(McpTool::new("charts", "show").unwrap()),
    )));
    // A later update naming nothing keeps the identity.
    events.push(event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            tool.clone(),
            None,
            None,
            Some(ToolStatus::Completed),
            None,
            None,
        )
        .with_content(vec![
            ToolContent::text("Two rows."),
            ToolContent::structured(json).unwrap(),
        ]),
    )));
    let view = committed_tool_view(&events);
    let mcp = view.tools[0].mcp.as_ref().unwrap();
    assert_eq!((mcp.server.as_str(), mcp.tool.as_str()), ("charts", "show"));
    assert_eq!(view.tools[0].structured_content.as_deref(), Some(json));
    assert_eq!(view.tools[0].details, "Two rows.");
    let wire = serde_json::to_value(&view.tools[0]).unwrap();
    assert_eq!(
        wire["mcp"],
        serde_json::json!({"server":"charts","tool":"show"})
    );
    assert_eq!(wire["structuredContent"], json);

    // Past the view's bound it is said, not cut; content replaced without one
    // no longer has one.
    let large = format!("\"{}\"", "a".repeat(MAX_STRUCTURED_CONTENT_BYTES));
    events.push(event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(tool.clone(), None, None, None, None, None)
            .with_content(vec![ToolContent::structured(large).unwrap()]),
    )));
    let view = committed_tool_view(&events);
    // Left out, not cut; the details are what the text said, here nothing.
    assert_eq!(view.tools[0].structured_content, None);
    assert_eq!(view.tools[0].details, "");
    events.push(event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        tool,
        None,
        None,
        None,
        None,
        Some(vec![ToolContent::structured(json).unwrap()]),
    ))));
    events.push(event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new("chart").unwrap(),
        None,
        None,
        None,
        None,
        Some(vec![ToolContent::text("plain")]),
    ))));
    assert_eq!(
        committed_tool_view(&events).tools[0].structured_content,
        None
    );
}

#[test]
fn a_structured_result_after_long_text_is_kept_and_the_last_one_reported_wins() {
    let mut events = Vec::new();
    let json = r#"{"rows":2}"#;
    events.push(event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new("chart").unwrap(),
        None,
        None,
        None,
        None,
        Some(vec![
            ToolContent::text("a".repeat(12_000)),
            ToolContent::text("b".repeat(12_000)),
            ToolContent::structured(format!("\"{}\"", "c".repeat(MAX_STRUCTURED_CONTENT_BYTES)))
                .unwrap(),
            ToolContent::structured(json).unwrap(),
        ]),
    ))));
    let view = committed_tool_view(&events);
    let tool = &view.tools[0];
    assert_eq!(tool.structured_content.as_deref(), Some(json));
    assert!(tool.details.ends_with("[Output truncated]"));
}

#[test]
fn a_view_past_its_budget_gives_up_structured_results_before_any_message() {
    let mut events = Vec::new();
    events.push(event(ExecutionUpdate::Message(MessageChunk::text(
        "Charted.",
    ))));
    let json = format!("\"{}\"", "s".repeat(MAX_STRUCTURED_CONTENT_BYTES - 2));
    for index in 0..3 {
        events.push(event(ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new(format!("chart-{index}")).unwrap(),
            None,
            None,
            None,
            None,
            Some(vec![
                ToolContent::text("d".repeat(12_000)),
                ToolContent::structured(json.clone()).unwrap(),
            ]),
        ))));
    }
    let view = committed_tool_view(&events);
    assert!(serde_json::to_vec(&view).unwrap().len() <= 60_000);
    // Nothing of the history was left out, so the view does not say it was.
    assert!(!view.truncated);
    assert_eq!(view.messages.len(), 1);
    assert_eq!(view.messages[0].parts.len(), 4);
    assert_eq!(view.tools.len(), 3);
    // Oldest first: the earliest tools give theirs up, their details as they
    // were; what still fits is kept.
    assert_eq!(view.tools[0].structured_content, None);
    assert_eq!(view.tools[0].details, "d".repeat(12_000));
    assert_eq!(
        view.tools[2].structured_content.as_deref(),
        Some(json.as_str())
    );
}

#[test]
fn a_tool_without_an_mcp_identity_is_written_as_before() {
    let events = vec![event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new("shell").unwrap(),
        Some("Shell".into()),
        None,
        None,
        None,
        Some(vec![ToolContent::text("out")]),
    )))];
    let wire = serde_json::to_value(&committed_tool_view(&events).tools[0]).unwrap();
    let mut keys: Vec<_> = wire.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(
        keys,
        [
            "details",
            "executionId",
            "input",
            "kind",
            "status",
            "title",
            "toolId"
        ]
    );
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

/// The view agrees with what the client checks before it will show it.
///
/// Every ask and review belongs to a message that is running, and everything
/// pending to one that is queued — or, in a truncated view, to one no longer
/// shown. The client refuses the whole view otherwise,
/// so the projection has to agree with it rather than hope the two never meet.
#[test]
fn an_ask_whose_closure_never_reached_storage_is_not_offered_after_restart() {
    // The gateway stopped with an ask open, so the ask was saved and its
    // closure was not. Restored, the message is unresolved; offering the ask
    // beside it made the client refuse the view on every restart.
    let snapshot = review_snapshot(vec![asked("execution", "1")]);
    let restored = Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
            agent_features: OperationCapabilities::default().into(),
        },
        Some(&snapshot),
    );
    assert!(restored.read().questions.is_empty());
}

#[test]
fn committed_partial_progress_cannot_be_replaced_by_an_older_complete_read() {
    let mut projection = projection();
    projection.replace_committed(&committed("incarnation", 1, 3, 3, None), &[], None);
    projection.transcript_state(ConversationTranscriptState::Partial);
    assert!(!projection.replace_committed(&committed("incarnation", 1, 1, 1, None), &[], None));
    assert_eq!(
        projection.read().transcript_state,
        ConversationTranscriptState::Partial
    );
    assert!(!projection.read().capabilities.permissions);
}

#[test]
fn a_review_reaching_beyond_its_request_is_not_offered() {
    let scoped = PermissionDecision::new(
        PermissionEffect::Allow,
        PermissionScope::session(
            PermissionApplicationId::new("app").unwrap(),
            PermissionSessionId::new("session").unwrap(),
        ),
    );
    let options = PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("always").unwrap(),
            "Allow for this session",
            scoped.clone(),
        )
        .unwrap()],
        &PermissionOfferPolicy::new(vec![scoped]).unwrap(),
    )
    .unwrap();
    let ExecutionUpdate::PermissionRequested {
        id,
        tool_id,
        observation,
        input,
        ..
    } = review("{}".into()).update().clone()
    else {
        unreachable!()
    };
    let snapshot = review_snapshot(vec![event(ExecutionUpdate::PermissionRequested {
        id,
        tool_id,
        observation,
        input,
        options,
    })]);
    let mut projection = projection();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&ExecutionId::new("execution").unwrap()),
    );
    projection.transcript_state(ConversationTranscriptState::Complete);
    let view = projection.read();
    // Its allow would read as one for this request alone: no choice is offered.
    assert!(view.permissions.is_empty());
    assert!(view.interaction_view_error.is_some());
}

#[test]
fn a_review_says_what_each_offered_option_decides() {
    let options = PermissionOptions::new(
        vec![
            PermissionOption::new(
                PermissionOptionId::new("first").unwrap(),
                "Not like this",
                PermissionDecision::new(PermissionEffect::Deny, PermissionScope::request()),
            )
            .unwrap(),
            PermissionOption::new(
                PermissionOptionId::new("second").unwrap(),
                "Go ahead",
                PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
            )
            .unwrap(),
        ],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap();
    let ExecutionUpdate::PermissionRequested {
        id,
        tool_id,
        observation,
        input,
        ..
    } = review("{}".into()).update().clone()
    else {
        unreachable!()
    };
    let snapshot = review_snapshot(vec![event(ExecutionUpdate::PermissionRequested {
        id,
        tool_id,
        observation,
        input,
        options,
    })]);
    let mut projection = projection();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&ExecutionId::new("execution").unwrap()),
    );
    projection.transcript_state(ConversationTranscriptState::Complete);
    let view = projection.read();
    let offered: Vec<_> = view.permissions[0]
        .options
        .iter()
        .map(|option| (option.id.as_str(), option.effect))
        .collect();
    // On the wire as the schema names it.
    let wire = serde_json::to_value(&view.permissions[0].options).unwrap();
    assert_eq!(wire[0]["effect"], "deny");
    assert_eq!(wire[1]["effect"], "allow");
    // The effect is the domain's decision, whatever the label or the order.
    assert_eq!(
        offered,
        [
            ("first", ConversationPermissionOptionEffect::Deny),
            ("second", ConversationPermissionOptionEffect::Allow),
        ]
    );
}

#[test]
fn only_the_exact_live_execution_can_offer_a_committed_interaction() {
    let mut projection = projection();
    let snapshot = review_snapshot(vec![asked("execution", "question"), review("{}".into())]);
    let unrelated = ExecutionId::new("other").unwrap();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&unrelated),
    );
    projection.transcript_state(ConversationTranscriptState::Complete);
    assert!(projection.read().questions.is_empty());
    assert!(projection.read().permissions.is_empty());
    let exact = ExecutionId::new("execution").unwrap();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&exact),
    );
    projection.transcript_state(ConversationTranscriptState::Complete);
    assert_eq!(projection.read().questions.len(), 1);
    assert_eq!(projection.read().permissions.len(), 1);
    assert!(!projection.replace_committed(
        &committed("replacement-incarnation", 2, 2, 2, Some(&snapshot)),
        &[],
        Some(&exact)
    ));
}

#[test]
fn committed_rendering_bounds_json_escaping_and_preserves_ordered_part_identity() {
    let mut snapshot = review_snapshot(vec![
        event(ExecutionUpdate::Message(
            MessageChunk::text("quoted \" text")
                .with_message_id(MessageId::new("response").unwrap()),
        )),
        event(ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("tool").unwrap(),
            None,
            None,
            None,
            None,
            None,
        ))),
        event(ExecutionUpdate::Message(MessageChunk::thought("thought"))),
    ]);
    let template = snapshot.invocations[0].clone();
    for index in 1..40 {
        let mut record = template.clone();
        let id = ExecutionId::new(index.to_string()).unwrap();
        record.request.execution_id = id.clone();
        record.events = vec![ExecutionEvent::new(
            id,
            ExecutionUpdate::Message(MessageChunk::text("\"\\\n".repeat(MAX_TEXT))),
        )];
        snapshot.invocations.push(record);
    }
    let view = Projection::new(
        "conversation".into(),
        projection().view.capabilities,
        Some(&snapshot),
    )
    .read();
    let view = bound_view(view);
    assert!(view.truncated);
    assert!(serde_json::to_vec(&view).unwrap().len() <= 60_000);
    let first = Projection::new(
        "conversation".into(),
        projection().view.capabilities,
        Some(&review_snapshot(template.events)),
    )
    .read();
    assert_eq!(
        first.messages[0].parts[0].message_id.as_deref(),
        Some("response")
    );
    assert_eq!(first.messages[0].parts[1].kind, "tool");
    assert_eq!(first.messages[0].parts[2].offset, 2);
}

#[test]
fn older_observed_head_cannot_reenable_committed_controls() {
    let mut projection = projection();
    assert!(projection.replace_committed(&committed("incarnation", 1, 1, 2, None), &[], None));
    projection.transcript_state(ConversationTranscriptState::Stale);
    let before = projection.read();
    if projection.replace_committed(&committed("incarnation", 1, 1, 1, None), &[], None) {
        projection.transcript_state(ConversationTranscriptState::Complete);
    }
    assert_eq!(
        serde_json::to_value(projection.read()).unwrap(),
        serde_json::to_value(before).unwrap()
    );
    assert_eq!(
        projection.read().transcript_state,
        ConversationTranscriptState::Stale
    );
}

#[test]
fn active_interactions_survive_recent_message_window_without_copying_the_message() {
    let sibling_ask = event(ExecutionUpdate::QuestionAsked {
        id: QuestionId::new("question").unwrap(),
        question: AgentQuestion::new(
            "Choose the environments",
            (0..3)
                .map(|index| {
                    Question::new(
                        format!("question_{index}"),
                        format!("Environment {index}?"),
                        None,
                        AnswerShape::One,
                        vec![AnswerOption::new("staging", "Staging", None).unwrap()],
                        None,
                        false,
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap(),
    });
    let requested = review("{}".into());
    let mut snapshot = review_snapshot(vec![sibling_ask, requested.clone()]);
    snapshot.invocations[0].request.user_message =
        said(&"x".repeat(ExecutionRequest::MAX_MESSAGE_BYTES));
    let mut order = Vec::new();
    for index in 0..25 {
        let id = ExecutionId::new(format!("queued-{index}")).unwrap();
        let mut record = review_snapshot(Vec::new()).invocations.remove(0);
        record.request.execution_id = id.clone();
        record.submission = InvocationSubmissionMode::Queued;
        record.scheduling = vec![InvocationSchedulingEvent {
            kind: InvocationKind::Queued,
            target: None,
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(record.actor.clone()),
        }];
        snapshot.queue_history.push(QueueHistoryRecord {
            mutation: QueueMutation::Admitted {
                id: id.clone(),
                kind: InvocationKind::Queued,
            },
            actor: Some(record.actor.clone()),
            scheduling_length: Some(1),
        });
        snapshot.invocations.push(record);
        order.push(id);
    }
    let current = committed("incarnation", 27, 27, 27, Some(&snapshot));
    let active = ExecutionId::new("execution").unwrap();
    let mut projection = self::projection();
    assert!(projection.replace_committed(&current, &order, Some(&active)));
    projection.transcript_state(ConversationTranscriptState::Complete);
    let view = projection.read();
    assert!(view.truncated);
    assert!(view
        .messages
        .iter()
        .all(|message| message.execution_id != "execution"));
    assert!(view.messages.len() <= 24);
    assert_eq!(view.questions.len(), 1);
    assert_eq!(view.questions[0].questions.len(), 3);
    assert_eq!(view.permissions.len(), 1);
    let view = bound_view(view);
    assert!(serde_json::to_vec(&view).unwrap().len() <= 60_000);
    assert!(projection.replace_committed(&current, &order, Some(&active)));
    assert_eq!(projection.read().questions.len(), 1);
    assert_eq!(projection.read().permissions.len(), 1);
    assert!(projection.replace_committed(&current, &order, None));
    assert!(projection.read().questions.is_empty());
    assert!(projection.read().permissions.is_empty());
    assert!(projection.replace_committed(&current, &order, Some(&active)));
    assert_eq!(projection.read().questions.len(), 1);
    // The extra record uses the same encoded-byte owner as recent records.
    // Open ask cost is below the admission owner's40KiB bound; complete
    // reviews give way before any actual question or answer option is cut.
    let mut costly = snapshot.clone();
    costly.invocations[0].events.clear();
    let mut ask_cost = 0;
    for index in 0..2 {
        let question = AgentQuestion::new(
            "\"".repeat(1024),
            (0..3)
                .map(|sibling| {
                    Question::new(
                        format!("large_{index}_{sibling}"),
                        "\"".repeat(1024),
                        Some("\\".repeat(256)),
                        AnswerShape::One,
                        vec![AnswerOption::new(
                            "staging",
                            "\"".repeat(1024),
                            Some("\\".repeat(256)),
                        )
                        .unwrap()],
                        None,
                        false,
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        ask_cost += question.carrying_cost();
        costly.invocations[0]
            .events
            .push(event(ExecutionUpdate::QuestionAsked {
                id: QuestionId::new(format!("large-{index}")).unwrap(),
                question,
            }));
    }
    assert!(ask_cost <= MAX_OPEN_ASK_COST);
    for index in 0..8 {
        let ExecutionUpdate::PermissionRequested {
            observation,
            input,
            options,
            ..
        } = review(serde_json::to_string(&"\"".repeat(3000)).unwrap())
            .update()
            .clone()
        else {
            unreachable!()
        };
        costly.invocations[0]
            .events
            .push(event(ExecutionUpdate::PermissionRequested {
                id: PermissionId::new(format!("costly-{index}")).unwrap(),
                tool_id: ToolCallId::new(format!("costly-tool-{index}")).unwrap(),
                observation,
                input,
                options,
            }));
    }
    let large = committed("incarnation", 27, 27, 27, Some(&costly));
    let mut bounded = self::projection();
    assert!(bounded.replace_committed(&large, &order, Some(&active)));
    bounded.transcript_state(ConversationTranscriptState::Complete);
    let view = bounded.read();
    let view = bound_view(view);
    assert!(serde_json::to_vec(&view).unwrap().len() <= 60_000);
    assert_eq!(view.questions.len(), 2);
    for (index, question) in view.questions.iter().enumerate() {
        assert_eq!(question.questions.len(), 3);
        for (sibling, asked) in question.questions.iter().enumerate() {
            assert_eq!(asked.key, format!("large_{index}_{sibling}"));
            assert_eq!(asked.prompt.len(), 1024);
            assert_eq!(asked.options[0].label.len(), 1024);
        }
    }
    assert!(view.permissions.len() < 8);
    assert!(view.interaction_view_error.is_some());
    for permission in &view.permissions {
        assert_eq!(permission.options.len(), 1);
        assert_eq!(permission.arguments_json.len(), 6002);
    }
    let mut owner = ExecutionController::new(ExecutionSessionId::new("provider-session").unwrap());
    owner.begin_execution(active.clone()).unwrap();
    let ExecutionUpdate::PermissionRequested {
        id, input, options, ..
    } = requested.update()
    else {
        unreachable!()
    };
    owner
        .request_permission(
            &active,
            id.clone(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            input.clone(),
            options.clone(),
        )
        .unwrap();
    let cancelled = owner
        .cancel_permission(
            &active,
            id,
            PermissionCancellationReason::provider_withdrawal(),
            CancellationOrigin::Provider,
        )
        .unwrap()
        .unwrap();
    snapshot.invocations[0]
        .events
        .push(event(ExecutionUpdate::PermissionCancelled(cancelled)));
    snapshot.invocations[0]
        .events
        .push(event(ExecutionUpdate::QuestionClosed {
            id: QuestionId::new("question").unwrap(),
        }));
    let closed = committed("incarnation", 28, 28, 28, Some(&snapshot));
    assert!(projection.replace_committed(&closed, &order, Some(&active)));
    assert!(projection.read().questions.is_empty());
    assert!(projection.read().permissions.is_empty());
    assert!(projection.replace_committed(&closed, &order, Some(&active)));
    assert!(projection.read().questions.is_empty());
    assert!(projection.read().permissions.is_empty());
    let mut reset = self::projection();
    assert!(reset.replace_committed(&current, &order, Some(&active)));
    reset.transcript_state(ConversationTranscriptState::Complete);
    assert_eq!(reset.read().questions[0].questions.len(), 3);
    assert_eq!(reset.read().permissions.len(), 1);
}

#[test]
fn custom_backend_questions_over_display_budget_remain_semantic_and_atomic() {
    let execution = ExecutionId::new("execution").unwrap();
    let mut controller =
        ExecutionController::new(ExecutionSessionId::new("provider-session").unwrap());
    controller.begin_execution(execution.clone()).unwrap();
    let mut events = Vec::new();
    let mut carry = 0;
    for index in 0..4 {
        let question = AgentQuestion::new(
            "\"".repeat(1024),
            (0..3)
                .map(|sibling| {
                    Question::new(
                        format!("custom_{index}_{sibling}"),
                        "\"".repeat(1024),
                        Some("\\".repeat(256)),
                        AnswerShape::One,
                        vec![
                            AnswerOption::new("yes", "\"".repeat(1024), Some("\\".repeat(256)))
                                .unwrap(),
                        ],
                        None,
                        false,
                    )
                    .unwrap()
                })
                .collect(),
        )
        .unwrap();
        carry += question.carrying_cost();
        events.push(
            controller
                .ask_question(
                    &execution,
                    QuestionId::new(format!("custom-{index}")).unwrap(),
                    question,
                )
                .unwrap()
                .event(),
        );
    }
    assert!(carry > MAX_OPEN_ASK_COST);
    for index in 0..8 {
        let ExecutionUpdate::PermissionRequested { input, options, .. } =
            review(serde_json::to_string(&"\"".repeat(3000)).unwrap())
                .update()
                .clone()
        else {
            unreachable!()
        };
        events.push(
            controller
                .request_permission(
                    &execution,
                    PermissionId::new(format!("mixed-review-{index}")).unwrap(),
                    ToolCallUpdate::new(
                        ToolCallId::new(format!("mixed-tool-{index}")).unwrap(),
                        None,
                        None,
                        None,
                        None,
                        None,
                    ),
                    input,
                    options,
                )
                .unwrap(),
        );
    }
    let snapshot = review_snapshot(events);
    let complete = committed("incarnation", 5, 5, 5, Some(&snapshot));
    assert_eq!(complete.snapshot().unwrap().invocations[0].events.len(), 12);
    let mut projection = self::projection();
    assert!(projection.replace_committed(&complete, &[], Some(&execution)));
    projection.transcript_state(ConversationTranscriptState::Complete);
    let view = projection.read();
    let view = bound_view(view);
    assert!(view.truncated);
    assert!(serde_json::to_vec(&view).unwrap().len() <= 60_000);
    assert!(!view.questions.is_empty());
    assert!(view.questions.len() < 4);
    assert!(view.permissions.len() < 8);
    assert_eq!(view.transcript_state, ConversationTranscriptState::Complete);
    assert!(view
        .interaction_view_error
        .as_deref()
        .unwrap()
        .contains("Use Stop"));
    assert!(view
        .interaction_view_error
        .as_deref()
        .unwrap()
        .contains("pending interactions"));
    for (index, ask) in view.questions.iter().enumerate() {
        assert_eq!(ask.question_id, format!("custom-{index}"));
        assert_eq!(ask.questions.len(), 3);
        for (sibling, question) in ask.questions.iter().enumerate() {
            assert_eq!(question.key, format!("custom_{index}_{sibling}"));
            assert_eq!(question.prompt, "\"".repeat(1024));
            assert_eq!(question.options.len(), 1);
            assert_eq!(question.options[0].label, "\"".repeat(1024));
        }
    }
    // Renderer omission does not mutate the full committed continuation.
    assert_eq!(complete.snapshot().unwrap().invocations[0].events.len(), 12);
}

#[test]
fn retained_projection_uses_shared_bounds_status_and_injected_revision() {
    let id = ConversationId::new("00000000-0000-0000-0000-000000000001").unwrap();
    let revision = Uuid::from_u128(1);
    let mut snapshot = completed_snapshot("first", vec![]);
    snapshot.id = SessionId::new(id.to_string()).unwrap();
    snapshot.invocations = (0..25)
        .map(|index| {
            let mut record = snapshot.invocations[0].clone();
            record.request.execution_id = ExecutionId::new(format!("execution-{index}")).unwrap();
            record.request.user_message = said(&"\"\n😀".repeat(4000));
            record
        })
        .collect();
    for (completeness, freshness, state) in [
        (
            CommittedCompleteness::NotLoaded,
            CommittedFreshness::Current,
            ConversationTranscriptState::NotLoaded,
        ),
        (
            CommittedCompleteness::CompleteEmpty,
            CommittedFreshness::Current,
            ConversationTranscriptState::CompleteEmpty,
        ),
        (
            CommittedCompleteness::Complete,
            CommittedFreshness::Current,
            ConversationTranscriptState::Complete,
        ),
        (
            CommittedCompleteness::Partial,
            CommittedFreshness::Current,
            ConversationTranscriptState::Partial,
        ),
        (
            CommittedCompleteness::Partial,
            CommittedFreshness::Stale,
            ConversationTranscriptState::Stale,
        ),
        (
            CommittedCompleteness::Complete,
            CommittedFreshness::Unknown,
            ConversationTranscriptState::Unknown,
        ),
    ] {
        let snapshot = if matches!(
            completeness,
            CommittedCompleteness::NotLoaded | CommittedCompleteness::CompleteEmpty
        ) {
            None
        } else {
            Some(&snapshot)
        };
        let status = CommittedStatus::new(completeness, freshness);
        let first = retained_view(&id, snapshot, status, revision);
        let second = retained_view(&id, snapshot, status, revision);
        assert_eq!(first.conversation_id, id.to_string());
        assert!(first.revision.starts_with(&revision.to_string()));
        assert_eq!(first.transcript_state, state);
        assert_eq!(first.capabilities, ConversationCapabilities::read_only());
        assert!(
            !first.capabilities.queue
                && !first.capabilities.steer
                && !first.capabilities.resume
                && !first.capabilities.permissions
                && !first.capabilities.image_input
        );
        assert!(first.permissions.is_empty() && first.questions.is_empty());
        let bytes = serde_json::to_vec(&first).unwrap();
        assert!(bytes.len() <= MAX_VIEW_BYTES);
        assert_eq!(bytes, serde_json::to_vec(&second).unwrap());
        assert_eq!(first.messages.is_empty(), snapshot.is_none());
        if snapshot.is_some() {
            assert!(first.truncated);
            assert!(first
                .messages
                .iter()
                .all(|message| message.status == ConversationMessageStatus::Completed));
        }
    }
}

#[test]
fn typed_authentication_refusal_survives_projection_and_not_diagnostic_text() {
    for (error, expected) in [
        (
            AgentError::AuthenticationRequired {
                diagnostic: Some(ProviderDiagnostic::new("OAuth session expired")),
            },
            Some(true),
        ),
        (
            AgentError::Provider {
                code: -32000,
                diagnostic: Some(ProviderDiagnostic::new(
                    "provider plan does not allow this request",
                )),
            },
            None,
        ),
        (
            AgentError::Provider {
                code: -32603,
                diagnostic: Some(ProviderDiagnostic::new("OAuth session expired")),
            },
            None,
        ),
    ] {
        let report = ExecutionReport::new(Some(Err(error)), None, ProviderSessionState::Usable);
        let mut snapshot = review_snapshot(Vec::new());
        snapshot.invocations[0].result = Some(report.clone().into_result());
        snapshot.invocations[0].provider_report = Some(report);
        let restored = Projection::new(
            "conversation".into(),
            ConversationCapabilities {
                queue: true,
                steer: true,
                resume: false,
                permissions: true,
                image_input: false,
                agent_features: OperationCapabilities::default().into(),
            },
            Some(&snapshot),
        );
        let message = &restored.read().messages[0];
        assert_eq!(message.authentication_required, expected);
        if expected.is_none() {
            assert!(message.error.as_ref().is_some_and(|value| value
                .contains("provider plan does not allow this request")
                || value.contains("OAuth session expired")));
        } else {
            assert_eq!(message.error, None);
        }
    }
}

#[test]
fn authentication_recovery_keeps_independent_required_work_failure() {
    for independent in [None, Some(AgentError::AuditFailure)] {
        let report = ExecutionReport::new(
            Some(Err(AgentError::AuthenticationRequired {
                diagnostic: Some(ProviderDiagnostic::new("login expired")),
            })),
            independent.clone(),
            ProviderSessionState::Usable,
        );
        let mut snapshot = review_snapshot(Vec::new());
        snapshot.invocations[0].result = Some(report.clone().into_result());
        snapshot.invocations[0].provider_report = Some(report);
        let projected = Projection::new(
            "conversation".into(),
            ConversationCapabilities {
                queue: true,
                steer: true,
                resume: false,
                permissions: true,
                image_input: false,
                agent_features: OperationCapabilities::default().into(),
            },
            Some(&snapshot),
        );
        let view = projected.read();
        assert_eq!(view.messages[0].authentication_required, Some(true));
        assert_eq!(
            view.messages[0].error.as_deref(),
            independent.map(|_| "The turn could not complete all required work.")
        );
    }
}
