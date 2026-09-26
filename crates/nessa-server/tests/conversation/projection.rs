//! Projections are bounded display state, not permission or scheduling authority.
use super::{
    projection::{clipped, Projection, MAX_TEXT},
    ConversationAgentFeatures, ConversationAttachmentEvidenceFailure,
    ConversationAttachmentEvidenceFailureCode, ConversationCaller, ConversationCapabilities,
    ConversationDependencies, ConversationLifecycle, ConversationLifecyclePhase,
    ConversationLimits, ConversationMessageStatus, ConversationPendingMode, ConversationService,
    PermissionDenialSupport, ProviderSessionErasers, SubmissionMode, SubmittedMessage,
};
use crate::{
    conversation::domain::ConversationId,
    conversation_test_support::{
        fixture, only, AcceptingCreationAudit, AcceptingDeletionAudit, MemorySummaries, Provider,
        RecordingFileLinkAudit, TestClock, Unlisted, DELETION_BUDGETS,
    },
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::{
    application::agent_execution::{
        agents::{AgentError, ProviderDiagnostic},
        executions::{
            ExecutionEvent, ExecutionRequest, ExecutionUpdate,
            SubmissionMode as InvocationSubmissionMode,
        },
        permissions::ActionContext,
        providers::{
            ExecutionReport, ObservationFailure, ObservationFailureCause, OperationCapabilities,
            ProviderExecutionReply, ProviderIdentity, ProviderSessionState,
        },
        sessions::{InvocationRecord, SessionSnapshot, SubmissionAcknowledgement},
        tools::ToolReviewInput,
    },
    domain::agent_execution::{
        executions::{ExecutionId, ExecutionOutcome, MessageChunk, MessageId},
        permissions::{
            PermissionDecision, PermissionEffect, PermissionId, PermissionOfferPolicy,
            PermissionOption, PermissionOptionId, PermissionOptions, PermissionScope,
            ReviewDecline, ReviewDeclineId, ReviewDeclineObservation, ReviewDeclineReason,
            ReviewDeclineStage,
        },
        prompts::{PromptText, UserMessage},
        questions::{
            AgentQuestion, AnswerOption, AnswerShape, Question, QuestionId, MAX_OPEN_ASK_COST,
        },
        sessions::{ExecutionSessionId, ProviderContext, SessionId},
        tools::{ToolCallId, ToolCallUpdate, ToolContent, ToolObservation, ToolStatus},
    },
};
use std::sync::Arc;
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
fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
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

fn decline_event(id: &str, delivery: ReviewDeclineStage) -> ExecutionEvent {
    let selected = ReviewDeclineObservation::selected(
        ReviewDeclineId::new(id).unwrap(),
        ReviewDecline::new(Some("Read"), ReviewDeclineReason::ToolNotReviewable),
    );
    event(ExecutionUpdate::ReviewDeclined(
        if delivery == ReviewDeclineStage::Selected {
            selected
        } else {
            selected.advance(delivery).unwrap()
        },
    ))
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
fn provider_diagnostic_survives_receipt_failure_and_snapshot_restoration() {
    let provider_error = AgentError::Provider {
        code: -32603,
        diagnostic: Some(ProviderDiagnostic::new("provider refused the prompt")),
    };
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations[0].provider_report = Some(ExecutionReport::new(
        Some(Err(provider_error.clone())),
        None,
        ProviderSessionState::CleanupRequired,
    ));
    snapshot.invocations[0].result = Some(Err(provider_error));

    let expected = "The agent provider reported an error: provider refused the prompt";
    let mut live = projection();
    live.settled("execution", Some(&snapshot));
    live.receipt_failed("execution");
    assert_eq!(live.read().messages[0].error.as_deref(), Some(expected));

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
    assert_eq!(restored.read().messages[0].error.as_deref(), Some(expected));
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

#[test]
fn blank_provider_diagnostic_and_missing_snapshot_use_reachable_generic_notice() {
    let provider_error = AgentError::Provider {
        code: -32603,
        diagnostic: Some(ProviderDiagnostic::new("  \n\t")),
    };
    let mut snapshot = review_snapshot(Vec::new());
    snapshot.invocations[0].provider_report = Some(ExecutionReport::new(
        Some(Err(provider_error.clone())),
        None,
        ProviderSessionState::CleanupRequired,
    ));
    snapshot.invocations[0].result = Some(Err(provider_error));
    let generic = "The turn could not complete all required work.";
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
    assert_eq!(restored.read().messages[0].error.as_deref(), Some(generic));

    let mut without_snapshot = projection();
    without_snapshot.receipt_failed("execution");
    assert_eq!(
        without_snapshot.read().messages[0].error.as_deref(),
        Some(generic)
    );
}

fn assert_partial_tool(view: &super::ConversationView, expected: bool) {
    if !expected {
        assert!(view.tools.is_empty());
        return;
    }
    assert_eq!(view.tools.len(), 1);
    assert_eq!(view.tools[0].status, "running");
    assert_eq!(view.tools[0].details, "partial output");
    assert!(view.messages[0]
        .parts
        .iter()
        .any(|part| part.kind == "tool" && part.tool_id == "tool"));
}

async fn assert_terminal_failure_round_trip(
    provider_error: AgentError,
    updates: Vec<ExecutionUpdate>,
    expected_notice: &str,
    expected_partial_tool: bool,
) {
    let (service, provider, repository, storage) = fixture(ConversationLimits::default());
    *provider.execution_reply.lock().unwrap() =
        Some(ProviderExecutionReply::Finished(ExecutionReport::new(
            Some(Err(provider_error.clone())),
            None,
            ProviderSessionState::CleanupRequired,
        )));
    *provider.execution_observation_failure.lock().unwrap() = Some(ObservationFailure::new(
        provider_error,
        ObservationFailureCause::ExecutionFailed,
    ));
    *provider.execution_updates.lock().unwrap() = updates;
    let (release_execution, execution_gate) = tokio::sync::oneshot::channel();
    *provider.execution_gate.lock().unwrap() = Some(execution_gate);
    let id = ConversationId::new(&uuid::Uuid::new_v4().to_string()).unwrap();
    service
        .create(id.clone(), caller("create"), None)
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("send"),
            "execution".into(),
            SubmittedMessage {
                text: "Hello".into(),
                images: Vec::new(),
                files: Vec::new(),
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    tokio::time::timeout(
        std::time::Duration::from_secs(3),
        provider.execution_started.notified(),
    )
    .await
    .unwrap_or_else(|_| {
        panic!(
            "provider execution did not start; dispatched executions: {:?}",
            provider.executions.lock().unwrap()
        )
    });
    assert_eq!(
        provider.executions.lock().unwrap().as_slice(),
        ["execution"]
    );
    release_execution.send(()).unwrap();

    let failed = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            let view = service.read(id.clone(), caller("read")).await.unwrap();
            if view.messages.first().is_some_and(|message| {
                message.status == ConversationMessageStatus::Failed && message.error.is_some()
            }) {
                break view;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| {
        let executions = provider.executions.lock().unwrap().clone();
        let close_calls = provider
            .close_calls
            .load(std::sync::atomic::Ordering::SeqCst);
        panic!(
            "provider execution did not project a terminal failure; executions: {executions:?}, close calls: {close_calls}"
        )
    });
    assert!(failed.pending.is_empty());
    assert_eq!(failed.messages[0].error.as_deref(), Some(expected_notice));
    assert_partial_tool(&failed, expected_partial_tool);
    assert_eq!(
        provider
            .close_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );

    let repeated = service
        .read(id.clone(), caller("repeat-read"))
        .await
        .unwrap();
    assert_eq!(repeated.revision, failed.revision);
    assert_eq!(
        repeated.messages[0].status,
        ConversationMessageStatus::Failed
    );
    assert_eq!(repeated.messages[0].error, failed.messages[0].error);
    assert_partial_tool(&repeated, expected_partial_tool);

    service.shutdown().await.unwrap();
    assert_eq!(
        provider
            .close_calls
            .load(std::sync::atomic::Ordering::SeqCst),
        1
    );
    drop(service);
    tokio::task::yield_now().await;
    let restored = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider))),
            storage,
            metadata: repository,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let view = restored.read(id, caller("restored-read")).await.unwrap();
    assert_eq!(view.messages[0].status, ConversationMessageStatus::Failed);
    assert_eq!(view.messages[0].error, failed.messages[0].error);
    assert_partial_tool(&view, expected_partial_tool);
}

#[tokio::test]
async fn provider_failure_stays_terminal_across_closed_session_reads_and_restoration() {
    assert_terminal_failure_round_trip(
        AgentError::Provider {
            code: -32603,
            diagnostic: Some(ProviderDiagnostic::new(
                "OpenCode's free tier can only be used from within OpenCode",
            )),
        },
        Vec::new(),
        "The agent provider reported an error: OpenCode's free tier can only be used from within OpenCode. The turn could not complete all required work.",
        false,
    )
    .await;
}

#[tokio::test]
async fn protocol_failure_after_partial_tool_preserves_observation_across_terminal_reads() {
    // The accepted prefix is a valid domain update. The ACP contract tests own
    // malformed-wire parsing; this boundary starts with its typed protocol
    // failure and proves the SDK/session/projection path that follows it.
    assert_terminal_failure_round_trip(
        AgentError::Protocol("invalid tool status".into()),
        vec![ExecutionUpdate::Tool(ToolCallUpdate::new(
            ToolCallId::new("tool").unwrap(),
            Some("Shell".into()),
            None,
            Some(ToolStatus::Running),
            None,
            Some(vec![ToolContent::text("partial output")]),
        ))],
        "The turn could not complete all required work.",
        true,
    )
    .await;
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
fn declined_reviews_upsert_by_identity_in_order_and_match_restoration() {
    let events = vec![
        decline_event("1", ReviewDeclineStage::Selected),
        decline_event("1", ReviewDeclineStage::WriteConfirmed),
        decline_event("2", ReviewDeclineStage::Selected),
        decline_event("2", ReviewDeclineStage::WriteUnconfirmed),
        event(ExecutionUpdate::Message(MessageChunk::text("carried on"))),
        event(ExecutionUpdate::Finished(ExecutionOutcome::Completed)),
    ];
    let mut live = projection();
    for event in &events {
        live.event(event);
    }
    let live_view = live.read();
    let parts = &live_view.messages[0].parts;
    assert_eq!(parts.len(), 3);
    assert_eq!(parts[0].notice_id, "1");
    assert_eq!(parts[0].offset, 0);
    assert_eq!(parts[1].notice_id, "2");
    assert_eq!(parts[1].offset, 2);
    assert!(parts[1].text.contains("could not confirm writing"));
    assert_eq!(parts[2].text, "carried on");
    assert_eq!(parts[2].offset, 4);
    assert_eq!(
        live_view.messages[0].status,
        ConversationMessageStatus::Completed
    );

    let snapshot = completed_snapshot("execution", events);
    let restored = Projection::new(
        "conversation".into(),
        live_view.capabilities.clone(),
        Some(&snapshot),
    )
    .read();
    assert_eq!(restored.messages[0].parts, live_view.messages[0].parts);
    assert_eq!(restored.messages[0].status, live_view.messages[0].status);
}

#[test]
fn declined_review_replay_after_lag_preserves_one_final_notice_and_following_text() {
    let selected = decline_event("1", ReviewDeclineStage::Selected);
    let confirmed = decline_event("1", ReviewDeclineStage::WriteConfirmed);
    let text = event(ExecutionUpdate::Message(MessageChunk::text("carried on")));
    let finished = event(ExecutionUpdate::Finished(ExecutionOutcome::Completed));
    let events = vec![selected.clone(), confirmed.clone(), text.clone(), finished];
    let snapshot = completed_snapshot("execution", events.clone());
    let mut live = projection();
    live.event(&selected);
    live.lagged();
    live.event(&confirmed);
    live.event(&text);
    let lagged = live.read();
    assert!(text_parts(&lagged, "execution").is_empty());
    assert_eq!(
        lagged.messages[0]
            .parts
            .iter()
            .filter(|part| part.kind == "local_notice")
            .count(),
        1
    );

    live.settled("execution", Some(&snapshot));
    let settled = live.read();
    for buffered in &events {
        live.event(buffered);
    }
    live.settled("execution", Some(&snapshot));
    let repeated = live.read();
    let restored = Projection::new(
        "conversation".into(),
        settled.capabilities.clone(),
        Some(&snapshot),
    )
    .read();
    for view in [&settled, &repeated, &restored] {
        assert_eq!(view.messages.len(), 1);
        let message = &view.messages[0];
        assert_eq!(message.parts, settled.messages[0].parts);
        assert_eq!(message.parts.len(), 2);
        assert_eq!(message.parts[0].kind, "local_notice");
        assert_eq!(message.parts[0].notice_id, "1");
        assert_eq!(message.parts[0].offset, 0);
        assert!(!message.parts[0].text.contains("has not confirmed"));
        assert_eq!(message.parts[1].kind, "text");
        assert_eq!(message.parts[1].offset, 2);
        assert_eq!(message.parts[1].text, "carried on");
        assert_eq!(message.event_count, events.len());
        assert_eq!(message.status, ConversationMessageStatus::Completed);
        assert!(message.error.is_none());
        assert!(view.permissions.is_empty());
        assert!(view.pending.is_empty());
    }
    // A recovered turn does not clear the session-wide live-lag warning. A fresh
    // restored projection has no missed-live-observation history to report.
    assert!(settled.truncated);
    assert!(repeated.truncated);
    assert_eq!(
        settled.permission_view_error,
        repeated.permission_view_error
    );
    assert!(!restored.truncated);
    assert!(restored.permission_view_error.is_none());
}

#[test]
fn local_notice_append_and_upsert_obey_the_message_text_budget() {
    let mut projection = projection();
    projection.event(&event(ExecutionUpdate::Message(MessageChunk::text(
        "x".repeat((MAX_TEXT * 2) - 8),
    ))));
    projection.event(&decline_event("1", ReviewDeclineStage::Selected));
    projection.event(&decline_event("1", ReviewDeclineStage::WriteUnconfirmed));
    let view = projection.read();
    assert!(view.truncated);
    assert!(
        view.messages[0]
            .parts
            .iter()
            .map(|part| part.text.len())
            .sum::<usize>()
            <= MAX_TEXT * 2
    );
    assert_eq!(
        view.messages[0]
            .parts
            .iter()
            .filter(|part| part.kind == "local_notice")
            .count(),
        1
    );
}

#[test]
fn selected_notice_remains_truthful_when_terminal_history_has_no_final_publication() {
    let events = vec![
        decline_event("1", ReviewDeclineStage::Selected),
        event(ExecutionUpdate::Finished(ExecutionOutcome::Completed)),
    ];
    let mut live = projection();
    for event in &events {
        live.event(event);
    }
    let live_view = live.read();
    let notice = &live_view.messages[0].parts[0];
    assert!(notice.text.contains("has not confirmed writing"));
    assert_eq!(
        live_view.messages[0].status,
        ConversationMessageStatus::Completed
    );

    let restored = Projection::new(
        "conversation".into(),
        live_view.capabilities.clone(),
        Some(&completed_snapshot("execution", events)),
    )
    .read();
    assert_eq!(restored.messages[0].parts, live_view.messages[0].parts);
    assert_eq!(restored.messages[0].status, live_view.messages[0].status);
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
/// pending to one that is queued — or, in a truncated view, to one no longer
/// shown. The client refuses the whole view otherwise,
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
        // Absent is allowed only in a view that says it was truncated.
        let found = status(execution);
        assert!(
            found == Some(ConversationMessageStatus::Running)
                || (found.is_none() && view.truncated),
            "{execution}: {found:?}"
        );
    }
    for item in &view.pending {
        let found = status(&item.execution_id);
        assert!(
            found == Some(ConversationMessageStatus::Queued) || (found.is_none() && view.truncated),
            "{}: {found:?}",
            item.execution_id
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
            agent_features: OperationCapabilities::default().into(),
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
            agent_features: OperationCapabilities::default().into(),
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

#[test]
fn a_late_event_does_not_revive_a_turn_whose_receipt_failed() {
    // An event buffered before the receipt failed, delivered after it, marked
    // the turn as begun here again; once pushed out of view, lag recovery then
    // offered an ask the agent had already stopped waiting on.
    let mut projection = projection();
    projection.event(&asked("execution", "1"));
    let snapshot = review_snapshot(vec![asked("execution", "1")]);
    projection.settled("execution", Some(&snapshot));
    projection.receipt_failed("execution");
    projection.event(&event(ExecutionUpdate::Message(MessageChunk::text("late"))));
    evict_running_execution(&mut projection);
    projection.lagged();
    projection.recover_permissions(Some(&snapshot));
    assert_eq!(offers_only_running_asks(&mut projection), 0);
}

#[test]
fn a_retried_submission_does_not_hide_the_ask_its_turn_is_waiting_on() {
    // Retrying a running turn whose message was pushed out rebuilt the message
    // as queued, and a queued message offers nothing — the ask vanished while
    // its agent waited on it.
    let mut projection = projection();
    projection.event(&asked("execution", "1"));
    evict_running_execution(&mut projection);
    projection.admitted(
        "execution",
        &said("message"),
        ConversationPendingMode::Queued,
    );
    assert_eq!(offers_only_running_asks(&mut projection), 1);
}

/// An ask as hard to carry as its text allows: every string full of the
/// characters a text format escapes, under the longest identities.
fn escaped_ask(execution: &str, question: &str, options: usize, bytes: usize) -> ExecutionEvent {
    let text = |length: usize| "\"\\\n\t".repeat(length.div_ceil(4))[..length].to_owned();
    ExecutionEvent::new(
        ExecutionId::new(execution).unwrap(),
        ExecutionUpdate::QuestionAsked {
            id: QuestionId::new(question).unwrap(),
            question: AgentQuestion::new(
                text(bytes),
                vec![Question::new(
                    "k".repeat(64),
                    text(bytes),
                    Some(text(bytes)),
                    AnswerShape::Many,
                    (0..options)
                        .map(|index| {
                            AnswerOption::new(
                                format!("{index}{}", text(bytes)),
                                text(bytes),
                                Some(text(bytes)),
                            )
                            .unwrap()
                        })
                        .collect(),
                    Some("f".repeat(64)),
                    true,
                )
                .unwrap()],
            )
            .unwrap(),
        },
    )
}
fn cost(event: &ExecutionEvent) -> usize {
    let ExecutionUpdate::QuestionAsked { question, .. } = event.update() else {
        unreachable!()
    };
    question.carrying_cost()
}

#[test]
fn an_ask_is_never_written_larger_than_it_costs_to_carry() {
    // The binding admits asks by their carrying cost and the view relies on
    // it, so the cost must bound what the view actually writes — escapes, the
    // longest identities and all.
    let execution = "e".repeat(256);
    for (options, bytes) in [(1, 1), (4, 64), (32, 16), (2, 1000)] {
        let mut projection = projection();
        let ask = escaped_ask(&execution, &"9".repeat(20), options, bytes);
        projection.event(&ask);
        let view = projection.read();
        assert_eq!(view.questions.len(), 1);
        let written = serde_json::to_vec(&view.questions[0]).unwrap().len();
        assert!(
            written <= cost(&ask),
            "{written} > {} for {options}x{bytes}",
            cost(&ask)
        );
    }
}

#[test]
fn every_ask_the_binding_admits_is_offered_whole_within_the_view() {
    // Eight asks whose costs together fill the binding's budget, beside a
    // transcript far larger than the view: every ask is still offered, and the
    // view keeps its bound by giving up everything else first.
    let mut projection = projection();
    for index in 0..30 {
        let execution = format!("chat-{index}");
        projection.event(&ExecutionEvent::new(
            ExecutionId::new(execution.as_str()).unwrap(),
            ExecutionUpdate::Message(MessageChunk::text("x".repeat(4000))),
        ));
    }
    let mut total = 0;
    for index in 0..8 {
        let ask = escaped_ask(&format!("asking-{index}"), "1", 3, 145);
        total += cost(&ask);
        projection.event(&ask);
    }
    assert!(total <= MAX_OPEN_ASK_COST, "{total}");
    assert!(
        total > MAX_OPEN_ASK_COST * 9 / 10,
        "the budget is nearly spent: {total}"
    );
    let view = projection.read();
    let bytes = serde_json::to_vec(&view).unwrap().len();
    assert!(bytes <= 60_000, "{bytes} bytes");
    assert_eq!(view.questions.len(), 8, "no admitted ask is given up");
}
