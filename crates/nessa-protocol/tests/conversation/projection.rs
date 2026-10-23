//! Projections are bounded display state, not permission or scheduling authority.
//! The cases that drive `ConversationService` stay in the gateway's
//! `tests/conversation/projection.rs`.
use crate::agents::AgentId;
use crate::conversation::domain::{ConversationApprovalMode, ConversationId, ConversationModelId};
use crate::conversation::projection::{
    agent_question, agent_review, bound_view, bound_view_within, clipped, question_entry,
    review_entry, Projection, MAX_LISTED_REVIEWS, MAX_QUESTION_ENTRY_BYTES,
    MAX_QUESTION_ENTRY_MESSAGE_BYTES, MAX_REVIEW_ENTRY_BYTES, MAX_REVIEW_ENTRY_TOOL_NAME_BYTES,
    MAX_TEXT, MAX_VIEW_BYTES,
};
use crate::conversation::view::{
    ConversationModelChangeEffect, ConversationModelChangeStatus, ConversationModelChangeView,
    ConversationPermissionAsk, ConversationPermissionOptionEffect, ConversationReviewKind,
    ConversationRuntime, ConversationSelectionView, ConversationTranscriptState,
};
use crate::conversation::{
    projection::retained_view,
    view::{
        ConversationAgentFeatures, ConversationAttachmentEvidenceFailure,
        ConversationAttachmentEvidenceFailureCode, ConversationCapabilities, ConversationLifecycle,
        ConversationLifecyclePhase, ConversationMessageStatus, ConversationView, NativeHooksChoice,
        PermissionDenialSupport, MAX_STRUCTURED_CONTENT_BYTES,
    },
};
use nessa_sdk::application::agent_execution::agents::{
    AgentError, ProviderDiagnostic, UsageLimitKind, UsageLimitProvider,
};
use nessa_sdk::application::agent_execution::executions::{
    ExecutionController, ExecutionEvent, ExecutionRequest, ExecutionUpdate,
    SubmissionMode as InvocationSubmissionMode,
};
use nessa_sdk::application::agent_execution::permissions::{
    ActionContext, ApprovalRuleReference, CancellationOrigin, DeclinedReview,
};
use nessa_sdk::application::agent_execution::providers::{
    ExecutionReport, OperationCapabilities, ProviderIdentity, ProviderSessionState,
};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedCompleteness, CommittedFreshness, CommittedSession, CommittedStatus, InvocationRecord,
    InvocationSchedulingEvent, QueueHistoryRecord, SessionSnapshot, StorageError,
    SubmissionAcknowledgement,
};
use nessa_sdk::application::agent_execution::tools::ToolReviewInput;
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, Initiation, InvocationKind, InvocationStage, MessageChunk,
    MessageId, QueueMutation, SchedulingCause,
};
use nessa_sdk::domain::agent_execution::permissions::{
    CustomPermissionCancellationReason, PendingQuestion, PendingReview, PermissionApplicationId,
    PermissionCancellationReason, PermissionDecision, PermissionEffect, PermissionId,
    PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionReviewDeadline, PermissionScope, PermissionSessionId, PermissionToolClass,
    RefusalOutcome, ReviewContent, ReviewDecline, ReviewDeclineId, ReviewDeclineReason,
    ReviewDeclineStage, MAX_OPEN_REVIEWS, STANDARD_DENY_REASON, STANDARD_STOP_REASON,
};
use nessa_sdk::domain::agent_execution::prompts::{PromptText, UserMessage};
use nessa_sdk::domain::agent_execution::questions::{
    AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId, MAX_OPEN_QUESTIONS,
};
use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, SessionId,
};
use nessa_sdk::domain::agent_execution::tools::{
    FilePath, McpCallArguments, McpTool, ToolCallId, ToolCallInput, ToolCallUpdate, ToolContent,
    ToolObservation, ToolStatus,
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
    assert_eq!(value["nativeHooks"], "off");
    assert_eq!(value["compactionReporting"], "unsupported_not_implemented");
    assert_eq!(value["modelSwitchReporting"], "unsupported_not_implemented");
    assert_eq!(value["permissionDeferral"], "unsupported_not_implemented");
    assert_eq!(value["preToolPolicy"], "unsupported_not_implemented");
    assert_eq!(value["policyEndTurn"], "unknown");
    assert_eq!(value["policyCloseSession"], "unknown");
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
        id: PermissionId::new("7").unwrap(),
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
        deadline: None,
    })
}
/// The fixture review, published with a deadline that takes `effect` for `reason`.
fn review_with_deadline(effect: PermissionEffect, reason: &str) -> ExecutionEvent {
    let ExecutionUpdate::PermissionRequested {
        id,
        tool_id,
        observation,
        input,
        options,
        ..
    } = review("{}".into()).update().clone()
    else {
        unreachable!()
    };
    event(ExecutionUpdate::PermissionRequested {
        id,
        tool_id,
        observation,
        input,
        options,
        deadline: Some(
            PermissionReviewDeadline::new(
                PermissionToolClass::Other,
                std::time::Duration::from_secs(30),
                effect,
                reason,
            )
            .unwrap()
            .promise(1_700_000_030_000)
            .unwrap(),
        ),
    })
}
/// The fixture review cancelled with `reason`, as a live controller records it.
fn cancelled_review(
    reason: PermissionCancellationReason,
    origin: CancellationOrigin,
) -> ExecutionEvent {
    let ExecutionUpdate::PermissionRequested {
        id, input, options, ..
    } = review("{}".into()).update().clone()
    else {
        unreachable!()
    };
    let execution = ExecutionId::new("execution").unwrap();
    let mut owner = ExecutionController::new(ExecutionSessionId::new("provider-session").unwrap());
    owner.begin_execution(execution.clone()).unwrap();
    owner
        .request_permission(
            &execution,
            id.clone(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            input,
            options,
            None,
        )
        .unwrap();
    let cancelled = owner
        .cancel_permission(&execution, &id, reason, origin)
        .unwrap()
        .unwrap();
    event(ExecutionUpdate::PermissionCancelled(cancelled))
}
/// The review cancelled by `deadline`, the one it published, because no
/// refusal was offered.
fn deadline_cancelled_review(deadline: PermissionReviewDeadline) -> ExecutionEvent {
    let ExecutionUpdate::PermissionRequested {
        id, input, options, ..
    } = review("{}".into()).update().clone()
    else {
        unreachable!()
    };
    let execution = ExecutionId::new("execution").unwrap();
    let mut owner = ExecutionController::new(ExecutionSessionId::new("provider-session").unwrap());
    owner.begin_execution(execution.clone()).unwrap();
    owner
        .request_permission(
            &execution,
            id.clone(),
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                None,
                None,
                None,
                None,
                None,
            ),
            input,
            options,
            Some(deadline.clone().promise(1_700_000_030_000).unwrap()),
        )
        .unwrap();
    let cancelled = owner
        .cancel_deadline_for_test(&execution, &id, deadline)
        .unwrap()
        .unwrap();
    event(ExecutionUpdate::PermissionCancelled(cancelled))
}
fn review_snapshot(events: Vec<ExecutionEvent>) -> SessionSnapshot {
    SessionSnapshot {
        model_change: None,
        current_configuration: None,
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
            initiation: Initiation::owner(),
            notes: Vec::new(),
            handoff: None,
            acknowledgement: SubmissionAcknowledgement::Acknowledged,
            events,
            scheduling: vec![],
            cancellation: None,
            provider_report: None,
            local_cancellation: None,
            local_outcome: None,
            result: None,
        }],
        lease: None,
        artifacts: Vec::new(),
        handoffs: Vec::new(),
        notes: Vec::new(),
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
fn an_mcp_tool_carries_the_arguments_its_connection_saw_and_a_later_update_keeps_them() {
    let tool = ToolCallId::new("chart").unwrap();
    let arguments = McpCallArguments::new(r#"{"city":"Oslo"}"#).unwrap();
    let mut events = vec![event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            tool.clone(),
            Some("mcp.charts.show".into()),
            None,
            None,
            None,
            None,
        )
        .with_mcp_tool(McpTool::new("charts", "show").unwrap())
        .with_mcp_arguments(arguments),
    ))];
    let view = committed_tool_view(&events);
    assert_eq!(
        view.tools[0]
            .mcp
            .as_ref()
            .unwrap()
            .arguments_json
            .as_deref(),
        Some(r#"{"city":"Oslo"}"#)
    );
    assert_eq!(
        serde_json::to_value(&view.tools[0]).unwrap()["mcp"]["argumentsJson"],
        r#"{"city":"Oslo"}"#
    );
    // The same server and tool, without repeating the arguments, keeps them.
    events.push(event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            tool.clone(),
            None,
            None,
            Some(ToolStatus::Completed),
            None,
            None,
        )
        .with_mcp_tool(McpTool::new("charts", "show").unwrap()),
    )));
    assert_eq!(
        committed_tool_view(&events).tools[0]
            .mcp
            .as_ref()
            .unwrap()
            .arguments_json
            .as_deref(),
        Some(r#"{"city":"Oslo"}"#)
    );
    // A different tool does not.
    events.push(event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(tool, None, None, None, None, None)
            .with_mcp_tool(McpTool::new("charts", "other").unwrap()),
    )));
    let replaced = committed_tool_view(&events);
    let mcp = replaced.tools[0].mcp.as_ref().unwrap();
    assert_eq!(mcp.tool, "other");
    assert_eq!(mcp.arguments_json, None);
    assert!(serde_json::to_value(mcp)
        .unwrap()
        .get("argumentsJson")
        .is_none());
}

