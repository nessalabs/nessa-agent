//! Projections are bounded display state, not permission or scheduling authority.
use super::{
    conversation_session, ConversationCaller, ConversationDependencies, ConversationLimits,
    ConversationModeRequest, ConversationModeRequestState, ConversationRepository,
    ConversationService, ProviderSessionErasers, RequestedConversation, SubmissionMode,
    SubmittedMessage,
};
use crate::conversation_test_support::{
    fixture, only, AcceptingCreationAudit, AcceptingDeletionAudit, AcceptingModeAudit,
    MemoryRepository, MemorySummaries, Provider, ProviderFactory, RecordingFileLinkAudit,
    RecordingModeAudit, TestClock, Unlisted, DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::conversation::domain::{ConversationApprovalMode, ConversationId};
use nessa_protocol::conversation::projection::{bound_view, Projection, MAX_VIEW_BYTES};
use nessa_protocol::conversation::tool_uis::McpToolUis;
use nessa_protocol::conversation::view::ConversationTranscriptState;
use nessa_protocol::conversation::view::{
    ConversationCapabilities, ConversationLifecyclePhase, ConversationMessageStatus,
    ConversationView,
};
use nessa_sdk::application::agent_execution::agents::{AgentError, ProviderDiagnostic};
use nessa_sdk::application::agent_execution::executions::{
    ExecutionEvent, ExecutionRequest, ExecutionUpdate, SubmissionMode as InvocationSubmissionMode,
};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::application::agent_execution::providers::{
    ExecutionReport, ObservationFailure, ObservationFailureCause, OperationCapabilities,
    ProviderExecutionReply, ProviderIdentity, ProviderSessionState,
};
use nessa_sdk::application::agent_execution::sessions::{
    CommittedCompleteness, CommittedFreshness, CommittedSession, CommittedStatus, InvocationRecord,
    MessageCommitClock, MessageCommitSleep, SessionSnapshot, SessionStorage,
    SubmissionAcknowledgement,
};
use nessa_sdk::domain::agent_execution::executions::{ExecutionId, ExecutionOutcome, MessageChunk};
use nessa_sdk::domain::agent_execution::prompts::{PromptText, UserMessage};
use nessa_sdk::domain::agent_execution::sessions::{
    ExecutionSessionId, ProviderContext, SessionId,
};
use nessa_sdk::domain::agent_execution::tools::{
    McpTool, ToolCallId, ToolCallUpdate, ToolContent, ToolStatus,
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
                environment: crate::conversation::infrastructure::in_process_environment().into(),
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
            environment: crate::conversation::infrastructure::in_process_environment().into(),
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

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}

pub(super) fn event(update: ExecutionUpdate) -> ExecutionEvent {
    ExecutionEvent::new(ExecutionId::new("execution").unwrap(), update)
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
        lease: None,
        artifacts: Vec::new(),
    }
}

pub(super) fn completed_snapshot(id: &str, events: Vec<ExecutionEvent>) -> SessionSnapshot {
    let mut snapshot = review_snapshot(events);
    snapshot.invocations[0].request.execution_id = ExecutionId::new(id).unwrap();
    snapshot.invocations[0].result = Some(Ok(ExecutionOutcome::Completed));
    snapshot
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
            environment: crate::conversation::infrastructure::in_process_environment().into(),
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
    fn resource_uri(&self, conversation: &ConversationId, call: &McpTool) -> Option<UiResourceUri> {
        let session = conversation_session(conversation);
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
            environment: crate::conversation::infrastructure::in_process_environment().into(),
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
        conversation_session(&id).as_str(),
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
