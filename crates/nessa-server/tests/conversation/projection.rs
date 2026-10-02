//! Projections are bounded display state, not permission or scheduling authority.
use super::projection::{bound_view, clipped, Projection, MAX_TEXT, MAX_VIEW_BYTES};
use super::view::ConversationTranscriptState;
use super::{
    retained_view, ConversationAgentFeatures, ConversationAttachmentEvidenceFailure,
    ConversationAttachmentEvidenceFailureCode, ConversationCaller, ConversationCapabilities,
    ConversationDependencies, ConversationLifecycle, ConversationLifecyclePhase,
    ConversationLimits, ConversationMessageStatus, ConversationModeRequest,
    ConversationModeRequestState, ConversationRepository, ConversationService, ConversationView,
    McpToolUis, PermissionDenialSupport, ProviderSessionErasers, RequestedConversation,
    SubmissionMode, SubmittedMessage, MAX_STRUCTURED_CONTENT_BYTES,
};
use crate::conversation::domain::{ConversationApprovalMode, ConversationId};
use crate::conversation_test_support::{
    fixture, only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit,
    MemoryRepository, MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit,
    RecordingModeAudit, TestClock, Unlisted, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::agents::{AgentError, ProviderDiagnostic};
use nessa_sdk::application::agent_execution::executions::{
    ExecutionController, ExecutionEvent, ExecutionRequest, ExecutionUpdate,
    SubmissionMode as InvocationSubmissionMode,
};
use nessa_sdk::application::agent_execution::permissions::{ActionContext, CancellationOrigin};
use nessa_sdk::application::agent_execution::providers::{
    ExecutionReport, ObservationFailure, ObservationFailureCause, OperationCapabilities,
    ProviderExecutionReply, ProviderIdentity, ProviderSessionState,
};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedCompleteness, CommittedFreshness, CommittedSession, CommittedStatus, InvocationRecord,
    InvocationSchedulingEvent, MessageCommitClock, MessageCommitSleep, QueueHistoryRecord,
    SessionSnapshot, SessionStorage, SubmissionAcknowledgement,
};
use nessa_sdk::application::agent_execution::tools::ToolReviewInput;
use nessa_sdk::domain::agent_execution::executions::{
    ExecutionId, ExecutionOutcome, InvocationKind, InvocationStage, MessageChunk, MessageId,
    QueueMutation, SchedulingCause,
};
use nessa_sdk::domain::agent_execution::permissions::{
    PermissionCancellationReason, PermissionDecision, PermissionEffect, PermissionId,
    PermissionOfferPolicy, PermissionOption, PermissionOptionId, PermissionOptions,
    PermissionScope,
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
use nessa_sdk::domain::mcp_apps::UiResourceUri;
use nessa_sdk::infrastructure::session_storage::{
    InMemoryStorage, RecordStorage, RuntimeMessageCommitClock,
};
use std::sync::atomic::Ordering;
use std::sync::{Arc, Mutex};
use std::time::Duration;
use tokio::sync::oneshot;
use uuid::Uuid;

/// Preparing an empty session commits Opened before provider attachment. This
/// must not look like absent history, which the frontend may reclaim on tab close.
#[tokio::test]
async fn prepared_empty_history_is_complete_before_provider_attachment_in_both_stores() {
    let directory = tempfile::tempdir().unwrap();
    let records = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    records.initialize().await.unwrap();
    let stores: [Arc<dyn SessionStorage>; 2] = [Arc::new(InMemoryStorage::new()), records];
    for storage in stores {
        let provider = Arc::new(ProviderFactory::default());
        let (release, gate) = oneshot::channel();
        *provider.open_gate.lock().unwrap() = Some(gate);
        let service = ConversationService::new(
            ConversationDependencies {
                agents: only(Arc::new(Provider::new(provider))),
                storage,
                metadata: Arc::new(MemoryRepository::default()),
                mode_audit: Arc::new(AcceptingModeAudit),
                creation_audit: Arc::new(AcceptingCreationAudit),
                file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
                attachments: None,
                summaries: Arc::new(MemorySummaries::default()),
                listing: Arc::new(Unlisted),
                deletion_audit: Arc::new(AcceptingDeletionAudit),
                provider_sessions: ProviderSessionErasers::default(),
                deletion_budgets: DELETION_BUDGETS,
                message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
                clock: Arc::new(TestClock),
            },
            ConversationLimits::default(),
            None,
        )
        .unwrap();
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        service
            .create(
                id.clone(),
                caller("create-empty"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        let view = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let view = service
                    .read(id.clone(), caller("read-empty"))
                    .await
                    .unwrap();
                if view.transcript_state == ConversationTranscriptState::Complete {
                    break view;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        assert!(view.messages.is_empty());
        assert!(view.pending.is_empty());
        assert!(matches!(
            view.lifecycle.phase,
            ConversationLifecyclePhase::Starting
        ));
        release.send(()).unwrap();
        service.shutdown().await.unwrap();
    }
}

#[tokio::test]
async fn gateway_record_view_waits_for_message_commit() {
    struct HeldClock;
    impl MessageCommitClock for HeldClock {
        fn now(&self) -> Duration {
            Duration::ZERO
        }
        fn sleep_until(&self, _: Duration) -> MessageCommitSleep {
            Box::pin(std::future::pending())
        }
    }

    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let provider = Arc::new(ProviderFactory::default());
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Message(MessageChunk::text(
            "committed later",
        )));
    let (release, gate) = oneshot::channel();
    *provider.after_updates_gate.lock().unwrap() = Some(gate);
    let repository = Arc::new(MemoryRepository::default());
    let service = ConversationService::new(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider.clone()))),
            storage: storage.clone(),
            metadata: repository,
            mode_audit: Arc::new(AcceptingModeAudit),
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(HeldClock),
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
    )
    .unwrap();
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("create-record-view"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("send-record-view"),
            "request".into(),
            SubmittedMessage {
                text: "question".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    provider.updates_sent.notified().await;
    tokio::task::yield_now().await;
    let before = service
        .read(id.clone(), caller("before-commit"))
        .await
        .unwrap();
    assert_eq!(before.messages.len(), 1);
    assert!(before.messages[0].parts.is_empty());
    release.send(()).unwrap();
    let after = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let view = service
                .read(id.clone(), caller("after-commit"))
                .await
                .unwrap();
            if view.messages[0].status == ConversationMessageStatus::Completed {
                break view;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(after.messages[0]
        .parts
        .iter()
        .map(|part| part.text.as_str())
        .collect::<String>()
        .starts_with("committed later"));
    service.shutdown().await.unwrap();
    storage.shutdown().await.unwrap();
}
fn said(text: &str) -> UserMessage {
    UserMessage::text_only(PromptText::new(text).unwrap())
}
fn projection() -> Projection {
    projection_for("conversation")
}

/// A conversation id as the gateway makes them: what an MCP tool's UI is
/// looked up under (`conversation_session`).
const MCP_CONVERSATION: &str = "00000000-0000-4000-8000-0000000000aa";

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

fn completed_snapshot(id: &str, events: Vec<ExecutionEvent>) -> SessionSnapshot {
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

fn assert_partial_tool(view: &ConversationView, expected: bool) {
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
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    service
        .create(
            id.clone(),
            caller("create"),
            crate::conversation::application::RequestedConversation::default(),
        )
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
            mode_audit: Arc::new(AcceptingModeAudit),

            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
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

fn committed_tool_view(events: &[ExecutionEvent]) -> ConversationView {
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

/// The tools a server listed, as the view's lookup sees them: `charts`'
/// `show` has whatever UI the test sets in the conversation's own session,
/// nothing else has any, and no other conversation's session has it.
struct ListedUis {
    /// The conversation whose session lists it, as its SDK session is named.
    conversation: String,
    uri: Mutex<Option<String>>,
    /// Every session the view asked about.
    asked: Mutex<Vec<SessionId>>,
}
impl ListedUis {
    fn of(conversation: &str, uri: Option<String>) -> Self {
        Self {
            conversation: conversation.into(),
            uri: Mutex::new(uri),
            asked: Mutex::default(),
        }
    }
}
impl McpToolUis for ListedUis {
    fn resource_uri(&self, session: &SessionId, call: &McpTool) -> Option<UiResourceUri> {
        self.asked.lock().unwrap().push(session.clone());
        if session.as_str() != self.conversation {
            return None;
        }
        if (call.server(), call.tool()) != ("charts", "show") {
            return None;
        }
        self.uri
            .lock()
            .unwrap()
            .as_ref()
            .map(|uri| UiResourceUri::new(uri.as_str()).unwrap())
    }
}

fn mcp_event(id: &str, server: &str, tool: &str) -> ExecutionEvent {
    event(ExecutionUpdate::Tool(
        ToolCallUpdate::new(ToolCallId::new(id).unwrap(), None, None, None, None, None)
            .with_mcp_tool(McpTool::new(server, tool).unwrap()),
    ))
}

#[test]
fn an_mcp_tools_ui_comes_from_the_listed_tools_and_moves_the_revision() {
    let listed = Arc::new(ListedUis::of(MCP_CONVERSATION, None));
    let mut projection = projection_for(MCP_CONVERSATION).with_tool_uis(listed.clone());
    let snapshot = completed_snapshot(
        "execution",
        vec![
            mcp_event("chart", "charts", "show"),
            mcp_event("report", "charts", "report"),
            event(ExecutionUpdate::Tool(ToolCallUpdate::new(
                ToolCallId::new("shell").unwrap(),
                Some("Shell".into()),
                None,
                None,
                None,
                None,
            ))),
        ],
    );
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        None
    ));
    // Not listed yet: no UI, and the wire says nothing about one.
    let unknown = projection.read();
    assert_eq!(unknown.tools[0].mcp.as_ref().unwrap().resource_uri, None);
    let wire = serde_json::to_value(&unknown.tools[0]).unwrap();
    assert_eq!(
        wire["mcp"],
        serde_json::json!({ "server": "charts", "tool": "show" })
    );

    // Listed later: the same projection's next read has it, under a new revision.
    *listed.uri.lock().unwrap() = Some("ui://charts/show.html".into());
    let known = projection.read();
    let mcp = known.tools[0].mcp.as_ref().unwrap();
    assert_eq!(mcp.resource_uri.as_deref(), Some("ui://charts/show.html"));
    assert_eq!(
        serde_json::to_value(&known.tools[0]).unwrap()["mcp"]["resourceUri"],
        "ui://charts/show.html"
    );
    assert_ne!(known.revision, unknown.revision);
    assert_eq!(projection.read().revision, known.revision);
    // A tool without UI, and a tool that is not MCP's, are as they were.
    assert_eq!(known.tools[1].mcp.as_ref().unwrap().resource_uri, None);
    assert_eq!(known.tools[2].mcp, None);

    // A committed replacement retains the injected lookup, and later changes
    // in cached UI metadata revise that committed view without live events.
    assert!(projection.replace_committed(
        &committed("incarnation", 2, 2, 2, Some(&snapshot)),
        &[],
        None
    ));
    assert_eq!(
        projection.read().tools[0]
            .mcp
            .as_ref()
            .unwrap()
            .resource_uri
            .as_deref(),
        Some("ui://charts/show.html")
    );
    let replaced_revision = projection.read().revision;
    *listed.uri.lock().unwrap() = Some("ui://charts/other.html".into());
    let changed_revision = projection.read().revision;
    assert_ne!(changed_revision, replaced_revision);
    *listed.uri.lock().unwrap() = None;
    let removed = projection.read();
    assert_ne!(removed.revision, changed_revision);
    assert_eq!(removed.tools[0].mcp.as_ref().unwrap().resource_uri, None);
    assert_eq!(
        serde_json::to_value(&removed.tools[0]).unwrap()["mcp"],
        serde_json::json!({"server":"charts","tool":"show"})
    );
    // Without a lookup, no UI.
    let mut bare = Projection::new("conversation".into(), unknown.capabilities.clone(), None);
    assert!(bare.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        None
    ));
    assert_eq!(
        bare.read().tools[0].mcp.as_ref().unwrap().resource_uri,
        None
    );
}

#[test]
fn committed_mcp_ui_enrichment_remains_inside_the_complete_view_budget() {
    let uri = format!("ui://{}", "a".repeat(2043));
    let listed = Arc::new(ListedUis::of(MCP_CONVERSATION, Some(uri.clone())));
    let events = (0..16)
        .map(|index| {
            event(ExecutionUpdate::Tool(
                ToolCallUpdate::new(
                    ToolCallId::new(format!("tool-{index}")).unwrap(),
                    None,
                    None,
                    None,
                    None,
                    None,
                )
                .with_mcp_tool(McpTool::new("charts", "show").unwrap())
                .with_content(vec![ToolContent::text("\"".repeat(6000))]),
            ))
        })
        .collect();
    let snapshot = completed_snapshot("execution", events);
    let mut projection = projection_for(MCP_CONVERSATION).with_tool_uis(listed);
    assert!(projection.replace_committed(
        &committed("incarnation", 1, 1, 1, Some(&snapshot)),
        &[],
        None
    ));
    let enriched = projection.read();
    assert!(serde_json::to_vec(&enriched).unwrap().len() > MAX_VIEW_BYTES);
    let bounded = bound_view(enriched);
    assert!(bounded.truncated);
    assert!(serde_json::to_vec(&bounded).unwrap().len() <= MAX_VIEW_BYTES);
    assert!(bounded
        .tools
        .iter()
        .any(|tool| tool.mcp.as_ref().unwrap().resource_uri.as_deref() == Some(uri.as_str())));
    assert_eq!(
        snapshot.invocations[0].events.len(),
        16,
        "display omission does not rewrite the committed fold"
    );
}

fn service_with_cached_tool_uis(
    storage: Arc<InMemoryStorage>,
    repository: Arc<MemoryRepository>,
    provider: Arc<ProviderFactory>,
    audit: Arc<RecordingModeAudit>,
    listed: Arc<ListedUis>,
) -> ConversationService {
    ConversationService::with_tool_uis(
        ConversationDependencies {
            agents: only(Arc::new(Provider::new(provider))),
            storage,
            metadata: repository,
            mode_audit: audit,
            creation_audit: Arc::new(AcceptingCreationAudit),
            file_link_audit: Arc::new(RecordingFileLinkAudit::default()),
            attachments: None,
            summaries: Arc::new(MemorySummaries::default()),
            listing: Arc::new(Unlisted),
            deletion_audit: Arc::new(AcceptingDeletionAudit),
            provider_sessions: ProviderSessionErasers::default(),
            deletion_budgets: DELETION_BUDGETS,
            message_commit_clock: Arc::new(RuntimeMessageCommitClock::new()),
            clock: Arc::new(TestClock),
        },
        ConversationLimits::default(),
        None,
        listed,
    )
    .unwrap()
}

#[tokio::test]
async fn service_committed_and_cold_pending_mode_views_preserve_cached_mcp_ui() {
    let storage = Arc::new(InMemoryStorage::new());
    let repository = Arc::new(MemoryRepository::default());
    let provider = Arc::new(ProviderFactory::default());
    provider
        .execution_updates
        .lock()
        .unwrap()
        .push(ExecutionUpdate::Tool(
            ToolCallUpdate::new(
                ToolCallId::new("chart").unwrap(),
                None,
                None,
                None,
                None,
                None,
            )
            .with_mcp_tool(McpTool::new("charts", "show").unwrap()),
        ));
    let audit = Arc::new(RecordingModeAudit::default());
    let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
    let listed = Arc::new(ListedUis::of(
        crate::conversation::application::conversation_session(&id).as_str(),
        Some("ui://charts/show.html".into()),
    ));
    let service = service_with_cached_tool_uis(
        storage.clone(),
        repository.clone(),
        provider.clone(),
        audit.clone(),
        listed.clone(),
    );
    service
        .create(
            id.clone(),
            caller("create-ui"),
            RequestedConversation::default(),
        )
        .await
        .unwrap();
    service
        .submit(
            id.clone(),
            caller("send-ui"),
            "ui-request".into(),
            SubmittedMessage {
                text: "show".into(),
                ..SubmittedMessage::default()
            },
            SubmissionMode::Queue,
        )
        .await
        .unwrap();
    let live = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let view = service.read(id.clone(), caller("live-ui")).await.unwrap();
            if view
                .messages
                .first()
                .is_some_and(|message| message.status == ConversationMessageStatus::Completed)
            {
                break view;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let live_uri = live.tools[0].mcp.as_ref().unwrap().resource_uri.clone();
    service.shutdown().await.unwrap();
    repository
        .begin_mode_change(ConversationModeRequest {
            conversation_id: id.clone(),
            organization_id: OrganizationId::new("org").unwrap(),
            request_id: "pending-mode-ui".into(),
            initiator_principal_id: PrincipalId::new("person").unwrap(),
            initiator_surface_id: "panel".into(),
            prior: ConversationApprovalMode::Ask,
            requested: ConversationApprovalMode::Auto,
            state: ConversationModeRequestState::Pending,
            application: None,
            requested_at_ms: 1_700_000_000_124,
        })
        .await
        .unwrap();
    // Refuse recovery audit before a cold Agent can be resolved; the pending
    // view must therefore use the service's committed, read-only constructor.
    audit.fail_application_once.store(true, Ordering::SeqCst);
    let opens = provider.open_calls.load(Ordering::SeqCst);
    let cold =
        service_with_cached_tool_uis(storage, repository, provider.clone(), audit, listed.clone());
    let pending = cold.read(id, caller("pending-ui")).await.unwrap();
    let cold_uri = pending.tools[0].mcp.as_ref().unwrap().resource_uri.clone();
    let later_opens = provider.open_calls.load(Ordering::SeqCst);
    cold.shutdown().await.unwrap();
    assert_eq!(live_uri.as_deref(), Some("ui://charts/show.html"));
    // The view asks about the very session the agent was opened in — the
    // one its MCP grants are issued for — and no other.
    let opened: Vec<_> = provider
        .opened_sessions
        .lock()
        .unwrap()
        .iter()
        .flatten()
        .cloned()
        .collect();
    assert!(!opened.is_empty());
    let asked = listed.asked.lock().unwrap().clone();
    assert!(!asked.is_empty());
    assert!(
        asked.iter().all(|session| opened.contains(session)),
        "{asked:?} vs {opened:?}"
    );
    assert_eq!(cold_uri.as_deref(), Some("ui://charts/show.html"));
    assert_eq!(
        later_opens, opens,
        "cold pending view does not open a provider"
    );
    assert!(!pending.capabilities.queue);
    assert!(serde_json::to_vec(&pending).unwrap().len() <= MAX_VIEW_BYTES);
}