#[test]
fn a_tool_spans_from_its_first_report_to_the_one_that_settles_it_and_keeps_its_input() {
    let tool = ToolCallId::new("grep").unwrap();
    let at = |update: ToolCallUpdate, ms: u64| {
        event(ExecutionUpdate::Tool(update)).with_observed_at_ms(ms)
    };
    let bare = |status| ToolCallUpdate::new(tool.clone(), None, None, status, None, None);
    let mut events = vec![at(
        bare(Some(ToolStatus::Pending))
            .with_input(ToolCallInput::new(r#"{"pattern":"todo"}"#).unwrap()),
        1_000,
    )];
    let running = committed_tool_view(&events);
    assert_eq!(running.tools[0].started_at_ms, Some(1_000));
    assert_eq!(running.tools[0].ended_at_ms, None);
    assert_eq!(running.tools[0].input, r#"{"pattern":"todo"}"#);
    // A report that does not repeat the input leaves it; one that settles the
    // call ends it, and a later report of the settled call does not move that.
    events.push(at(bare(Some(ToolStatus::Running)), 1_200));
    events.push(at(bare(Some(ToolStatus::Completed)), 1_900));
    events.push(at(bare(Some(ToolStatus::Completed)), 2_500));
    let settled = committed_tool_view(&events);
    assert_eq!(settled.tools[0].started_at_ms, Some(1_000));
    assert_eq!(settled.tools[0].ended_at_ms, Some(1_900));
    assert_eq!(settled.tools[0].input, r#"{"pattern":"todo"}"#);
    let json = serde_json::to_value(&settled.tools[0]).unwrap();
    assert_eq!(json["startedAtMs"], 1_000);
    assert_eq!(json["endedAtMs"], 1_900);
    // A call saved before times were kept shows none, rather than a guess.
    let old = committed_tool_view(&[event(ExecutionUpdate::Tool(bare(Some(
        ToolStatus::Completed,
    ))))]);
    assert_eq!(old.tools[0].started_at_ms, None);
    assert_eq!(old.tools[0].ended_at_ms, None);
    let json = serde_json::to_value(&old.tools[0]).unwrap();
    assert!(json.get("startedAtMs").is_none() && json.get("endedAtMs").is_none());
    // A call first reported before then, and settled after, keeps its start
    // unknown: the settling report's time is its end, not its start.
    let straddling = committed_tool_view(&[
        event(ExecutionUpdate::Tool(bare(Some(ToolStatus::Running)))),
        at(bare(Some(ToolStatus::Completed)), 3_000),
    ]);
    assert_eq!(straddling.tools[0].started_at_ms, None);
    assert_eq!(straddling.tools[0].ended_at_ms, Some(3_000));
    // A time no JSON reader can hold, read back from a damaged file, is left
    // unknown rather than refusing the whole view.
    let damaged = committed_tool_view(&[at(
        bare(Some(ToolStatus::Completed)),
        crate::conversation::domain::LATEST_TIME_MS + 1,
    )]);
    assert_eq!(damaged.tools[0].started_at_ms, None);
    assert_eq!(damaged.tools[0].ended_at_ms, None);
}

#[test]
fn a_tool_lists_the_files_it_changed_and_says_when_a_side_was_cut() {
    use crate::conversation::view::{
        ConversationToolChange, MAX_TOOL_CHANGES, MAX_TOOL_CHANGE_SIDE_BYTES,
    };
    let tool = ToolCallId::new("edit").unwrap();
    let path = |name: &str| FilePath::new(name).unwrap();
    let long = "x".repeat(MAX_TOOL_CHANGE_SIDE_BYTES + 10);
    let view = committed_tool_view(&[event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(tool.clone(), None, None, None, None, None).with_content(vec![
            ToolContent::text("Applied"),
            // Created: nothing before it, which is not an empty file.
            ToolContent::diff(path("new.ts"), None, "export {}"),
            ToolContent::diff(path("big.ts"), Some(long.clone()), "short"),
        ]),
    ))]);
    assert_eq!(
        view.tools[0].changes,
        vec![
            ConversationToolChange {
                path: "new.ts".into(),
                before: None,
                after: "export {}".into(),
                cut: false,
            },
            ConversationToolChange {
                path: "big.ts".into(),
                before: Some("x".repeat(MAX_TOOL_CHANGE_SIDE_BYTES)),
                after: "short".into(),
                cut: true,
            },
        ]
    );
    // The details still say the same in text, for readers that print them.
    assert!(view.tools[0].details.contains("File: new.ts"));
    let json = serde_json::to_value(&view.tools[0]).unwrap();
    assert_eq!(json["changes"][0]["path"], "new.ts");
    assert!(json["changes"][0].get("before").is_none());
    // Every reported file is listed, so the list is not marked cut.
    assert!(!view.tools[0].changes_cut);
    assert!(json.get("changesCut").is_none());
    // A later report replaces the changes, as it does the details; past the
    // bound the rest are left to the details.
    let many = (0..MAX_TOOL_CHANGES + 2)
        .map(|index| ToolContent::diff(path(&format!("f{index}.ts")), Some("a".into()), "b"))
        .collect();
    let view = committed_tool_view(&[event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(tool, None, None, None, None, None).with_content(many),
    ))]);
    assert_eq!(view.tools[0].changes.len(), MAX_TOOL_CHANGES);
    // The list says it leaves files out, so no reader totals it as the call's.
    assert!(view.tools[0].changes_cut);
    assert_eq!(
        serde_json::to_value(&view.tools[0]).unwrap()["changesCut"],
        true
    );
    // Text that fills the details does not hide a later file change.
    let view = committed_tool_view(&[event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            ToolCallId::new("long").unwrap(),
            None,
            None,
            None,
            None,
            None,
        )
        .with_content(vec![
            ToolContent::text("y".repeat(20_000)),
            ToolContent::diff(path("after.ts"), Some("a".into()), "b"),
        ]),
    ))]);
    assert!(view.tools[0].details.ends_with("[Output truncated]"));
    assert_eq!(view.tools[0].changes.len(), 1);
    assert_eq!(view.tools[0].changes[0].path, "after.ts");
    // A view short of room gives the list up and says so.
    let full = serde_json::to_vec(&view).unwrap().len();
    let bounded = bound_view_within(view, full - 1, true);
    assert!(bounded.tools[0].changes.is_empty());
    assert!(bounded.tools[0].changes_cut);
}

#[test]
fn arguments_on_a_later_update_join_the_identity_that_update_does_not_repeat() {
    let tool = ToolCallId::new("chart").unwrap();
    let mut events = vec![event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(tool.clone(), None, None, None, None, None)
            .with_mcp_tool(McpTool::new("charts", "show").unwrap()),
    ))];
    assert!(committed_tool_view(&events).tools[0]
        .mcp
        .as_ref()
        .unwrap()
        .arguments_json
        .is_none());
    events.push(event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(tool, None, None, Some(ToolStatus::Completed), None, None)
            .with_mcp_arguments(McpCallArguments::new(r#"{"city":"Oslo"}"#).unwrap()),
    )));
    let joined = committed_tool_view(&events);
    let mcp = joined.tools[0].mcp.as_ref().unwrap();
    assert_eq!(mcp.server, "charts");
    assert_eq!(mcp.tool, "show");
    assert_eq!(mcp.arguments_json.as_deref(), Some(r#"{"city":"Oslo"}"#));
    // No identity to join: the arguments are not invented onto the row.
    let bare = committed_tool_view(&[event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            ToolCallId::new("plain").unwrap(),
            None,
            None,
            None,
            None,
            None,
        )
        .with_mcp_arguments(McpCallArguments::new("{}").unwrap()),
    ))]);
    assert!(bare.tools[0].mcp.is_none());
}

fn tool_named(name: &str) -> ExecutionUpdate {
    ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new(name).unwrap(),
        None,
        None,
        None,
        None,
        None,
    ))
}

#[test]
fn a_restored_tool_shows_the_hooks_choice_saved_with_it_not_the_one_in_effect_now() {
    // After a restart with the switch turned on, the saved calls still say
    // what their process start used.
    let saved = completed_snapshot(
        "execution",
        vec![
            event(tool_named("read")).with_native_hooks(false),
            event(tool_named("edit")).with_native_hooks(false),
            event(tool_named("older")),
        ],
    );
    let mut capabilities = ConversationCapabilities {
        queue: true,
        steer: true,
        resume: false,
        permissions: true,
        image_input: false,
        agent_features: OperationCapabilities::default().into(),
    };
    capabilities.agent_features.native_hooks = NativeHooksChoice::On;
    let mut projection = Projection::new("conversation".into(), capabilities.clone(), None);
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&saved)),
        &[],
        None,
    ));
    let tools = &projection.view().tools;
    let mark = |id: &str| {
        tools
            .iter()
            .find(|tool| tool.tool_id == id)
            .unwrap()
            .native_hooks
    };
    assert_eq!(mark("read"), Some(NativeHooksChoice::Off));
    assert_eq!(mark("edit"), Some(NativeHooksChoice::Off));
    // Saved without a choice: nothing is claimed for it.
    assert_eq!(mark("older"), None);
    assert_eq!(projection.view().capabilities, capabilities);
}

#[test]
fn a_later_update_of_an_unmarked_tool_fills_in_its_hooks_choice() {
    let saved = completed_snapshot(
        "execution",
        vec![
            event(tool_named("read")),
            event(tool_named("read")).with_native_hooks(false),
        ],
    );
    let projection = Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
            agent_features: OperationCapabilities::default().into(),
        },
        Some(&saved),
    );
    assert_eq!(
        projection.view().tools[0].native_hooks,
        Some(NativeHooksChoice::Off)
    );
}

#[test]
fn saved_hooks_choices_that_disagree_within_one_execution_show_none() {
    // One execution runs on one process start, so these stamps cannot both be
    // true. The view skips the choice rather than show whichever came last.
    let saved = completed_snapshot(
        "execution",
        vec![
            event(tool_named("read")).with_native_hooks(true),
            event(tool_named("write")).with_native_hooks(false),
            event(tool_named("read")).with_native_hooks(true),
        ],
    );
    let projection = Projection::new(
        "conversation".into(),
        ConversationCapabilities {
            queue: true,
            steer: true,
            resume: false,
            permissions: true,
            image_input: false,
            agent_features: OperationCapabilities::default().into(),
        },
        Some(&saved),
    );
    assert_eq!(projection.view().tools.len(), 2);
    assert!(projection
        .view()
        .tools
        .iter()
        .all(|tool| tool.native_hooks.is_none()));
}

#[test]
fn a_view_past_its_budget_gives_up_call_arguments_after_its_history_and_before_the_tool() {
    let mut view = committed_tool_view(&[event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            ToolCallId::new("chart").unwrap(),
            None,
            None,
            None,
            None,
            Some(vec![ToolContent::text("out")]),
        )
        .with_mcp_tool(McpTool::new("charts", "show").unwrap())
        .with_mcp_arguments(
            McpCallArguments::new(format!(r#"{{"a":"{}"}}"#, "x".repeat(4_000))).unwrap(),
        ),
    ))]);
    // Nothing of the history can yield, so the arguments are why the tool
    // would go. They are left out and the tool stays, and the view does not
    // say it was truncated.
    let message = &mut view.messages[0];
    message.parts.clear();
    message.user_text.clear();
    message.files.clear();
    message.attachments.clear();
    message.retained_text = 0;
    let full = serde_json::to_vec(&view).unwrap().len();
    let bounded = bound_view_within(view, full - 1, true);
    assert_eq!(bounded.messages.len(), 1);
    assert_eq!(bounded.tools.len(), 1);
    assert!(bounded.tools[0]
        .mcp
        .as_ref()
        .unwrap()
        .arguments_json
        .is_none());
    assert!(!bounded.truncated);

    // An older message is why the view is over, so it yields and the
    // arguments stay.
    let mut view = committed_tool_view(&[event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(
            ToolCallId::new("chart").unwrap(),
            None,
            None,
            None,
            None,
            Some(vec![ToolContent::text("out")]),
        )
        .with_mcp_tool(McpTool::new("charts", "show").unwrap())
        .with_mcp_arguments(McpCallArguments::new(r#"{"a":1}"#).unwrap()),
    ))]);
    let older = view.messages[0].clone();
    view.messages.insert(0, older);
    let full = serde_json::to_vec(&view).unwrap().len();
    let bounded = bound_view_within(view, full - 1, true);
    assert!(bounded.truncated);
    assert_eq!(bounded.messages.len(), 1);
    assert_eq!(bounded.tools.len(), 1);
    assert_eq!(
        bounded.tools[0]
            .mcp
            .as_ref()
            .unwrap()
            .arguments_json
            .as_deref(),
        Some(r#"{"a":1}"#)
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
    assert_eq!(
        restored.read().messages[0].status,
        ConversationMessageStatus::Unresolved
    );
}

fn opened_on(snapshot: &SessionSnapshot) -> Projection {
    Projection::new(
        "conversation".into(),
        projection().view.capabilities.clone(),
        Some(snapshot),
    )
}

fn empty_history() -> SessionSnapshot {
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations.clear();
    snapshot
}

fn status_of(view: &ConversationView, id: &str) -> ConversationMessageStatus {
    view.messages
        .iter()
        .find(|message| message.execution_id == id)
        .expect("message")
        .status
}

/// The order in docs/design/transcript-fold.md: a turn this projection did
/// not open onto stays running through the gaps around the active execution,
/// and offers an interaction only while that execution is active.
#[test]
fn a_turn_admitted_after_open_stays_running_until_its_result() {
    let mut admitted = review_snapshot(vec![asked("execution", "1"), review("{}".into())]);
    let mut projection = opened_on(&empty_history());
    let execution = ExecutionId::new("execution").unwrap();
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&admitted)),
        &[],
        None,
    ));
    let saved = projection.read();
    assert_eq!(saved.messages[0].status, ConversationMessageStatus::Running);
    assert!(saved.questions.is_empty());
    assert!(saved.permissions.is_empty());

    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&admitted)),
        &[],
        Some(&execution),
    ));
    let live = projection.read();
    assert_eq!(live.messages[0].status, ConversationMessageStatus::Running);
    // What waits is the live owner's to list (`agent_question`): the fold
    // lists no question.
    assert!(live.questions.is_empty());

    assert!(projection.replace_committed(
        &committed("incarnation", 3, 3, 3, Some(&admitted)),
        &[],
        None,
    ));
    let gap = projection.read();
    assert_eq!(gap.messages[0].status, ConversationMessageStatus::Running);
    assert!(gap.questions.is_empty());
    assert!(gap.permissions.is_empty());

    admitted.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    assert!(projection.replace_committed(
        &committed("incarnation", 4, 4, 4, Some(&admitted)),
        &[],
        None,
    ));
    let done = projection.read();
    assert_eq!(
        done.messages[0].status,
        ConversationMessageStatus::Completed
    );
    assert!(done.questions.is_empty());
    assert!(done.permissions.is_empty());
}

/// A restart that is still pending reads as queued, because a pending id
/// whose message is not queued is refused by the client. Leaving the queue
/// without becoming active is unresolved again.
#[test]
fn a_restarted_turn_still_in_the_queue_is_queued_then_unresolved() {
    let snapshot = review_snapshot(vec![asked("execution", "1")]);
    let mut projection = opened_on(&snapshot);
    let id = ExecutionId::new("execution").unwrap();
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        std::slice::from_ref(&id),
        None,
    ));
    let queued = projection.read();
    assert_eq!(queued.messages[0].status, ConversationMessageStatus::Queued);
    assert!(queued.questions.is_empty());
    assert_eq!(queued.pending.len(), 1);

    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&snapshot)),
        &[],
        None,
    ));
    let left = projection.read();
    assert_eq!(
        left.messages[0].status,
        ConversationMessageStatus::Unresolved
    );
    assert!(left.questions.is_empty());
    assert!(left.pending.is_empty());
}

