//! Projections are bounded display state, not permission or scheduling authority.
use super::{
    projection::{clipped, Projection, MAX_TEXT},
    ConversationAgentFeatures, ConversationAttachmentEvidenceFailure,
    ConversationAttachmentEvidenceFailureCode, ConversationCaller, ConversationCapabilities,
    ConversationDependencies, ConversationLifecycle, ConversationLifecyclePhase,
    ConversationLimits, ConversationMessageStatus, ConversationPendingMode, ConversationService,
    PermissionDenialSupport, SubmissionMode, SubmittedMessage,
};
use crate::{
    conversation::domain::ConversationId,
    conversation_test_support::{
        fixture, only, AcceptingCreationAudit, Provider, RecordingFileLinkAudit, TestClock,
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