#[test]
fn a_queued_turn_admitted_after_open_is_queued_then_running() {
    let snapshot = review_snapshot(Vec::new());
    let mut projection = opened_on(&empty_history());
    let id = ExecutionId::new("execution").unwrap();
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        std::slice::from_ref(&id),
        None,
    ));
    assert_eq!(
        projection.read().messages[0].status,
        ConversationMessageStatus::Queued
    );
    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&snapshot)),
        &[],
        None,
    ));
    let view = projection.read();
    assert_eq!(view.messages[0].status, ConversationMessageStatus::Running);
    assert!(view.questions.is_empty());
    assert!(view.pending.is_empty());
}

#[test]
fn a_pending_tool_does_not_make_an_admitted_turn_unresolved() {
    let snapshot = review_snapshot(vec![event(ExecutionUpdate::Tool(ToolCallUpdate::new(
        ToolCallId::new("shell").unwrap(),
        Some("Shell".into()),
        None,
        None,
        None,
        None,
    )))]);
    let mut projection = opened_on(&empty_history());
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        None,
    ));
    let view = projection.read();
    assert_eq!(view.messages[0].status, ConversationMessageStatus::Running);
    assert_eq!(view.tools[0].status, "pending");
}

#[test]
fn a_committed_result_is_the_turn_status_while_nothing_is_active() {
    let cases = [
        (
            Some(Ok(ExecutionOutcome::Completed)),
            ConversationMessageStatus::Completed,
        ),
        (
            Some(Ok(ExecutionOutcome::Cancelled)),
            ConversationMessageStatus::Cancelled,
        ),
        (
            Some(Err(AgentError::AuditFailure)),
            ConversationMessageStatus::Failed,
        ),
    ];
    for (result, status) in cases {
        let mut snapshot = review_snapshot(vec![asked("execution", "1"), review("{}".into())]);
        snapshot.invocations[0].result = result;
        let mut projection = opened_on(&empty_history());
        assert!(projection.replace_committed(
            &committed("incarnation", 1, 1, 1, Some(&snapshot)),
            &[],
            None,
        ));
        let view = projection.read();
        assert_eq!(view.messages[0].status, status);
        assert!(view.questions.is_empty());
        assert!(view.permissions.is_empty());
    }
}

fn scheduled(id: &str, stage: InvocationStage, target: Option<&str>) -> InvocationRecord {
    let mut record = review_snapshot(Vec::new()).invocations.remove(0);
    record.request.execution_id = ExecutionId::new(id).unwrap();
    let target_id = target.map(|value| ExecutionId::new(value).unwrap());
    let kind = if target_id.is_some() {
        record.submission = InvocationSubmissionMode::Steering;
        record.target_event_offset = Some(0);
        InvocationKind::Steering
    } else {
        record.submission = InvocationSubmissionMode::Queued;
        InvocationKind::Queued
    };
    let actor = record.actor.clone();
    record.scheduling = vec![
        InvocationSchedulingEvent {
            kind,
            target: target_id.clone(),
            before: None,
            stage: InvocationStage::Queued,
            cause: SchedulingCause::Submitted,
            actor: Some(actor.clone()),
        },
        InvocationSchedulingEvent {
            kind,
            target: target_id,
            before: Some(InvocationStage::Queued),
            stage,
            cause: if stage == InvocationStage::Injected {
                SchedulingCause::SteeringInjected
            } else {
                SchedulingCause::Withdrawn
            },
            actor: (stage != InvocationStage::Injected).then_some(actor),
        },
    ];
    record
}

/// Injected and cancelled are the last stage, on a restart and on a later
/// admission. Neither gap around active turns them into unresolved or running.
#[test]
fn an_injected_or_cancelled_turn_keeps_that_status() {
    let stages = [
        (
            InvocationStage::Injected,
            ConversationMessageStatus::Injected,
        ),
        (
            InvocationStage::Cancelled,
            ConversationMessageStatus::Cancelled,
        ),
    ];
    for (stage, status) in stages {
        let target = (stage == InvocationStage::Injected).then_some("active-turn");
        let record = scheduled("execution", stage, target);
        let mut opened = empty_history();
        if target.is_some() {
            let mut preceding = review_snapshot(Vec::new()).invocations.remove(0);
            preceding.request.execution_id = ExecutionId::new("active-turn").unwrap();
            opened.invocations.push(preceding);
        }
        opened.invocations.push(record.clone());
        let restored = opened_on(&opened);
        let view = restored.read();
        assert_eq!(status_of(&view, "execution"), status);
        assert!(view.questions.is_empty());
        let message = view
            .messages
            .iter()
            .find(|message| message.execution_id == "execution")
            .expect("message");
        if stage == InvocationStage::Injected {
            assert_eq!(message.steering_target.as_deref(), target);
            assert_eq!(message.steering_offset, Some(0));
        }

        let mut admitted = empty_history();
        admitted.invocations.clone_from(&opened.invocations);
        let mut projection = opened_on(&empty_history());
        assert!(projection.replace_committed(
            &committed("incarnation", 1, 1, 1, Some(&admitted)),
            &[],
            None,
        ));
        let later = projection.read();
        assert_eq!(status_of(&later, "execution"), status);
        assert!(later.questions.is_empty());
    }
}

/// The restart set drops only the id this process is running. The other
/// restored turn stays unresolved, including after the active one is omitted.
#[test]
fn running_one_restored_turn_leaves_the_other_unresolved() {
    let mut snapshot = empty_history();
    snapshot.invocations.push(
        review_snapshot(vec![asked("kept", "1")])
            .invocations
            .remove(0),
    );
    snapshot.invocations[0].request.execution_id = ExecutionId::new("kept").unwrap();
    snapshot.invocations.push(
        review_snapshot(vec![asked("other", "2")])
            .invocations
            .remove(0),
    );
    snapshot.invocations[1].request.execution_id = ExecutionId::new("other").unwrap();
    let mut projection = opened_on(&snapshot);
    let kept = ExecutionId::new("kept").unwrap();
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&kept),
    ));
    let live = projection.read();
    assert_eq!(status_of(&live, "kept"), ConversationMessageStatus::Running);
    assert_eq!(
        status_of(&live, "other"),
        ConversationMessageStatus::Unresolved
    );
    // What waits is the live owner's to list (`agent_question`): the fold
    // lists no question.
    assert!(live.questions.is_empty());

    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&snapshot)),
        &[],
        None,
    ));
    let gap = projection.read();
    assert_eq!(status_of(&gap, "kept"), ConversationMessageStatus::Running);
    assert_eq!(
        status_of(&gap, "other"),
        ConversationMessageStatus::Unresolved
    );
    assert!(gap.questions.is_empty());
}

/// A restored turn can finish between reads, so its result arrives while it
/// is still in the restart set and nothing is active. The result is the status.
#[test]
fn a_restored_turn_takes_its_result_without_being_passed_as_active() {
    let cases = [
        (
            Some(Ok(ExecutionOutcome::Completed)),
            ConversationMessageStatus::Completed,
        ),
        (
            Some(Ok(ExecutionOutcome::Cancelled)),
            ConversationMessageStatus::Cancelled,
        ),
        (
            Some(Err(AgentError::AuditFailure)),
            ConversationMessageStatus::Failed,
        ),
    ];
    for (result, status) in cases {
        let open = review_snapshot(vec![asked("execution", "1"), review("{}".into())]);
        let mut projection = opened_on(&open);
        assert_eq!(
            projection.read().messages[0].status,
            ConversationMessageStatus::Unresolved
        );
        let mut finished = open.clone();
        finished.invocations[0].result = result;
        assert!(projection.replace_committed(
            &committed("incarnation", 1, 1, 1, Some(&finished)),
            &[],
            None,
        ));
        let view = projection.read();
        assert_eq!(view.messages[0].status, status);
        assert!(view.questions.is_empty());
        assert!(view.permissions.is_empty());
    }
}

#[test]
fn a_restarted_turn_stays_unresolved_beside_one_admitted_later() {
    let restarted = review_snapshot(vec![asked("execution", "1")]);
    let mut projection = opened_on(&restarted);
    assert_eq!(
        projection.read().messages[0].status,
        ConversationMessageStatus::Unresolved
    );
    assert!(projection.read().questions.is_empty());
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&restarted)),
        &[],
        None,
    ));
    assert_eq!(
        projection.read().messages[0].status,
        ConversationMessageStatus::Unresolved
    );

    let mut both = restarted.clone();
    let mut later = review_snapshot(Vec::new()).invocations.remove(0);
    later.request.execution_id = ExecutionId::new("later").unwrap();
    both.invocations.push(later);
    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&both)),
        &[],
        None,
    ));
    let view = projection.read();
    assert_eq!(
        status_of(&view, "execution"),
        ConversationMessageStatus::Unresolved
    );
    assert_eq!(
        status_of(&view, "later"),
        ConversationMessageStatus::Running
    );
    assert!(view.questions.is_empty());
}

/// A turn the first fold called a restart, which this process then runs,
/// stays running after it stops being active and before its result is saved.
#[test]
fn a_turn_this_process_runs_stays_running_after_it_stops_being_active() {
    let snapshot = review_snapshot(vec![asked("execution", "1"), review("{}".into())]);
    let mut projection = projection();
    let execution = ExecutionId::new("execution").unwrap();
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        None,
    ));
    assert_eq!(
        projection.read().messages[0].status,
        ConversationMessageStatus::Unresolved
    );

    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&snapshot)),
        &[],
        Some(&execution),
    ));
    let live = projection.read();
    assert_eq!(live.messages[0].status, ConversationMessageStatus::Running);
    // What waits is the live owner's to list (`agent_question`): the fold
    // lists no question.
    assert!(live.questions.is_empty());

    assert!(projection.replace_committed(
        &committed("incarnation", 3, 3, 3, Some(&snapshot)),
        &[],
        None,
    ));
    let gap = projection.read();
    assert_eq!(gap.messages[0].status, ConversationMessageStatus::Running);
    assert!(gap.questions.is_empty());
    assert!(gap.permissions.is_empty());
}

#[test]
fn the_first_snapshot_folded_into_an_empty_projection_is_a_restart() {
    let mut projection = projection();
    let snapshot = review_snapshot(vec![asked("execution", "1")]);
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        None,
    ));
    let view = projection.read();
    assert_eq!(
        view.messages[0].status,
        ConversationMessageStatus::Unresolved
    );
    assert!(view.questions.is_empty());

    let mut both = snapshot.clone();
    let mut later = review_snapshot(Vec::new()).invocations.remove(0);
    later.request.execution_id = ExecutionId::new("later").unwrap();
    both.invocations.push(later);
    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&both)),
        &[],
        None,
    ));
    let view = projection.read();
    assert_eq!(
        status_of(&view, "execution"),
        ConversationMessageStatus::Unresolved
    );
    assert_eq!(
        status_of(&view, "later"),
        ConversationMessageStatus::Running
    );
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

/// A reader's receiver let go starts over from the beginning, so its next
/// read is behind the view a longer read already gave, and is refused. The
/// head it saw past that view's is not: the view is behind the history now,
/// and says so, so its reader reads on instead of waiting for a commit that
/// already happened. A refused read that saw no newer head changes nothing
/// (`older_committed_read_cannot_replace_a_newer_terminal_view`).
#[test]
fn a_read_behind_the_view_that_saw_a_newer_head_marks_it_stale() {
    let mut projection = projection();
    let newer = completed_snapshot("execution", Vec::new());
    assert!(projection.replace_committed(
        &committed("incarnation", 3, 3, 3, Some(&newer)),
        &[],
        None
    ));
    projection.transcript_state(ConversationTranscriptState::Complete);
    let shown = projection.read();

    let older = completed_snapshot("earlier", Vec::new());
    assert!(!projection.replace_committed(
        &committed("incarnation", 1, 1, 4, Some(&older)),
        &[],
        None
    ));
    let behind = projection.read();
    assert_eq!(
        behind.transcript_state,
        ConversationTranscriptState::Stale,
        "a head past the view's was seen"
    );
    assert_eq!(
        serde_json::to_value(&behind.messages).unwrap(),
        serde_json::to_value(&shown.messages).unwrap(),
        "the newer view is kept"
    );
}

/// Options that each decide this one request.
fn once_options() -> PermissionOptions {
    PermissionOptions::new(
        vec![PermissionOption::new(
            PermissionOptionId::new("allow").unwrap(),
            "Allow once",
            PermissionDecision::new(PermissionEffect::Allow, PermissionScope::request()),
        )
        .unwrap()],
        &PermissionOfferPolicy::once_only(),
    )
    .unwrap()
}

/// A review as its live owner holds it while it waits.
fn pending_review(
    options: PermissionOptions,
    arguments: &str,
    deadline: Option<PermissionReviewDeadline>,
) -> PendingReview {
    PendingReview {
        execution_id: ExecutionId::new("execution").unwrap(),
        id: PermissionId::new("7").unwrap(),
        tool_id: ToolCallId::new("tool").unwrap(),
        options,
        content: std::sync::Arc::new(ReviewContent::new(
            None,
            "write_file".into(),
            arguments.into(),
            deadline,
        )),
    }
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
    // Its allow would read as one for this request alone: no choice is offered.
    assert!(agent_review(&pending_review(options, "{}", None)).is_none());
}

// What a review shows comes from its live owner, never from the folded
// history: a long history of reviews costs the view nothing.
#[test]
fn a_folded_history_lists_no_reviews() {
    let snapshot = review_snapshot(
        (0..MAX_OPEN_REVIEWS * 4)
            .map(|index| {
                let ExecutionUpdate::PermissionRequested {
                    observation,
                    input,
                    options,
                    ..
                } = review("{}".into()).update().clone()
                else {
                    unreachable!()
                };
                event(ExecutionUpdate::PermissionRequested {
                    id: PermissionId::new(index.to_string()).unwrap(),
                    tool_id: ToolCallId::new(format!("tool-{index}")).unwrap(),
                    observation,
                    input,
                    options,
                    deadline: None,
                })
            })
            .collect(),
    );
    let mut projection = projection();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&ExecutionId::new("execution").unwrap()),
    );
    projection.transcript_state(ConversationTranscriptState::Complete);
    let view = projection.read();
    assert!(view.permissions.is_empty());
    assert_eq!(view.interaction_view_error, None);
    assert!(!view.truncated);
}

// A review's input is never cut: its whole is read from the live owner.
#[test]
fn a_review_of_a_large_write_carries_its_whole_input() {
    let arguments =
        serde_json::json!({"file_path": "/tmp/big.txt", "content": "x".repeat(100_000)})
            .to_string();
    let shown = agent_review(&pending_review(once_options(), &arguments, None)).unwrap();
    assert_eq!(shown.arguments_json, arguments);
    assert_eq!(shown.tool_name, "write_file");
    assert_eq!(shown.permission_id, "7");
}

// An entry is small and of one size whatever its tool is named, so every
// waiting review fits in the room the view keeps for them.
#[test]
fn a_review_entry_clips_its_tool_name_to_a_fixed_encoded_size() {
    for name in ["\"".repeat(500), "\u{1}".repeat(500), "é".repeat(500)] {
        let entry = review_entry(
            "0123456789abcdef0123456789abcdef",
            ConversationReviewKind::App,
            &name,
        );
        let encoded = serde_json::to_string(&entry.tool_name).unwrap();
        assert!(encoded.len() - 2 <= MAX_REVIEW_ENTRY_TOOL_NAME_BYTES);
        assert!(name.starts_with(&entry.tool_name));
        assert!(serde_json::to_vec(&entry).unwrap().len() < MAX_REVIEW_ENTRY_BYTES);
    }
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
    let shown = agent_review(&pending_review(options, "{}", None)).unwrap();
    let offered: Vec<_> = shown
        .options
        .iter()
        .map(|option| (option.id.as_str(), option.effect))
        .collect();
    // On the wire as the schema names it.
    let wire = serde_json::to_value(&shown.options).unwrap();
    assert_eq!(wire[0]["effect"], "deny");
    assert_eq!(wire[1]["effect"], "allow");
    // The agent's review asks a tool, and says so (D19 on #390).
    assert_eq!(shown.ask, ConversationPermissionAsk::Tool);
    assert_eq!(serde_json::to_value(&shown).unwrap()["ask"], "tool");
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
    assert_eq!(
        projection.read().messages[0].status,
        ConversationMessageStatus::Unresolved
    );
    assert!(projection.read().questions.is_empty());
    assert!(projection.read().permissions.is_empty());
    let exact = ExecutionId::new("execution").unwrap();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&exact),
    );
    projection.transcript_state(ConversationTranscriptState::Complete);
    // What waits is the live owner's to list (`agent_question`): the fold
    // lists no question.
    assert!(projection.read().questions.is_empty());
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

/// The turn running here is listed however many turns are queued after it:
/// it is the one a person stops, by its id, and a queue longer than the window
/// would otherwise hide it, so nothing could stop it (found by the conversation
/// chaos check). The window's oldest other turn gives way instead, and the
/// view stays within its size bound with that turn's message as large as one
/// may be.
#[test]
fn the_running_turn_stays_listed_past_the_recent_message_window() {
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
        // Each as long as a queued message may be, so the bound below has to
        // give messages up.
        record.request.user_message = said(&"q".repeat(ExecutionRequest::MAX_MESSAGE_BYTES));
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
    assert_eq!(
        status_of(&view, "execution"),
        ConversationMessageStatus::Running
    );
    assert!(view.messages.len() <= 24);
    assert!(view
        .messages
        .iter()
        .all(|message| message.execution_id != "queued-1"));
    assert_eq!(
        status_of(&view, "queued-24"),
        ConversationMessageStatus::Queued
    );
    // What waits is the live owner's to list (`agent_question`): the fold
    // lists no question.
    assert!(view.questions.is_empty());
    let view = bound_view(view);
    assert!(serde_json::to_vec(&view).unwrap().len() <= 60_000);
    assert!(view.truncated);
    assert_eq!(
        status_of(&view, "execution"),
        ConversationMessageStatus::Running
    );
    assert!(projection.replace_committed(&current, &order, None));
    assert!(projection.read().questions.is_empty());
    assert!(projection.read().permissions.is_empty());
    // Reviews are not part of the folded view; their live owner lists them.
    let view = projection.read();
    assert!(view.permissions.is_empty());
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
            None,
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
}

/// A question as large as the SDK admits one: its message, and every
/// question, header and option as long as each may be, in characters that
/// take the most an encoding can make of them.
fn largest_question(index: usize) -> AgentQuestion {
    AgentQuestion::new(
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
    .unwrap()
}

// All a question asks is read from its live owner, never cut: however
// large, its entry is small and of one size, and what it shows is whole.
#[test]
fn the_largest_question_is_listed_by_a_small_entry_and_shown_whole() {
    let execution = ExecutionId::new("execution").unwrap();
    let mut keys = std::collections::HashSet::new();
    for index in 0..MAX_OPEN_QUESTIONS {
        let question = largest_question(index);
        let pending = PendingQuestion::new(
            execution.clone(),
            QuestionId::new(format!("custom-{index}")).unwrap(),
            question.clone(),
        );
        let key = pending.key();
        assert!(keys.insert(key.clone()));
        let shown = agent_question(&pending);
        assert_eq!(shown.question, key);
        assert_eq!(shown.question_id, format!("custom-{index}"));
        assert_eq!(shown.message, question.message());
        assert_eq!(shown.questions.len(), 3);
        for (sibling, asked) in shown.questions.iter().enumerate() {
            assert_eq!(asked.key, format!("custom_{index}_{sibling}"));
            assert_eq!(asked.prompt, "\"".repeat(1024));
            assert_eq!(asked.header.as_deref(), Some(&*"\\".repeat(256)));
            assert_eq!(asked.options[0].label, "\"".repeat(1024));
        }
        let entry = question_entry(&key, question.message());
        assert_eq!(entry.question, key);
        assert!(question.message().starts_with(&entry.message));
        let encoded = serde_json::to_string(&entry.message).unwrap();
        assert!(encoded.len() - 2 <= MAX_QUESTION_ENTRY_MESSAGE_BYTES);
        assert!(serde_json::to_vec(&entry).unwrap().len() < MAX_QUESTION_ENTRY_BYTES);
    }
}

#[test]
fn a_question_entry_clips_its_message_to_a_fixed_encoded_size() {
    for message in [
        "\"".repeat(500),
        "\u{1}".repeat(500),
        "é".repeat(500),
        "x".to_owned(),
    ] {
        let entry = question_entry("0123456789abcdef0123456789abcdef", &message);
        let encoded = serde_json::to_string(&entry.message).unwrap();
        assert!(encoded.len() - 2 <= MAX_QUESTION_ENTRY_MESSAGE_BYTES);
        assert!(!entry.message.is_empty());
        assert!(message.starts_with(&entry.message));
        assert!(serde_json::to_vec(&entry).unwrap().len() < MAX_QUESTION_ENTRY_BYTES);
    }
}

// The questions and reviews a view lists keep their room however full the
// rest of it is: every one of the most it lists, each at its largest.
#[test]
fn a_bounded_view_keeps_every_question_and_review_it_lists() {
    let mut projection = projection();
    let snapshot = review_snapshot(Vec::new());
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&ExecutionId::new("execution").unwrap()),
    );
    projection.transcript_state(ConversationTranscriptState::Complete);
    let mut view = projection.read();
    let message = view.messages[0].clone();
    view.messages = (0..40)
        .map(|_| crate::conversation::view::ConversationMessage {
            user_text: "y".repeat(8_000),
            ..message.clone()
        })
        .collect();
    view.messages.push(message);
    view.title = Some("t".repeat(512));
    // The current selection and pending change remain beside the full keyed
    // lists, with the largest encoded identities their admission owners allow.
    let model = ConversationModelId::new("\"".repeat(ConversationModelId::MAX_BYTES)).unwrap();
    let actor = ActionContext::new(
        "person",
        "panel",
        "\u{1}".repeat(ActionContext::MAX_IDENTITY_BYTES),
    )
    .unwrap();
    view.selection = Some(ConversationSelectionView {
        agent: AgentId::Claude,
        model: model.as_str().to_owned(),
        approval_mode: ConversationApprovalMode::Full,
    });
    view.model_change = Some(ConversationModelChangeView {
        request_id: actor.request_id().to_owned(),
        requested_model: model.as_str().to_owned(),
        status: ConversationModelChangeStatus::RecoveryRequired,
    });
    view.model_change_effect = Some(ConversationModelChangeEffect::SubsequentResponses);
    let selection = view.selection.clone();
    let model_change = serde_json::to_value(&view.model_change).unwrap();
    let model_effect = view.model_change_effect;
    view.permissions = (0..MAX_LISTED_REVIEWS)
        .map(|index| {
            review_entry(
                &format!("{index:032x}"),
                ConversationReviewKind::App,
                &"\u{1}".repeat(1_000),
            )
        })
        .collect();
    view.questions = (0..MAX_OPEN_QUESTIONS)
        .map(|index| question_entry(&format!("{index:032x}"), &"\u{1}".repeat(1_000)))
        .collect();
    let (permissions, questions) = (view.permissions.clone(), view.questions.clone());
    assert!(serde_json::to_vec(&view).unwrap().len() > MAX_VIEW_BYTES);
    let view = bound_view(view);
    assert!(serde_json::to_vec(&view).unwrap().len() <= MAX_VIEW_BYTES);
    assert_eq!(view.permissions, permissions);
    assert_eq!(view.questions, questions);
    assert_eq!(view.selection, selection);
    assert_eq!(
        serde_json::to_value(&view.model_change).unwrap(),
        model_change
    );
    assert_eq!(view.model_change_effect, model_effect);
    assert_eq!(view.interaction_view_error, None);
    assert!(view.truncated);
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
            independent
                .as_ref()
                .map(|_| "The turn could not complete all required work.")
        );
        assert_eq!(
            view.messages[0].required_work_failed,
            independent.as_ref().map(|_| true)
        );
    }
}

#[test]
fn a_usage_limit_is_published_without_the_provider_failure_sentence() {
    let limit = nessa_sdk::application::agent_execution::agents::UsageLimit::timed_reset(
        UsageLimitProvider::Claude,
        UsageLimitKind::FiveHour,
        1_800_000_000_000,
        Some(ProviderDiagnostic::new("You've hit your limit")),
    );
    let report = ExecutionReport::new(
        Some(Err(AgentError::UsageLimitReached(limit))),
        None,
        ProviderSessionState::Usable,
    );
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations[0].result = Some(report.clone().into_result());
    snapshot.invocations[0].provider_report = Some(report);
    let message = &Projection::new(
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
    )
    .read()
    .messages[0];
    let usage = message.usage_limit.as_ref().expect("usage limit");
    assert_eq!(
        usage.provider,
        crate::conversation::view::ConversationUsageLimitProvider::Claude
    );
    assert_eq!(
        usage.recovery,
        crate::conversation::view::ConversationUsageLimitRecovery::TimedReset
    );
    assert_eq!(usage.resets_at_ms, Some(1_800_000_000_000));
    assert_eq!(usage.auto_resuming, None);
    assert_eq!(message.error, None);
    assert_eq!(message.required_work_failed, None);
    assert_eq!(message.status, ConversationMessageStatus::Failed);
}

#[test]
fn a_usage_limit_wrapped_as_its_own_observation_is_not_a_second_failure() {
    let reached = AgentError::UsageLimitReached(weekly_limit());
    let report = ExecutionReport::new(
        Some(Err(reached.clone())),
        None,
        ProviderSessionState::Usable,
    );
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations[0].result = Some(Err(AgentError::ExecutionObservation {
        error: Box::new(reached.clone()),
        execution_result: Some(Box::new(Err(reached))),
    }));
    snapshot.invocations[0].provider_report = Some(report);
    let message = &project_snapshot(&snapshot).messages[0];
    assert!(message.usage_limit.is_some());
    assert_eq!(message.error, None);
    assert_eq!(message.required_work_failed, None);
}

#[test]
fn a_usage_limit_keeps_a_different_failure_beside_it() {
    let reached = AgentError::UsageLimitReached(weekly_limit());
    let report = ExecutionReport::new(
        Some(Err(reached.clone())),
        None,
        ProviderSessionState::Usable,
    );
    for result in [
        AgentError::ExecutionObservation {
            error: Box::new(AgentError::Protocol(
                "terminal observation contradicts execution settlement".into(),
            )),
            execution_result: Some(Box::new(Err(reached.clone()))),
        },
        AgentError::StorageAfterExecution {
            error: StorageError::Busy,
            execution_result: Box::new(Err(reached.clone())),
        },
    ] {
        let mut snapshot = review_snapshot(Vec::new());
        snapshot.invocations[0].result = Some(Err(result));
        snapshot.invocations[0].provider_report = Some(report.clone());
        let message = &project_snapshot(&snapshot).messages[0];
        assert!(message.usage_limit.is_some());
        assert_eq!(
            message.error.as_deref(),
            Some("The turn could not complete all required work.")
        );
        assert_eq!(message.required_work_failed, Some(true));
    }
}

#[test]
fn a_usage_limit_in_a_later_aggregate_error_is_published_without_a_provider_report() {
    let reached = AgentError::UsageLimitReached(weekly_limit());
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations[0].result = Some(Err(AgentError::MultipleOperationFailures {
        first_error: Box::new(AgentError::Protocol("provider had no report".into())),
        subsequent_error: Box::new(reached),
    }));
    snapshot.invocations[0].provider_report = None;
    let message = &project_snapshot(&snapshot).messages[0];
    let usage = message.usage_limit.as_ref().expect("usage limit");
    assert_eq!(
        usage.provider,
        crate::conversation::view::ConversationUsageLimitProvider::Claude
    );
    assert_eq!(message.required_work_failed, Some(true));
}

fn weekly_limit() -> nessa_sdk::application::agent_execution::agents::UsageLimit {
    nessa_sdk::application::agent_execution::agents::UsageLimit::unknown(
        UsageLimitProvider::Claude,
        UsageLimitKind::Unspecified,
        Some(ProviderDiagnostic::new("You've hit your weekly limit")),
    )
}

fn project_snapshot(snapshot: &SessionSnapshot) -> ConversationView {
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
        Some(snapshot),
    )
    .read()
}

fn shown(events: Vec<ExecutionEvent>) -> ConversationView {
    let snapshot = review_snapshot(events);
    let mut projection = projection();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&ExecutionId::new("execution").unwrap()),
    );
    projection.read()
}

fn refusal(tool: &str) -> DeclinedReview {
    DeclinedReview::selected(
        ReviewDeclineId::new("1").unwrap(),
        ReviewDecline::new(
            Some(tool),
            ReviewDeclineReason::ToolNotReviewable,
            RefusalOutcome::Continue,
        ),
        None,
    )
    .unwrap()
}

fn declined(review: DeclinedReview) -> ExecutionEvent {
    event(ExecutionUpdate::ReviewDeclined(review))
}

fn stopping_rule() -> ApprovalRuleReference {
    ApprovalRuleReference::new(
        "refuse-shell",
        "3",
        ActionContext::new("host", "policy", "grant").unwrap(),
    )
    .unwrap()
}

fn stopped_review(outcome: RefusalOutcome) -> DeclinedReview {
    DeclinedReview::selected(
        ReviewDeclineId::new("4").unwrap(),
        ReviewDecline::new(
            Some("Bash"),
            ReviewDeclineReason::ToolNotReviewable,
            outcome,
        ),
        Some(stopping_rule()),
    )
    .unwrap()
}

fn local_notice(
    message: &crate::conversation::view::ConversationMessage,
) -> &crate::conversation::view::ConversationPart {
    message
        .parts
        .iter()
        .find(|part| part.kind == "local_notice")
        .expect("the turn has a notice")
}

#[test]
fn a_stopping_refusal_says_why_the_turn_stopped() {
    let ended = stopped_review(RefusalOutcome::EndTurn);
    let mut snapshot = review_snapshot(vec![
        declined(ended),
        event(ExecutionUpdate::Message(MessageChunk::text(
            "still talking",
        ))),
        event(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)),
    ]);
    snapshot.invocations[0].result = Some(Ok(ExecutionOutcome::Cancelled));
    let mut projection = projection();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        Some(&ExecutionId::new("execution").unwrap()),
    );
    let view = projection.read();
    let message = &view.messages[0];
    assert_eq!(message.status, ConversationMessageStatus::Cancelled);
    let notice = local_notice(message);
    assert_eq!(
        notice.text,
        "Nessa stopped this turn before Bash ran because rule refuse-shell (revision 3) refused it. That tool is not reviewable here."
    );
    assert_eq!(notice.notice_id, "4");
    assert_eq!(notice.notice_kind, "review");
    let closed = shown(vec![declined(
        DeclinedReview::selected(
            ReviewDeclineId::new("8").unwrap(),
            ReviewDecline::new(
                None,
                ReviewDeclineReason::UnusableOptions,
                RefusalOutcome::CloseSession,
            ),
            Some(stopping_rule()),
        )
        .unwrap(),
    )]);
    assert_eq!(
        local_notice(&closed.messages[0]).text,
        "Nessa is closing the session before the requested tool ran because rule refuse-shell (revision 3) refused it. The request offered no usable choices."
    );
}

#[test]
fn a_hook_stop_names_the_tool_and_the_hook_reason() {
    let session = ExecutionSessionId::new("provider-session").unwrap();
    let execution = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("7").unwrap();
    let reason = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::hook_refusal_for_test(
            &session,
            &execution,
            &permission,
            RefusalOutcome::EndTurn,
            "shell is blocked",
        )
        .unwrap(),
    );
    let origin = CancellationOrigin::Client(
        ActionContext::new("permission-review-hook", "0", permission.as_str()).unwrap(),
    );
    let view = shown(vec![
        review("{}".into()),
        cancelled_review(reason.clone(), origin.clone()),
        event(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)),
    ]);
    let unfinished = shown(vec![
        review("{}".into()),
        cancelled_review(reason.clone(), origin.clone()),
    ]);
    assert!(unfinished.messages[0]
        .parts
        .iter()
        .all(|part| part.kind != "local_notice"));
    let completed = shown(vec![
        review("{}".into()),
        cancelled_review(reason, origin.clone()),
        event(ExecutionUpdate::Finished(ExecutionOutcome::Completed)),
    ]);
    assert!(completed.messages[0]
        .parts
        .iter()
        .all(|part| part.kind != "local_notice"));
    let notice = local_notice(&view.messages[0]);
    assert_eq!(
        notice.text,
        "Nessa stopped this turn before write_file ran because a hook refused it: shell is blocked."
    );
    assert_eq!(notice.notice_id, "7");
    assert_eq!(notice.notice_kind, "hook");
    assert!(!notice.text.contains("{"));

    let continued = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::hook_refusal_for_test(
            &session,
            &execution,
            &permission,
            RefusalOutcome::Continue,
            "not this tool",
        )
        .unwrap(),
    );
    let quiet = shown(vec![
        review("{}".into()),
        cancelled_review(continued, origin.clone()),
    ]);
    assert!(quiet.messages[0]
        .parts
        .iter()
        .all(|part| part.kind != "local_notice"));

    let unnamed = PermissionId::new("not-a-notice").unwrap();
    let hidden = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::hook_refusal_for_test(
            &session,
            &execution,
            &unnamed,
            RefusalOutcome::CloseSession,
            "close it",
        )
        .unwrap(),
    );
    let ExecutionUpdate::PermissionRequested {
        tool_id,
        observation,
        input,
        options,
        ..
    } = review("{}".into()).update().clone()
    else {
        unreachable!()
    };
    let requested = event(ExecutionUpdate::PermissionRequested {
        id: unnamed.clone(),
        tool_id: tool_id.clone(),
        observation: observation.clone(),
        input: input.clone(),
        options: options.clone(),
        deadline: None,
    });
    let mut owner = ExecutionController::new(session);
    owner.begin_execution(execution.clone()).unwrap();
    owner
        .request_permission(
            &execution,
            unnamed.clone(),
            ToolCallUpdate::new(tool_id, None, None, None, None, None),
            input,
            options,
            None,
        )
        .unwrap();
    let cancelled = owner
        .cancel_permission(
            &execution,
            &unnamed,
            hidden,
            CancellationOrigin::Client(
                ActionContext::new("permission-review-hook", "runtime", unnamed.as_str()).unwrap(),
            ),
        )
        .unwrap()
        .unwrap();
    let skipped = shown(vec![
        requested,
        event(ExecutionUpdate::PermissionCancelled(cancelled)),
    ]);
    assert!(skipped.messages[0]
        .parts
        .iter()
        .all(|part| part.kind != "local_notice"));
}

fn notices(
    message: &crate::conversation::view::ConversationMessage,
) -> Vec<&crate::conversation::view::ConversationPart> {
    message
        .parts
        .iter()
        .filter(|part| part.kind == "local_notice")
        .collect()
}

fn hook_cancellation(outcome: RefusalOutcome, explanation: &str) -> ExecutionEvent {
    let session = ExecutionSessionId::new("provider-session").unwrap();
    let execution = ExecutionId::new("execution").unwrap();
    let permission = PermissionId::new("7").unwrap();
    let reason = PermissionCancellationReason::custom(
        CustomPermissionCancellationReason::hook_refusal_for_test(
            &session,
            &execution,
            &permission,
            outcome,
            explanation,
        )
        .unwrap(),
    );
    cancelled_review(
        reason,
        CancellationOrigin::Client(
            ActionContext::new("permission-review-hook", "0", permission.as_str()).unwrap(),
        ),
    )
}

/// A declined review and a hook stop are different decisions. The same decimal
/// may name both, and each sentence stays. Admission accepts that history.
#[test]
fn a_hook_stop_stays_beside_a_declined_review_with_the_same_id() {
    let decline = DeclinedReview::selected(
        ReviewDeclineId::new("7").unwrap(),
        ReviewDecline::new(
            Some("Bash"),
            ReviewDeclineReason::ToolNotReviewable,
            RefusalOutcome::Continue,
        ),
        None,
    )
    .unwrap();
    let ended = shown(vec![
        declined(decline.clone()),
        review("{}".into()),
        hook_cancellation(RefusalOutcome::EndTurn, "shell is blocked"),
        event(ExecutionUpdate::Finished(ExecutionOutcome::Cancelled)),
    ]);
    let ended = notices(&ended.messages[0]);
    assert_eq!(ended.len(), 2);
    assert_eq!(ended[0].notice_kind, "review");
    assert_eq!(ended[0].notice_id, "7");
    assert_eq!(
        ended[0].text,
        "Nessa declined a tool review labeled Bash because that tool is not reviewable here. Nessa has not confirmed writing the refusal to the agent."
    );
    assert_eq!(ended[1].notice_kind, "hook");
    assert_eq!(ended[1].notice_id, "7");
    assert_eq!(
        ended[1].text,
        "Nessa stopped this turn before write_file ran because a hook refused it: shell is blocked."
    );

    let confirmed = decline.advance(ReviewDeclineStage::WriteConfirmed).unwrap();
    let closed = shown(vec![
        declined(decline),
        review("{}".into()),
        hook_cancellation(RefusalOutcome::CloseSession, "shell is blocked"),
        declined(confirmed),
    ]);
    let closed = notices(&closed.messages[0]);
    assert_eq!(closed.len(), 2);
    assert_eq!(closed[0].notice_kind, "review");
    assert_eq!(
        closed[0].text,
        "Nessa declined a tool review labeled Bash because that tool is not reviewable here."
    );
    assert_eq!(closed[1].notice_kind, "hook");
    assert_eq!(closed[1].notice_id, "7");
    assert_eq!(
        closed[1].text,
        "Nessa closed the session before write_file ran because a hook refused it: shell is blocked."
    );
}

#[test]
fn a_repeated_permission_id_is_still_one_review() {
    let snapshot = review_snapshot(vec![review("{}".into()), review("{}".into())]);
    let error = CommittedSession::new(
        SessionId::new("conversation").unwrap(),
        "incarnation".into(),
        1,
        1,
        1,
        Some(snapshot),
        CommittedStatus::new(CommittedCompleteness::Complete, CommittedFreshness::Current),
    )
    .expect_err("one permission id is one review");
    assert!(
        matches!(
            error,
            StorageError::Corrupt(ref message)
                if message.contains("permission identity is repeated")
        ),
        "{error:?}"
    );
}

/// A deadline settlement and a declined review are different decisions. The
/// same decimal may name both once admission treats the identities as
/// different, and each sentence stays.
#[test]
fn a_deadline_notice_stays_beside_a_declined_review_with_the_same_id() {
    use crate::product::generated::{ConversationNoticeKind, ConversationPart as WirePart};
    use nessa_sdk::domain::agent_execution::permissions::DeadlineReason;
    let decline = DeclinedReview::selected(
        ReviewDeclineId::new("7").unwrap(),
        ReviewDecline::new(
            Some("Bash"),
            ReviewDeclineReason::ToolNotReviewable,
            RefusalOutcome::Continue,
        ),
        None,
    )
    .unwrap();
    let view = shown(vec![
        review_with_deadline(PermissionEffect::Deny, STANDARD_DENY_REASON),
        event(ExecutionUpdate::DeadlineAnswered {
            id: PermissionId::new("7").unwrap(),
            effect: PermissionEffect::Deny,
            reason: DeadlineReason::new(STANDARD_DENY_REASON).unwrap(),
        }),
        declined(decline),
    ]);
    let shown_notices = notices(&view.messages[0]);
    assert_eq!(shown_notices.len(), 2);
    assert_eq!(shown_notices[0].notice_kind, "deadline");
    assert_eq!(shown_notices[0].notice_id, "7");
    assert_eq!(
        shown_notices[0].text,
        format!("Denied automatically: {STANDARD_DENY_REASON}")
    );
    assert_eq!(shown_notices[1].notice_kind, "review");
    assert_eq!(shown_notices[1].notice_id, "7");
    assert!(shown_notices[1].text.contains("not reviewable"));
    let wire = |part: &crate::conversation::view::ConversationPart| -> WirePart {
        serde_json::from_value(serde_json::to_value(part).unwrap()).unwrap()
    };
    assert_eq!(
        wire(shown_notices[0]).notice_kind,
        Some(ConversationNoticeKind::Deadline)
    );
    assert_eq!(
        wire(shown_notices[1]).notice_kind,
        Some(ConversationNoticeKind::Review)
    );
}

#[test]
fn a_notice_kind_survives_the_product_wire() {
    use crate::product::generated::{ConversationNoticeKind, ConversationPart as WirePart};
    let view = shown(vec![
        event(ExecutionUpdate::Message(MessageChunk::text("hello"))),
        declined(refusal("Bash")),
        review("{}".into()),
        hook_cancellation(RefusalOutcome::CloseSession, "shell is blocked"),
    ]);
    let wire = |part: &crate::conversation::view::ConversationPart| -> WirePart {
        serde_json::from_value(serde_json::to_value(part).unwrap()).unwrap()
    };
    let text = &view.messages[0].parts[0];
    assert_eq!(text.kind, "text");
    assert_eq!(wire(text).notice_kind, None);
    let shown_notices = notices(&view.messages[0]);
    assert_eq!(
        wire(shown_notices[0]).notice_kind,
        Some(ConversationNoticeKind::Review)
    );
    assert_eq!(
        wire(shown_notices[1]).notice_kind,
        Some(ConversationNoticeKind::Hook)
    );
}

fn text_len(message: &crate::conversation::view::ConversationMessage) -> usize {
    message.parts.iter().map(|part| part.text.len()).sum()
}

#[test]
fn the_text_budget_keeps_a_running_total() {
    let limit = MAX_TEXT * 2;
    let whole = "a".repeat(limit);
    let exact = shown(vec![event(ExecutionUpdate::Message(MessageChunk::text(
        whole.clone(),
    )))]);
    assert!(!exact.truncated);
    assert_eq!(exact.messages[0].parts[0].text, whole);
    assert_eq!(exact.messages[0].retained_text, limit);
    assert_eq!(
        exact.messages[0].retained_text,
        text_len(&exact.messages[0])
    );

    let over = shown(vec![event(ExecutionUpdate::Message(MessageChunk::text(
        format!("{whole}b"),
    )))]);
    assert!(over.truncated);
    assert_eq!(over.messages[0].parts[0].text.len(), limit);
    assert_eq!(over.messages[0].retained_text, text_len(&over.messages[0]));

    let emoji = shown(vec![
        event(ExecutionUpdate::Message(MessageChunk::text(
            "a".repeat(limit - 1),
        ))),
        event(ExecutionUpdate::Message(MessageChunk::text("😀"))),
    ]);
    assert!(emoji.truncated);
    assert_eq!(emoji.messages[0].parts[1].text, "");
    assert!(text_len(&emoji.messages[0]) <= limit);
    assert_eq!(
        emoji.messages[0].retained_text,
        text_len(&emoji.messages[0])
    );

    let selected = refusal("a");
    let selected_event = declined(selected.clone());
    let confirmed = declined(
        selected
            .advance(ReviewDeclineStage::WriteConfirmed)
            .unwrap(),
    );
    let selected_len = shown(vec![selected_event.clone()]).messages[0].parts[0]
        .text
        .len();
    let confirmed_len = shown(vec![selected_event.clone(), confirmed.clone()]).messages[0].parts[0]
        .text
        .len();
    assert!(confirmed_len < selected_len);
    let filler = "x".repeat(limit - selected_len);
    let shrunk = shown(vec![
        selected_event.clone(),
        event(ExecutionUpdate::Message(MessageChunk::text(filler))),
        confirmed.clone(),
    ]);
    assert_eq!(shrunk.messages[0].parts[0].text.len(), confirmed_len);
    assert_eq!(
        shrunk.messages[0].retained_text,
        limit - selected_len + confirmed_len
    );
    assert_eq!(
        shrunk.messages[0].retained_text,
        text_len(&shrunk.messages[0])
    );

    let room = 8;
    let unconfirmed = declined(
        selected
            .advance(ReviewDeclineStage::WriteUnconfirmed)
            .unwrap(),
    );
    let full_unconfirmed = shown(vec![selected_event.clone(), unconfirmed.clone()]).messages[0]
        .parts[0]
        .text
        .len();
    assert!(full_unconfirmed > room);
    let clipped_notice = shown(vec![
        event(ExecutionUpdate::Message(MessageChunk::text(
            "y".repeat(limit - room),
        ))),
        selected_event.clone(),
        unconfirmed,
    ]);
    assert_eq!(
        clipped_notice.messages[0].parts[1].text.len(),
        room,
        "a notice longer than the room the old one held was emptied"
    );
    assert!(!clipped_notice.messages[0].parts[1].text.is_empty());
    assert_eq!(clipped_notice.messages[0].retained_text, limit);
    assert_eq!(
        clipped_notice.messages[0].retained_text,
        text_len(&clipped_notice.messages[0])
    );

    let events = vec![
        selected_event,
        event(ExecutionUpdate::Message(MessageChunk::text("kept"))),
        confirmed,
    ];
    let snapshot = review_snapshot(events);
    let restored = Projection::new(
        "conversation".into(),
        projection().read().capabilities,
        Some(&snapshot),
    );
    let replaced_view = shown_snapshot(&snapshot);
    assert_eq!(
        restored.read().messages[0].parts,
        replaced_view.messages[0].parts
    );
    assert_eq!(
        restored.read().messages[0].retained_text,
        text_len(&restored.read().messages[0])
    );
    assert_eq!(
        replaced_view.messages[0].retained_text,
        restored.read().messages[0].retained_text
    );
    let wire = serde_json::to_value(&restored.read().messages[0]).unwrap();
    assert!(wire.get("retainedText").is_none());
    assert!(wire.get("retained_text").is_none());

    let mut again = projection();
    let first = review_snapshot(vec![event(ExecutionUpdate::Message(MessageChunk::text(
        "a".repeat(limit),
    )))]);
    again.record(&first.invocations[0], false);
    let second = review_snapshot(vec![event(ExecutionUpdate::Message(MessageChunk::text(
        "hi",
    )))]);
    again.record(&second.invocations[0], false);
    let rebuilt = again.read();
    assert_eq!(rebuilt.messages[0].parts[0].text, "hi");
    assert_eq!(rebuilt.messages[0].retained_text, 2);
    assert_eq!(
        rebuilt.messages[0].retained_text,
        text_len(&rebuilt.messages[0])
    );

    let kept = shown(vec![
        event(ExecutionUpdate::Message(MessageChunk::text("alpha"))),
        event(ExecutionUpdate::Message(MessageChunk::text("beta-beta"))),
    ]);
    assert_eq!(kept.messages.len(), 1);
    assert_eq!(kept.messages[0].parts.len(), 2);
    let full = serde_json::to_vec(&kept).unwrap().len();
    let trimmed = bound_view_within(kept.clone(), full - 1, true);
    assert!(
        trimmed.messages[0].parts.len() < kept.messages[0].parts.len(),
        "the display bound did not drop a part"
    );
    assert_eq!(
        trimmed.messages[0].retained_text,
        text_len(&trimmed.messages[0]),
        "a part the display bound removed was still counted"
    );
    assert!(trimmed.messages[0].retained_text < kept.messages[0].retained_text);
    assert_eq!(kept.messages[0].retained_text, text_len(&kept.messages[0]));

    let mut capped_events = Vec::new();
    for _ in 0..512 {
        capped_events.push(event(ExecutionUpdate::Message(MessageChunk::text("x"))));
    }
    capped_events.push(event(ExecutionUpdate::Message(MessageChunk::text("y"))));
    let capped = shown(capped_events);
    assert_eq!(capped.messages[0].parts.len(), 512);
    assert!(capped.truncated);
    assert!(capped.messages[0].parts.iter().all(|part| part.text == "x"));
    assert_eq!(capped.messages[0].retained_text, 512);
    assert_eq!(
        capped.messages[0].retained_text,
        text_len(&capped.messages[0]),
        "a part past the cap was still counted"
    );
}

fn shown_snapshot(snapshot: &SessionSnapshot) -> ConversationView {
    let mut projection = projection();
    projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(snapshot)),
        &[],
        Some(&ExecutionId::new("execution").unwrap()),
    );
    projection.read()
}

#[test]
fn startup_authentication_uses_primary_retained_cause_without_a_provider_report() {
    let auth = AgentError::AuthenticationRequired {
        diagnostic: Some(ProviderDiagnostic::new("Authentication required")),
    };
    let generic = AgentError::Provider {
        code: -32000,
        diagnostic: Some(ProviderDiagnostic::new("OAuth session expired")),
    };
    for (error, expected, notice) in [
        (auth.clone(), Some(true), None),
        (
            AgentError::MultipleOperationFailures {
                first_error: Box::new(auth.clone()),
                subsequent_error: Box::new(AgentError::AuditFailure),
            },
            Some(true),
            Some("The turn could not complete all required work."),
        ),
        (
            AgentError::OperationAndCleanupFailure {
                operation_error: Box::new(auth.clone()),
                cleanup_error: Box::new(AgentError::CleanupUncertain),
            },
            Some(true),
            Some("The turn could not complete all required work."),
        ),
        (
            AgentError::StorageAfterExecution {
                error: StorageError::Io("write failed".into()),
                execution_result: Box::new(Err(auth.clone())),
            },
            Some(true),
            Some("The turn could not complete all required work."),
        ),
        (
            generic.clone(),
            None,
            Some("The turn could not complete all required work."),
        ),
        (
            AgentError::MultipleOperationFailures {
                first_error: Box::new(generic),
                subsequent_error: Box::new(auth.clone()),
            },
            None,
            Some("The turn could not complete all required work."),
        ),
    ] {
        let mut snapshot = review_snapshot(Vec::new());
        snapshot.invocations[0].provider_report = None;
        snapshot.invocations[0].result = Some(Err(error));
        let restored = Projection::new(
            "conversation".into(),
            projection().read().capabilities,
            Some(&snapshot),
        )
        .read();
        let live = shown_snapshot(&snapshot);
        assert_eq!(live.messages[0].authentication_required, expected);
        assert_eq!(live.messages[0].error.as_deref(), notice);
        assert_eq!(
            live.messages[0].required_work_failed,
            (expected == Some(true) && notice.is_some()).then_some(true)
        );
        assert_eq!(restored.messages[0].authentication_required, expected);
        assert_eq!(restored.messages[0].error.as_deref(), notice);
        assert_eq!(
            restored.messages[0].required_work_failed,
            live.messages[0].required_work_failed
        );
    }
    // A real report remains authoritative even if a later local operation is auth-related.
    for provider_result in [
        Ok(ExecutionOutcome::Completed),
        Err(AgentError::Provider {
            code: -32000,
            diagnostic: None,
        }),
    ] {
        let mut snapshot = review_snapshot(Vec::new());
        snapshot.invocations[0].result = Some(Err(auth.clone()));
        snapshot.invocations[0].provider_report = Some(ExecutionReport::new(
            Some(provider_result),
            None,
            ProviderSessionState::Usable,
        ));
        assert_eq!(
            shown_snapshot(&snapshot).messages[0].authentication_required,
            None
        );
    }
}

mod artifacts {
    use super::*;
    use crate::conversation::projection::MAX_VIEW_ARTIFACTS;
    use crate::conversation::view::ConversationArtifact;
    use nessa_sdk::application::agent_execution::sessions::{
        ArtifactName, ArtifactRecord, PublishedFile,
    };
    use nessa_sdk::domain::agent_execution::leases::LeaseId;
    use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};

    fn artifact(index: usize, turn: Option<&str>) -> ArtifactRecord {
        ArtifactRecord {
            lease: LeaseId::new("lease-1").unwrap(),
            turn: turn.map(|turn| ExecutionId::new(turn).unwrap()),
            name: ArtifactName::new(format!("file-{index}.txt")).unwrap(),
            file: PublishedFile::new(
                Sha256Digest::from_bytes([1; 32]),
                MediaType::parse("text/plain").unwrap(),
                index as u64 + 1,
            )
            .unwrap(),
            actor: ActionContext::new("person", "panel", "send").unwrap(),
        }
    }
    fn holding(artifacts: Vec<ArtifactRecord>) -> SessionSnapshot {
        let mut snapshot = completed_snapshot("execution", Vec::new());
        snapshot.artifacts = artifacts;
        snapshot
    }
    fn names(view: &ConversationView) -> Vec<String> {
        view.artifacts.iter().map(|a| a.name.clone()).collect()
    }

    #[test]
    fn the_view_lists_each_artifact_as_recorded_in_camel_case() {
        let view = shown_snapshot(&holding(vec![
            artifact(0, Some("execution")),
            artifact(1, None),
        ]));
        assert_eq!(
            view.artifacts[0],
            ConversationArtifact {
                execution_id: Some("execution".into()),
                lease: "lease-1".into(),
                name: "file-0.txt".into(),
                digest: format!("sha256:{}", "01".repeat(32)),
                mime_type: "text/plain".into(),
                size: 1,
            }
        );
        let wire = serde_json::to_value(&view).unwrap();
        assert_eq!(
            wire["artifacts"][0],
            serde_json::json!({
                "executionId": "execution",
                "lease": "lease-1",
                "name": "file-0.txt",
                "digest": format!("sha256:{}", "01".repeat(32)),
                "mimeType": "text/plain",
                "size": 1,
            })
        );
        // No turn was running: the field is left out, not null.
        assert!(wire["artifacts"][1].get("executionId").is_none());
        assert_eq!(
            serde_json::to_value(shown_snapshot(&holding(Vec::new()))).unwrap()["artifacts"],
            serde_json::json!([])
        );
    }

    #[test]
    fn the_view_lists_the_newest_artifacts_in_the_order_they_were_recorded() {
        let count = MAX_VIEW_ARTIFACTS + 3;
        let view = shown_snapshot(&holding((0..count).map(|i| artifact(i, None)).collect()));
        let expected: Vec<String> = (3..count).map(|i| format!("file-{i}.txt")).collect();
        assert_eq!(names(&view), expected);
        // The list is a window, not a transcript cut: nothing says it was.
        assert!(!view.truncated);
    }

    #[test]
    fn artifacts_yield_oldest_first_after_the_transcript_and_before_tools() {
        let view = bound_view(shown_snapshot(&holding(
            (0..MAX_VIEW_ARTIFACTS).map(|i| artifact(i, None)).collect(),
        )));
        let full = serde_json::to_vec(&view).unwrap().len();
        // An older message yields before any artifact does.
        let mut longer = view.clone();
        let older = longer.messages[0].clone();
        longer.messages.insert(0, older);
        let bounded = bound_view_within(longer, full, true);
        assert_eq!(bounded.artifacts.len(), MAX_VIEW_ARTIFACTS);
        // With the transcript given up, the oldest artifact goes next.
        let mut bare = view;
        bare.messages.clear();
        let full = serde_json::to_vec(&bare).unwrap().len();
        let bounded = bound_view_within(bare, full - 1, true);
        assert!(bounded.truncated);
        assert_eq!(bounded.artifacts.len(), MAX_VIEW_ARTIFACTS - 1);
        assert_eq!(bounded.artifacts[0].name, "file-1.txt");
        // Kept back for its interactions, a bound still gives artifacts up.
        let mut bare = shown_snapshot(&holding(vec![artifact(0, None)]));
        bare.messages.clear();
        let full = serde_json::to_vec(&bare).unwrap().len();
        assert!(bound_view_within(bare, full - 1, false)
            .artifacts
            .is_empty());
    }
}

mod notes {
    use super::*;
    use crate::conversation::projection::MAX_VIEW_NOTES;
    use nessa_sdk::application::agent_execution::sessions::{
        NoteDelivery, NoteId, NoteRecord, NoteRetirement, NoteRetirementReason, NoteText,
    };
    use nessa_sdk::domain::agent_execution::executions::{GrantInitiator, Initiator};

    fn note(index: usize, delivery: NoteDelivery) -> NoteRecord {
        NoteRecord {
            id: NoteId::new(format!("note-{index}")).unwrap(),
            text: NoteText::new(format!("said {index}")).unwrap(),
            delivery,
            actor: ActionContext::new("person", "phone", format!("request-{index}")).unwrap(),
            author: Initiator::Owner,
            accepted_at_ms: index as u64,
            after_execution: None,
            retired: None,
        }
    }
    fn ids(view: &ConversationView) -> Vec<String> {
        view.notes
            .iter()
            .map(|note| note.message_id.clone())
            .collect()
    }

    /// Row J24: each note shows where it stands, the turn that took it or
    /// why it was retired, the turn it follows, and who left it from where.
    #[test]
    fn the_view_shows_each_note_where_it_stands_and_who_left_it() {
        let mut snapshot = completed_snapshot("execution", Vec::new());
        let taken = note(1, NoteDelivery::NextTurn);
        snapshot.invocations[0].notes = vec![taken.id.clone()];
        let mut retired = note(2, NoteDelivery::NextTurn);
        retired.author = Initiator::Grant(GrantInitiator::new("commenter", 4).unwrap());
        retired.after_execution = Some(ExecutionId::new("execution").unwrap());
        retired.retired = Some(NoteRetirement {
            reason: NoteRetirementReason::PermissionLost,
            at_ms: 9,
        });
        snapshot.notes = vec![
            note(0, NoteDelivery::RecordOnly),
            taken,
            retired,
            note(3, NoteDelivery::NextTurn),
        ];
        let wire = serde_json::to_value(shown_snapshot(&snapshot)).unwrap();
        assert_eq!(
            wire["notes"],
            serde_json::json!([
                {
                    "messageId": "note-0", "text": "said 0", "acceptedAtMs": 0,
                    "delivery": "record_only", "deliveryState": "recorded",
                    "author": {"kind": "owner", "surface": "phone"},
                },
                {
                    "messageId": "note-1", "text": "said 1", "acceptedAtMs": 1,
                    "delivery": "next_turn", "deliveryState": "assigned",
                    "executionId": "execution",
                    "author": {"kind": "owner", "surface": "phone"},
                },
                {
                    "messageId": "note-2", "text": "said 2", "acceptedAtMs": 2,
                    "delivery": "next_turn", "deliveryState": "retired",
                    "retiredReason": "permission_lost", "afterExecutionId": "execution",
                    "author": {"kind": "grant", "surface": "phone"},
                },
                {
                    "messageId": "note-3", "text": "said 3", "acceptedAtMs": 3,
                    "delivery": "next_turn", "deliveryState": "pending",
                    "author": {"kind": "owner", "surface": "phone"},
                },
            ])
        );
        // Notes are not messages: the transcript is the turn alone.
        assert_eq!(wire["messages"].as_array().unwrap().len(), 1);
    }

    #[test]
    fn the_view_lists_the_newest_notes_and_they_yield_after_the_transcript() {
        let count = MAX_VIEW_NOTES + 3;
        let mut snapshot = completed_snapshot("execution", Vec::new());
        snapshot.notes = (0..count)
            .map(|i| note(i, NoteDelivery::RecordOnly))
            .collect();
        let view = bound_view(shown_snapshot(&snapshot));
        let expected: Vec<String> = (3..count).map(|i| format!("note-{i}")).collect();
        assert_eq!(ids(&view), expected);
        assert!(!view.truncated);
        // An older message yields before any note does.
        let full = serde_json::to_vec(&view).unwrap().len();
        let mut longer = view.clone();
        let older = longer.messages[0].clone();
        longer.messages.insert(0, older);
        assert_eq!(
            bound_view_within(longer, full, true).notes.len(),
            MAX_VIEW_NOTES
        );
        // With the transcript given up, the oldest note goes before any
        // artifact.
        let mut bare = view;
        bare.messages.clear();
        let full = serde_json::to_vec(&bare).unwrap().len();
        let bounded = bound_view_within(bare, full - 1, true);
        assert!(bounded.truncated);
        assert_eq!(bounded.notes.len(), MAX_VIEW_NOTES - 1);
        assert_eq!(bounded.notes[0].message_id, "note-4");
        // Kept back for its interactions, a bound still gives notes up.
        snapshot.notes.truncate(1);
        let mut bare = shown_snapshot(&snapshot);
        bare.messages.clear();
        let full = serde_json::to_vec(&bare).unwrap().len();
        assert!(bound_view_within(bare, full - 1, false).notes.is_empty());
    }
}

#[test]
fn selection_and_runtime_replacements_own_revision_without_transcript_output() {
    let mut projection = projection();
    let initial = projection.read();
    let selection = ConversationSelectionView {
        agent: AgentId::Claude,
        model: "test".into(),
        approval_mode: ConversationApprovalMode::Ask,
    };
    projection.selection(selection.clone());
    let selected = projection.read();
    assert_ne!(selected.revision, initial.revision);
    projection.selection(selection);
    assert_eq!(projection.read().revision, selected.revision);
    projection.approval_mode(ConversationApprovalMode::Auto);
    let changed = projection.read();
    assert_ne!(changed.revision, selected.revision);
    assert_eq!(
        changed.selection.as_ref().unwrap().approval_mode,
        ConversationApprovalMode::Auto
    );
    projection.approval_mode(ConversationApprovalMode::Auto);
    assert_eq!(projection.read().revision, changed.revision);
    let runtime = ConversationRuntime {
        model: "test".into(),
        provider: "provider".into(),
        workspace: "/workspace".into(),
    };
    projection.runtime(runtime.clone());
    let running = projection.read();
    assert_ne!(running.revision, changed.revision);
    projection.runtime(runtime);
    assert_eq!(projection.read().revision, running.revision);
    projection.runtime(ConversationRuntime {
        workspace: "/other".into(),
        ..running.runtime.unwrap()
    });
    assert_ne!(projection.read().revision, running.revision);
    projection.selection(ConversationSelectionView {
        agent: AgentId::Codex,
        model: "other".into(),
        approval_mode: ConversationApprovalMode::Auto,
    });
    assert_ne!(projection.read().revision, running.revision);
    assert!(projection.read().messages.is_empty());
}

#[test]
fn a_waiting_review_counts_down_from_the_deadline_its_live_owner_keeps() {
    use std::time::Duration;
    let due = 1_700_000_030_000;
    let armed = PermissionReviewDeadline::new(
        PermissionToolClass::Other,
        Duration::from_secs(30),
        PermissionEffect::Deny,
        STANDARD_DENY_REASON,
    )
    .unwrap()
    .promise(due)
    .unwrap();
    let shown = agent_review(&pending_review(once_options(), "{}", Some(armed))).unwrap();
    let deadline = shown
        .deadline
        .as_ref()
        .expect("an armed deadline is on the waiting review");
    assert_eq!(deadline.wait_ms, 30_000);
    assert_eq!(deadline.due_at_ms, due);
    assert_eq!(deadline.effect, ConversationPermissionOptionEffect::Deny);
    assert_eq!(deadline.reason, STANDARD_DENY_REASON);
    assert!(!deadline.stops_turn);
    // A deadline restored from storage is shown only because the live owner
    // waits on it now; history alone never puts a review on the card.
    let restored = PermissionReviewDeadline::new(
        PermissionToolClass::Other,
        Duration::from_secs(30),
        PermissionEffect::Deny,
        STANDARD_DENY_REASON,
    )
    .unwrap()
    .stopping()
    .promise(due)
    .unwrap()
    .restored(true, Some(due))
    .expect("a stopping deny deadline restores");
    assert!(!restored.armed());
    let shown = agent_review(&pending_review(once_options(), "{}", Some(restored))).unwrap();
    let deadline = shown
        .deadline
        .as_ref()
        .expect("a review the live owner holds counts down from its saved deadline");
    assert_eq!(deadline.due_at_ms, due);
    assert_eq!(deadline.reason, STANDARD_STOP_REASON);
    assert!(deadline.stops_turn);
}

#[test]
fn an_automatic_decision_is_named_allowed_or_denied_in_the_notice() {
    let notice = |effect, sentence: &str| {
        let reason =
            nessa_sdk::domain::agent_execution::permissions::DeadlineReason::new(sentence).unwrap();
        let snapshot = review_snapshot(vec![
            review_with_deadline(effect, sentence),
            event(ExecutionUpdate::DeadlineAnswered {
                id: PermissionId::new("7").unwrap(),
                effect,
                reason,
            }),
        ]);
        let mut projection = projection();
        projection.replace_committed(
            &committed("incarnation", 1, 1, 1, Some(&snapshot)),
            &[],
            Some(&ExecutionId::new("execution").unwrap()),
        );
        projection.transcript_state(ConversationTranscriptState::Complete);
        let view = projection.read();
        view.messages[0].parts[0].text.clone()
    };
    assert_eq!(
        notice(PermissionEffect::Deny, STANDARD_DENY_REASON),
        format!("Denied automatically: {STANDARD_DENY_REASON}")
    );
    assert_eq!(
        notice(PermissionEffect::Allow, "allowed because nobody was there"),
        "Allowed automatically: allowed because nobody was there"
    );
    assert_eq!(
        notice(PermissionEffect::Deny, STANDARD_STOP_REASON),
        format!("Denied automatically: {STANDARD_STOP_REASON}")
    );
}

#[test]
fn a_deadline_notice_without_an_earlier_request_in_that_execution_is_omitted() {
    let reason =
        nessa_sdk::domain::agent_execution::permissions::DeadlineReason::new(STANDARD_DENY_REASON)
            .unwrap();
    let notice = |id: &str| {
        event(ExecutionUpdate::DeadlineAnswered {
            id: PermissionId::new(id).unwrap(),
            effect: PermissionEffect::Deny,
            reason: reason.clone(),
        })
    };
    let texts = |events| {
        let snapshot = review_snapshot(events);
        let mut projection = projection();
        projection.replace_committed(
            &committed("incarnation", 1, 1, 1, Some(&snapshot)),
            &[],
            Some(&ExecutionId::new("execution").unwrap()),
        );
        projection.transcript_state(ConversationTranscriptState::Complete);
        projection
            .read()
            .messages
            .into_iter()
            .flat_map(|message| message.parts)
            .filter(|part| part.kind == "local_notice")
            .map(|part| part.text)
            .collect::<Vec<_>>()
    };
    assert!(
        texts(vec![notice("7")]).is_empty(),
        "a notice with no earlier request is omitted"
    );
    let published = || review_with_deadline(PermissionEffect::Deny, STANDARD_DENY_REASON);
    assert!(
        texts(vec![published(), notice("other")]).is_empty(),
        "a notice whose id is not the request's is omitted"
    );
    assert_eq!(
        texts(vec![published(), notice("7")]),
        vec![format!("Denied automatically: {STANDARD_DENY_REASON}")]
    );
    let ExecutionUpdate::PermissionRequested {
        tool_id,
        observation,
        input,
        options,
        deadline,
        ..
    } = published().update().clone()
    else {
        unreachable!()
    };
    let opaque = event(ExecutionUpdate::PermissionRequested {
        id: PermissionId::new("review-a").unwrap(),
        tool_id,
        observation,
        input,
        options,
        deadline,
    });
    assert!(
        texts(vec![opaque, notice("review-a")]).is_empty(),
        "a review whose id is not a notice id shows no notice the client would refuse"
    );
}

#[test]
fn a_deadline_notice_is_shown_only_as_the_one_settlement_its_review_published() {
    use nessa_sdk::domain::agent_execution::permissions::{DeadlineReason, STANDARD_ALLOW_REASON};
    let notice = |effect, reason: &str| {
        event(ExecutionUpdate::DeadlineAnswered {
            id: PermissionId::new("7").unwrap(),
            effect,
            reason: DeadlineReason::new(reason).unwrap(),
        })
    };
    let shown = |events| {
        let snapshot = review_snapshot(events);
        let mut projection = projection();
        projection.replace_committed(
            &committed("incarnation", 1, 1, 1, Some(&snapshot)),
            &[],
            Some(&ExecutionId::new("execution").unwrap()),
        );
        projection.transcript_state(ConversationTranscriptState::Complete);
        let view = projection.read();
        let notices = view
            .messages
            .iter()
            .flat_map(|message| &message.parts)
            .filter(|part| part.kind == "local_notice")
            .map(|part| part.text.clone())
            .collect::<Vec<String>>();
        notices
    };
    let deny = || notice(PermissionEffect::Deny, STANDARD_DENY_REASON);
    let denied = format!("Denied automatically: {STANDARD_DENY_REASON}");
    let published = || review_with_deadline(PermissionEffect::Deny, STANDARD_DENY_REASON);

    assert_eq!(
        shown(vec![review("{}".into()), deny()]),
        Vec::<String>::new(),
        "a review that published no deadline was not settled by one"
    );
    assert_eq!(
        shown(vec![
            published(),
            notice(PermissionEffect::Allow, STANDARD_ALLOW_REASON)
        ]),
        Vec::<String>::new(),
        "a notice that contradicts the published effect is not shown"
    );
    assert_eq!(
        shown(vec![
            published(),
            notice(PermissionEffect::Deny, STANDARD_STOP_REASON)
        ]),
        Vec::<String>::new(),
        "a notice whose reason is not the published one is not shown"
    );
    assert_eq!(
        shown(vec![published(), deny(), deny()]),
        vec![denied.clone()],
        "a duplicate notice is shown once"
    );
    assert_eq!(
        shown(vec![
            published(),
            cancelled_review(
                PermissionCancellationReason::provider_withdrawal(),
                CancellationOrigin::Provider,
            ),
            deny(),
        ]),
        Vec::<String>::new(),
        "a review cancelled for another reason was not settled by its deadline"
    );
    assert_eq!(
        shown(vec![
            published(),
            cancelled_review(
                PermissionCancellationReason::deadline_exceeded(),
                CancellationOrigin::Runtime,
            ),
            deny(),
        ]),
        Vec::<String>::new(),
        "an execution timeout shares the reason, but the review's deadline did not settle it"
    );
    let deadline_after = |wait, effect, reason| {
        PermissionReviewDeadline::new(
            PermissionToolClass::Other,
            std::time::Duration::from_secs(wait),
            effect,
            reason,
        )
        .unwrap()
    };
    let deadline = |effect, reason| deadline_after(30, effect, reason);
    // A cancellation by a deadline the review did not publish never reaches
    // the projection: the SDK refuses that history.
    let refused = |events| {
        CommittedSession::new(
            SessionId::new("conversation").unwrap(),
            "incarnation".into(),
            1,
            1,
            1,
            Some(review_snapshot(events)),
            CommittedStatus::new(CommittedCompleteness::Complete, CommittedFreshness::Current),
        )
        .is_err()
    };
    assert!(
        refused(vec![
            published(),
            deadline_cancelled_review(deadline_after(
                8,
                PermissionEffect::Deny,
                STANDARD_DENY_REASON
            )),
            deny(),
        ]),
        "a cancellation by a deadline that shares the outcome but not the wait"
    );
    assert!(
        refused(vec![
            published(),
            deadline_cancelled_review(deadline(PermissionEffect::Allow, STANDARD_ALLOW_REASON)),
            deny(),
        ]),
        "a cancellation by a deadline the review did not publish"
    );
    assert_eq!(
        shown(vec![
            published(),
            deadline_cancelled_review(deadline(PermissionEffect::Deny, STANDARD_DENY_REASON)),
            deny(),
        ]),
        vec![denied],
        "a review its deadline cancelled has that deadline's notice"
    );
}

#[test]
fn publishing_mode_without_a_selection_does_not_invent_one() {
    let mut projection = projection();
    let before = projection.read();
    projection.approval_mode(ConversationApprovalMode::Auto);
    assert!(projection.read().selection.is_none());
    assert_eq!(projection.read().revision, before.revision);
}

#[test]
fn model_selection_publishes_acknowledged_settings_in_one_revision() {
    let mut projection = projection();
    projection.selection(ConversationSelectionView {
        agent: AgentId::Claude,
        model: "before".into(),
        approval_mode: ConversationApprovalMode::Auto,
    });
    let before = projection.read();
    projection.model_selection(
        "after".into(),
        ConversationApprovalMode::Ask,
        ConversationModelChangeEffect::SubsequentResponses,
    );
    let after = projection.read();
    assert_ne!(after.revision, before.revision);
    let selection = after.selection.as_ref().unwrap();
    assert_eq!(selection.agent, AgentId::Claude);
    assert_eq!(selection.model, "after");
    assert_eq!(selection.approval_mode, ConversationApprovalMode::Ask);
    assert_eq!(
        after.model_change_effect,
        Some(ConversationModelChangeEffect::SubsequentResponses)
    );
    projection.model_selection(
        "after".into(),
        ConversationApprovalMode::Ask,
        ConversationModelChangeEffect::SubsequentResponses,
    );
    assert_eq!(projection.read().revision, after.revision);
}

#[test]
fn pending_model_request_retains_confirmed_selection_and_has_its_own_revision() {
    let mut projection = projection();
    projection.selection(ConversationSelectionView {
        agent: AgentId::Codex,
        model: "confirmed".into(),
        approval_mode: ConversationApprovalMode::Ask,
    });
    let confirmed = projection.read();
    let pending = |request: &str| {
        projection.read_with_model_change(Some(ConversationModelChangeView {
            request_id: request.into(),
            requested_model: "requested".into(),
            status: ConversationModelChangeStatus::Changing,
        }))
    };
    let first = pending("first");
    assert_eq!(first.selection, confirmed.selection);
    assert_eq!(
        first.model_change.as_ref().unwrap().requested_model,
        "requested"
    );
    assert_ne!(first.revision, confirmed.revision);
    assert_eq!(first.revision, pending("first").revision);
    assert_ne!(first.revision, pending("second").revision);
    assert_eq!(projection.read().revision, confirmed.revision);
}

#[test]
fn publishing_model_without_a_selection_does_not_invent_one() {
    let mut projection = projection();
    let before = projection.read();
    projection.model_selection(
        "model".into(),
        ConversationApprovalMode::Ask,
        ConversationModelChangeEffect::NextTurn,
    );
    let after = projection.read();
    assert!(after.selection.is_none());
    assert!(after.model_change_effect.is_none());
    assert_eq!(after.revision, before.revision);
}
