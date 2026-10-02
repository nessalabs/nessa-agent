//! An MCP App's calls through the conversation service (#348): one test per
//! row of the "MCP App call" state table — each policy refusal, a call sent
//! at once, a destructive call allowed, denied, cancelled, expired and
//! withdrawn (by its request going, its mount being released, and its
//! conversation ending), a server's failures, a resource held behind a
//! ticket — and the audit each leaves.
use super::*;
use crate::conversation::application::app_reviews::{ALLOW, DENY, MAX_OPEN_APP_REVIEWS};
use crate::conversation::application::mcp_apps::{
    McpAppAudit, McpAppFailure, McpAppFuture, McpApps, ResourceTickets,
};
use crate::conversation::application::view::ConversationPermissionOrigin;
use crate::conversation::application::{
    ConversationDependencies, ConversationFuture, ConversationLimits, McpToolUis,
    ProviderSessionErasers, RequestedConversation, SubmissionMode, SubmittedMessage,
};
use crate::conversation::domain::ConversationId;
use crate::conversation_test_support::{
    only, AcceptingCreationAudit, AcceptingDeletionAudit, MemoryRepository, MemorySummaries,
    Provider, ProviderFactory, RecordingFileLinkAudit, RecordingModeAudit, TestClock, Unlisted,
    DELETION_BUDGETS,
};
use crate::mcp_servers::domain::MAX_APP_ARGUMENTS_BYTES;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::agents::AgentError;
use nessa_sdk::application::agent_execution::executions::ExecutionUpdate;
use nessa_sdk::domain::agent_execution::tools::{ToolCallId, ToolCallUpdate};
use nessa_sdk::domain::mcp_apps::{ListedTool, ToolHints, ToolUi, UiCsp, UiResource, UiVisibility};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use serde_json::json;
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Mutex;
use std::time::Duration;

const SERVER: &str = "charts";
const UI_TOOL: &str = "show";
const URI: &str = "ui://charts/show.html";
const INSTANCE: &str = "6f1d6c0e-8f8c-4a52-9b8e-1f6c3d2a4b5c";
const OTHER_INSTANCE: &str = "0d6c3e7a-1b2c-4d5e-8f90-a1b2c3d4e5f6";

/// The UI the call `charts/show` declared, for the conversation's session.
struct Uis(String);
impl McpToolUis for Uis {
    fn resource_uri(&self, session: &SessionId, call: &McpTool) -> Option<UiResourceUri> {
        (session.as_str() == self.0 && (call.server(), call.tool()) == (SERVER, UI_TOOL))
            .then(|| UiResourceUri::new(URI).unwrap())
    }
}

/// The server's tools and answers, as its session gives them.
#[derive(Default)]
struct Apps {
    listed: Mutex<HashMap<String, ListedTool>>,
    /// Answers in turn; a plain `{content: []}` when none is left.
    answers: Mutex<Vec<Result<Value, McpAppFailure>>>,
    resource: Mutex<Option<Result<UiResource, McpAppFailure>>>,
    calls: Mutex<Vec<(String, Option<Value>)>>,
    /// While set, a call waits for the gate to open before it answers.
    hold: AtomicBool,
    gate: Gate,
}
/// Closed until a test opens it.
struct Gate(tokio::sync::Semaphore);
impl Default for Gate {
    fn default() -> Self {
        Self(tokio::sync::Semaphore::new(0))
    }
}
impl Apps {
    fn list(&self, tool: &str, ui: Option<UiVisibility>, hints: ToolHints) {
        let listed = ListedTool::new(
            McpTool::new(SERVER, tool).unwrap(),
            ui.map(|visibility| ToolUi::new(UiResourceUri::new(URI).unwrap(), visibility)),
        )
        .with_hints(hints);
        self.listed.lock().unwrap().insert(tool.into(), listed);
    }
    fn calls(&self) -> usize {
        self.calls.lock().unwrap().len()
    }
}
impl McpApps for Apps {
    fn listed_tool(
        &self,
        _: &SessionId,
        server: &str,
        name: &str,
    ) -> Result<Option<ListedTool>, McpAppFailure> {
        Ok((server == SERVER)
            .then(|| self.listed.lock().unwrap().get(name).cloned())
            .flatten())
    }
    fn call_tool<'a>(
        &'a self,
        _: &'a SessionId,
        _: &'a str,
        name: &'a str,
        arguments: Option<Value>,
    ) -> McpAppFuture<'a, Value> {
        Box::pin(async move {
            self.calls.lock().unwrap().push((name.into(), arguments));
            if self.hold.load(Ordering::SeqCst) {
                drop(self.gate.0.acquire().await.unwrap());
            }
            let answer = self.answers.lock().unwrap().pop();
            answer.unwrap_or_else(|| Ok(json!({"content": []})))
        })
    }
    fn read_resource<'a>(
        &'a self,
        _: &'a SessionId,
        _: &'a str,
        _: &'a UiResourceUri,
    ) -> McpAppFuture<'a, UiResource> {
        Box::pin(async move {
            self.resource
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Err(McpAppFailure::NoSession))
        })
    }
}

#[derive(Default)]
struct Audit {
    records: Mutex<Vec<McpAppAuditRecord>>,
    failing: AtomicBool,
    /// Fails every record from this many taken on, when set.
    failing_after: Mutex<Option<usize>>,
}
impl Audit {
    fn phases(&self) -> Vec<McpAppAuditPhase> {
        self.records
            .lock()
            .unwrap()
            .iter()
            .map(|record| record.phase.clone())
            .collect()
    }
}
impl McpAppAudit for Audit {
    fn record(&self, record: McpAppAuditRecord) -> ConversationFuture<'_, ()> {
        let taken = self.records.lock().unwrap().len();
        let failing = self.failing.load(Ordering::SeqCst)
            || self
                .failing_after
                .lock()
                .unwrap()
                .is_some_and(|after| taken >= after);
        if !failing {
            self.records.lock().unwrap().push(record);
        }
        Box::pin(async move {
            if failing {
                Err(ConversationError::Audit)
            } else {
                Ok(())
            }
        })
    }
}

#[derive(Default)]
struct Tickets {
    issued: Mutex<Vec<HeldResource>>,
    released_apps: Mutex<Vec<McpAppRef>>,
    released_conversations: AtomicUsize,
    discarded: Mutex<Vec<String>>,
    full: AtomicBool,
}
impl ResourceTickets for Tickets {
    fn issue(&self, resource: HeldResource) -> Result<String, TicketRefusal> {
        if self.full.load(Ordering::SeqCst) {
            return Err(TicketRefusal::Capacity);
        }
        self.issued.lock().unwrap().push(resource);
        Ok("t".repeat(43))
    }
    fn release_conversation(&self, _: &ConversationId) {
        self.released_conversations.fetch_add(1, Ordering::SeqCst);
    }
    fn release_app(&self, _: &ConversationId, app: &McpAppRef) {
        self.released_apps.lock().unwrap().push(app.clone());
    }
    fn discard(&self, ticket: &str) {
        self.discarded.lock().unwrap().push(ticket.into());
    }
}

struct Fixture {
    service: ConversationService,
    id: ConversationId,
    apps: Arc<Apps>,
    audit: Arc<Audit>,
    tickets: Arc<Tickets>,
    /// The tool call whose UI the app is.
    execution_id: String,
    tool_id: String,
}
impl Fixture {
    /// A conversation whose one turn called `charts/show`, a tool with a
    /// UI, and finished.
    async fn new() -> Self {
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
                .with_mcp_tool(McpTool::new(SERVER, UI_TOOL).unwrap()),
            ));
        let id = ConversationId::new(&Uuid::new_v4().to_string()).unwrap();
        let apps = Arc::new(Apps::default());
        let audit = Arc::new(Audit::default());
        let tickets = Arc::new(Tickets::default());
        let repository = Arc::new(MemoryRepository::default());
        let service = ConversationService::with_mcp_apps(
            ConversationDependencies {
                agents: only(Arc::new(Provider::new(provider))),
                storage: Arc::new(InMemoryStorage::new()),
                metadata: repository,
                mode_audit: Arc::new(RecordingModeAudit::default()),
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
            Arc::new(Uis(conversation_session(&id).as_str().to_owned())),
            McpAppPorts {
                apps: apps.clone(),
                audit: audit.clone(),
                tickets: tickets.clone(),
            },
        )
        .unwrap();
        service
            .create(
                id.clone(),
                caller("create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        service
            .submit(
                id.clone(),
                caller("send"),
                "send".into(),
                SubmittedMessage {
                    text: "show me".into(),
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        let view = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let view = service.read(id.clone(), caller("read")).await.unwrap();
                if view.messages.first().is_some_and(|message| {
                    message.status
                        == crate::conversation::application::ConversationMessageStatus::Completed
                }) {
                    break view;
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        let tool = &view.tools[0];
        assert_eq!(
            tool.mcp.as_ref().unwrap().resource_uri.as_deref(),
            Some(URI)
        );
        // A tool for the app to call that reads, and one that deletes.
        apps.list(
            "read_rows",
            Some(UiVisibility::new(true, true)),
            ToolHints::new(Some(true), None),
        );
        apps.list("delete_rows", None, ToolHints::new(None, None));
        Self {
            execution_id: tool.execution_id.clone(),
            tool_id: tool.tool_id.clone(),
            service,
            id,
            apps,
            audit,
            tickets,
        }
    }

    fn app(&self, instance: &str) -> McpAppRef {
        McpAppRef {
            execution_id: self.execution_id.clone(),
            tool_id: self.tool_id.clone(),
            instance_id: instance.into(),
        }
    }

    fn call(&self, tool: &str, arguments: Option<&str>) -> McpAppCall {
        McpAppCall {
            app: self.app(INSTANCE),
            server: SERVER.into(),
            tool: tool.into(),
            arguments_json: arguments.map(str::to_owned),
        }
    }

    async fn call_tool(&self, call: McpAppCall) -> Result<String, ConversationError> {
        self.service
            .call_app_tool(self.id.clone(), caller("app-call"), call)
            .await
    }

    /// The call started in the background, with the review it waits on once
    /// the conversation shows it.
    async fn held(
        &self,
        call: McpAppCall,
    ) -> (
        tokio::task::JoinHandle<Result<String, ConversationError>>,
        ConversationPermission,
    ) {
        let before = self.app_reviews().await.len();
        let service = self.service.clone();
        let id = self.id.clone();
        let task =
            tokio::spawn(async move { service.call_app_tool(id, caller("held"), call).await });
        let review = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let reviews = self.app_reviews().await;
                if reviews.len() > before {
                    break reviews.last().unwrap().clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap();
        (task, review)
    }

    async fn app_reviews(&self) -> Vec<ConversationPermission> {
        self.service
            .read(self.id.clone(), caller("read"))
            .await
            .unwrap()
            .permissions
            .into_iter()
            .filter(|review| matches!(review.origin, ConversationPermissionOrigin::App { .. }))
            .collect()
    }

    async fn answer(&self, review: &ConversationPermission, option: &str) {
        self.service
            .answer(
                self.id.clone(),
                caller("answer"),
                review.execution_id.clone(),
                review.permission_id.clone(),
                option.into(),
            )
            .await
            .unwrap();
    }
}

fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}

fn refused(result: Result<impl std::fmt::Debug, ConversationError>) -> McpAppError {
    match result {
        Err(ConversationError::McpApp(error)) => error,
        other => panic!("expected an app refusal, got {other:?}"),
    }
}

fn is_permission(phase: &McpAppAuditPhase) -> Option<&str> {
    match phase {
        McpAppAuditPhase::ApprovalRequested { permission_id }
        | McpAppAuditPhase::Approved { permission_id }
        | McpAppAuditPhase::Denied { permission_id }
        | McpAppAuditPhase::Expired { permission_id }
        | McpAppAuditPhase::Withdrawn { permission_id, .. } => Some(permission_id),
        _ => None,
    }
}

#[tokio::test]
async fn a_tool_that_only_reads_is_called_at_once_and_its_answer_returned_verbatim() {
    let fixture = Fixture::new().await;
    fixture.apps.answers.lock().unwrap().push(Ok(
        json!({"content": [{"type": "text", "text": "3 rows"}], "isError": true}),
    ));
    let answer = fixture
        .call_tool(fixture.call("read_rows", Some("{\"table\":\"t\"}")))
        .await
        .unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&answer).unwrap(),
        json!({"content": [{"type": "text", "text": "3 rows"}], "isError": true})
    );
    assert_eq!(
        fixture.apps.calls.lock().unwrap()[0],
        ("read_rows".into(), Some(json!({"table": "t"})))
    );
    let bytes = answer.len();
    assert_eq!(
        fixture.audit.phases(),
        [
            McpAppAuditPhase::Admitted,
            McpAppAuditPhase::Completed(McpAppOutcome::Answered {
                is_error: true,
                bytes
            }),
        ]
    );
    // Every step of one call shares the gateway's call id, and names the
    // app, its mount, its ask, and who it acted for.
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_eq!(records[0].call_id, records[1].call_id);
    assert_eq!(records[0].app, fixture.app(INSTANCE));
    assert_eq!(records[0].request_id, "app-call");
    assert_eq!(
        records[0].ask,
        McpAppAsk::CallTool {
            server: SERVER.into(),
            tool: "read_rows".into()
        }
    );
    assert_eq!(
        records[0].initiator,
        McpAppInitiator::App {
            principal_id: PrincipalId::new("person").unwrap(),
            surface_id: "panel".into()
        }
    );
    // A second call is a second call id, even under the same request id.
    fixture
        .call_tool(fixture.call("read_rows", None))
        .await
        .unwrap();
    let records = fixture.audit.records.lock().unwrap().clone();
    assert_ne!(records[2].call_id, records[0].call_id);
}

#[tokio::test]
async fn each_policy_refusal_is_its_code_on_record_and_nothing_is_sent() {
    let fixture = Fixture::new().await;
    fixture.apps.list(
        "model_only",
        Some(UiVisibility::new(true, false)),
        ToolHints::new(Some(true), None),
    );
    let not_this_app = McpAppCall {
        app: McpAppRef {
            tool_id: "no-such-call".into(),
            ..fixture.app(INSTANCE)
        },
        ..fixture.call("read_rows", None)
    };
    let another_server = McpAppCall {
        server: "files".into(),
        ..fixture.call("read_rows", None)
    };
    let past_the_bound = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_APP_ARGUMENTS_BYTES - 7));
    assert_eq!(past_the_bound.len(), MAX_APP_ARGUMENTS_BYTES + 1);
    for (call, expected) in [
        (not_this_app, McpAppError::AppUnknown),
        (another_server, McpAppError::ServerMismatch),
        (fixture.call("not_listed", None), McpAppError::ToolNotForApp),
        (fixture.call("model_only", None), McpAppError::ToolNotForApp),
        (
            fixture.call("read_rows", Some(&past_the_bound)),
            McpAppError::RequestTooLarge,
        ),
    ] {
        assert_eq!(refused(fixture.call_tool(call).await), expected);
        assert_eq!(
            fixture.audit.phases().last(),
            Some(&McpAppAuditPhase::Refused(expected.code()))
        );
    }
    // Not a JSON object: refused as invalid, on record too.
    assert!(matches!(
        fixture
            .call_tool(fixture.call("read_rows", Some("[1]")))
            .await,
        Err(ConversationError::InvalidInput)
    ));
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Refused("invalid_request"))
    );
    assert_eq!(fixture.apps.calls(), 0);
    // Exactly at the bound is taken.
    let at_the_bound = format!("{{\"a\":\"{}\"}}", "x".repeat(MAX_APP_ARGUMENTS_BYTES - 8));
    assert_eq!(at_the_bound.len(), MAX_APP_ARGUMENTS_BYTES);
    fixture
        .call_tool(fixture.call("read_rows", Some(&at_the_bound)))
        .await
        .unwrap();
}

#[tokio::test]
async fn a_destructive_call_waits_on_the_persons_review_and_is_sent_once_allowed() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture
        .held(fixture.call("delete_rows", Some("{\"id\":1}")))
        .await;
    // Shown beside the agent's, naming the app's tool and what it asked.
    assert_eq!(review.execution_id, fixture.execution_id);
    assert_eq!(review.tool_id, fixture.tool_id);
    assert_eq!(review.arguments_json, "{\"id\":1}");
    assert_eq!(
        review.origin,
        ConversationPermissionOrigin::App {
            server: SERVER.into(),
            tool: "delete_rows".into()
        }
    );
    assert_eq!(fixture.apps.calls(), 0, "nothing is sent while it waits");
    fixture.answer(&review, ALLOW).await;
    task.await.unwrap().unwrap();
    assert_eq!(fixture.apps.calls(), 1);
    assert!(fixture.app_reviews().await.is_empty());
    let records = fixture.audit.records.lock().unwrap().clone();
    let phases: Vec<_> = records.iter().map(|record| &record.phase).collect();
    assert!(matches!(
        phases[..],
        [
            McpAppAuditPhase::ApprovalRequested { .. },
            McpAppAuditPhase::Approved { .. },
            McpAppAuditPhase::Completed(McpAppOutcome::Answered { .. }),
        ]
    ));
    assert_eq!(
        is_permission(phases[0]),
        Some(review.permission_id.as_str())
    );
    // The approval is the person's, answering by its own request.
    assert_eq!(
        records[1].initiator,
        McpAppInitiator::Person {
            principal_id: PrincipalId::new("person").unwrap(),
            surface_id: "panel".into(),
            request_id: "answer".into()
        }
    );
    // Answered again: stale, and nothing more happens.
    let again = fixture
        .service
        .answer(
            fixture.id.clone(),
            caller("again"),
            review.execution_id.clone(),
            review.permission_id.clone(),
            DENY.into(),
        )
        .await;
    assert!(matches!(
        again,
        Err(ConversationError::Agent(AgentError::StalePermission))
    ));
    assert_eq!(fixture.apps.calls(), 1);
}

#[tokio::test]
async fn a_denied_or_cancelled_review_ends_its_call_denied_and_nothing_is_sent() {
    let fixture = Fixture::new().await;
    let (denied, review) = fixture.held(fixture.call("delete_rows", None)).await;
    fixture.answer(&review, DENY).await;
    assert_eq!(refused(denied.await.unwrap()), McpAppError::ApprovalDenied);
    let (cancelled, review) = fixture.held(fixture.call("delete_rows", None)).await;
    fixture
        .service
        .cancel_permission(
            fixture.id.clone(),
            caller("cancel"),
            review.execution_id.clone(),
            review.permission_id.clone(),
            "no".into(),
        )
        .await
        .unwrap();
    assert_eq!(
        refused(cancelled.await.unwrap()),
        McpAppError::ApprovalDenied
    );
    assert_eq!(fixture.apps.calls(), 0);
    let denials = fixture
        .audit
        .phases()
        .into_iter()
        .filter(|phase| matches!(phase, McpAppAuditPhase::Denied { .. }))
        .count();
    assert_eq!(denials, 2);
}

#[tokio::test]
async fn a_review_nobody_answers_expires_and_its_call_is_refused_expired() {
    let fixture = Fixture::new().await;
    let (task, _review) = fixture.held(fixture.call("delete_rows", None)).await;
    tokio::time::pause();
    tokio::time::advance(APP_REVIEW_DEADLINE + Duration::from_secs(1)).await;
    tokio::time::resume();
    assert_eq!(refused(task.await.unwrap()), McpAppError::ApprovalExpired);
    assert_eq!(fixture.apps.calls(), 0);
    assert!(fixture.app_reviews().await.is_empty());
    let records = fixture.audit.records.lock().unwrap().clone();
    let expired = records.last().unwrap();
    assert!(matches!(expired.phase, McpAppAuditPhase::Expired { .. }));
    assert_eq!(expired.initiator, McpAppInitiator::System);
}

#[tokio::test]
async fn a_call_whose_caller_went_withdraws_its_review_on_record() {
    let fixture = Fixture::new().await;
    let (task, review) = fixture.held(fixture.call("delete_rows", None)).await;
    task.abort();
    let _ = task.await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !fixture.app_reviews().await.is_empty()
            || !matches!(
                fixture.audit.phases().last(),
                Some(McpAppAuditPhase::Withdrawn { .. })
            )
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Withdrawn {
            permission_id: review.permission_id.clone(),
            cause: McpAppWithdrawal::RequestCancelled
        })
    );
    assert_eq!(fixture.apps.calls(), 0);
}

#[tokio::test]
async fn releasing_one_mount_cancels_only_its_own_calls_and_lets_go_of_its_resources() {
    let fixture = Fixture::new().await;
    let (mine, _) = fixture.held(fixture.call("delete_rows", None)).await;
    let (other, other_review) = fixture
        .held(McpAppCall {
            app: fixture.app(OTHER_INSTANCE),
            ..fixture.call("delete_rows", None)
        })
        .await;
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
    assert_eq!(refused(mine.await.unwrap()), McpAppError::Cancelled);
    assert_eq!(
        fixture.tickets.released_apps.lock().unwrap().clone(),
        [fixture.app(INSTANCE)]
    );
    // The other mount still waits, and is answered as it is answered.
    let left = fixture.app_reviews().await;
    assert_eq!(left.len(), 1);
    assert_eq!(left[0].permission_id, other_review.permission_id);
    fixture.answer(&other_review, ALLOW).await;
    other.await.unwrap().unwrap();
    assert!(fixture.audit.phases().iter().any(|phase| matches!(
        phase,
        McpAppAuditPhase::Withdrawn {
            cause: McpAppWithdrawal::AppTornDown,
            ..
        }
    )));
    // Releasing it again, or a mount with nothing open, is nothing.
    fixture
        .service
        .release_app(fixture.id.clone(), caller("release"), fixture.app(INSTANCE))
        .await
        .unwrap();
}

#[tokio::test]
async fn closing_the_conversation_cancels_every_waiting_call_and_its_resources() {
    let fixture = Fixture::new().await;
    let (first, _) = fixture.held(fixture.call("delete_rows", None)).await;
    let (second, _) = fixture
        .held(McpAppCall {
            app: fixture.app(OTHER_INSTANCE),
            ..fixture.call("delete_rows", None)
        })
        .await;
    fixture
        .service
        .close(fixture.id.clone(), caller("close"))
        .await
        .unwrap();
    for task in [first, second] {
        assert_eq!(refused(task.await.unwrap()), McpAppError::Cancelled);
    }
    assert_eq!(
        fixture
            .tickets
            .released_conversations
            .load(Ordering::SeqCst),
        1
    );
    let withdrawals: Vec<_> = fixture
        .audit
        .records
        .lock()
        .unwrap()
        .iter()
        .filter(|record| {
            matches!(
                record.phase,
                McpAppAuditPhase::Withdrawn {
                    cause: McpAppWithdrawal::ConversationEnded,
                    ..
                }
            )
        })
        .map(|record| record.initiator.clone())
        .collect();
    assert_eq!(
        withdrawals,
        [McpAppInitiator::System, McpAppInitiator::System]
    );
    assert_eq!(fixture.apps.calls(), 0);
}

#[tokio::test]
async fn past_the_open_review_cap_a_destructive_call_is_refused_unavailable() {
    let fixture = Fixture::new().await;
    let mut held = Vec::new();
    for _ in 0..MAX_OPEN_APP_REVIEWS {
        held.push(fixture.held(fixture.call("delete_rows", None)).await.0);
    }
    assert!(matches!(
        fixture.call_tool(fixture.call("delete_rows", None)).await,
        Err(ConversationError::Unavailable)
    ));
    for task in held {
        task.abort();
    }
}

#[tokio::test]
async fn a_servers_failure_is_its_code_and_on_record_as_completed() {
    let fixture = Fixture::new().await;
    let huge = json!({"content": [{"type": "text", "text": "x".repeat(MAX_APP_RESULT_BYTES)}]});
    for (answer, expected) in [
        (
            Err(McpAppFailure::Remote {
                code: -32602,
                message: "bad".into(),
            }),
            McpAppError::Remote(Some((-32602, "bad".into()))),
        ),
        (Err(McpAppFailure::TimedOut), McpAppError::TimedOut),
        (
            Err(McpAppFailure::SessionEnded),
            McpAppError::SessionUnavailable,
        ),
        (Err(McpAppFailure::Malformed), McpAppError::Remote(None)),
        (Ok(huge), McpAppError::ResultTooLarge),
    ] {
        fixture.apps.answers.lock().unwrap().push(answer);
        let code = expected.code();
        assert_eq!(
            refused(fixture.call_tool(fixture.call("read_rows", None)).await),
            expected
        );
        assert_eq!(
            fixture.audit.phases().last(),
            Some(&McpAppAuditPhase::Completed(McpAppOutcome::Failed(code)))
        );
    }
}

#[tokio::test]
async fn a_step_that_cannot_be_recorded_is_not_taken() {
    let fixture = Fixture::new().await;
    fixture.audit.failing.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture.call_tool(fixture.call("read_rows", None)).await,
        Err(ConversationError::Audit)
    ));
    assert!(matches!(
        fixture.call_tool(fixture.call("delete_rows", None)).await,
        Err(ConversationError::Audit)
    ));
    assert_eq!(fixture.apps.calls(), 0);
    assert!(fixture.app_reviews().await.is_empty(), "no review shown");
}

#[tokio::test]
async fn a_resource_is_read_once_and_held_behind_a_ticket_on_record() {
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        "<p>chart</p>".into(),
        UiCsp::new(
            vec![],
            vec!["https://cdn.example.com".into()],
            vec![],
            vec![],
        )
        .unwrap(),
        UiPermissions::default(),
        None,
        Some(true),
    )
    .unwrap()));
    let read = McpAppRead {
        app: fixture.app(INSTANCE),
        server: SERVER.into(),
        uri: URI.into(),
    };
    let resource = fixture
        .service
        .read_app_resource(fixture.id.clone(), caller("read-resource"), read.clone())
        .await
        .unwrap();
    assert_eq!(resource.size, "<p>chart</p>".len());
    assert_eq!(resource.sha256, hex(&Sha256::digest(b"<p>chart</p>")));
    assert_eq!(resource.csp.resource_domains().len(), 1);
    assert_eq!(resource.prefers_border, Some(true));
    let issued = fixture.tickets.issued.lock().unwrap().clone();
    assert_eq!(&*issued[0].bytes, b"<p>chart</p>");
    assert_eq!(issued[0].app(), &fixture.app(INSTANCE));
    let phases = fixture.audit.phases();
    assert_eq!(phases[0], McpAppAuditPhase::Admitted);
    assert_eq!(
        phases[2],
        McpAppAuditPhase::TicketIssued {
            // Only its digest is ever on record.
            ticket_digest: hex(&Sha256::digest(resource.ticket.as_bytes())),
            size: resource.size,
            sha256: resource.sha256.clone()
        }
    );
    // Another server's: refused. Not an app's HTML: app_unknown, on record.
    assert_eq!(
        refused(
            fixture
                .service
                .read_app_resource(
                    fixture.id.clone(),
                    caller("read-resource"),
                    McpAppRead {
                        server: "files".into(),
                        ..read.clone()
                    }
                )
                .await
        ),
        McpAppError::ServerMismatch
    );
    *fixture.apps.resource.lock().unwrap() = Some(Err(McpAppFailure::NotAnApp));
    assert_eq!(
        refused(
            fixture
                .service
                .read_app_resource(fixture.id.clone(), caller("read-resource"), read.clone())
                .await
        ),
        McpAppError::AppUnknown
    );
    // No room to hold it: refused unavailable, on record, nothing issued.
    *fixture.apps.resource.lock().unwrap() = Some(Ok(UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        "<p/>".into(),
        UiCsp::default(),
        UiPermissions::default(),
        None,
        None,
    )
    .unwrap()));
    fixture.tickets.full.store(true, Ordering::SeqCst);
    assert!(matches!(
        fixture
            .service
            .read_app_resource(fixture.id.clone(), caller("read-resource"), read)
            .await,
        Err(ConversationError::Unavailable)
    ));
    assert_eq!(
        fixture.audit.phases().last(),
        Some(&McpAppAuditPhase::Completed(McpAppOutcome::Failed(
            "temporarily_unavailable"
        )))
    );
}

#[tokio::test]
async fn calls_running_are_bounded_across_callers_until_each_task_ends() {
    // A caller that goes leaves its call running on its own task; the bound
    // counts that task, not the caller, so coming back cannot start more.
    let fixture = Fixture::new().await;
    fixture.apps.hold.store(true, Ordering::SeqCst);
    let mut callers = Vec::new();
    for _ in 0..MAX_APP_CALLS {
        let service = fixture.service.clone();
        let id = fixture.id.clone();
        let call = fixture.call("read_rows", None);
        callers.push(tokio::spawn(async move {
            service.call_app_tool(id, caller("held"), call).await
        }));
    }
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture.apps.calls() < MAX_APP_CALLS {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    for caller in &callers {
        caller.abort();
    }
    for caller in callers {
        let _ = caller.await;
    }
    // Every caller went; every call is still running, and still counted.
    assert!(matches!(
        fixture.call_tool(fixture.call("read_rows", None)).await,
        Err(ConversationError::Unavailable)
    ));
    fixture.apps.hold.store(false, Ordering::SeqCst);
    fixture.apps.gate.0.add_permits(1);
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .audit
            .phases()
            .iter()
            .filter(|phase| matches!(phase, McpAppAuditPhase::Completed(_)))
            .count()
            < MAX_APP_CALLS
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    // Each finished on its own and is on record; then there is room again.
    tokio::time::timeout(Duration::from_secs(5), async {
        while fixture
            .call_tool(fixture.call("read_rows", None))
            .await
            .is_err()
        {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn a_ticket_whose_issue_cannot_be_recorded_is_discarded_and_never_handed_out() {
    let fixture = Fixture::new().await;
    *fixture.apps.resource.lock().unwrap() = Some(Ok(UiResource::new(
        UiResourceUri::new(URI).unwrap(),
        "<p/>".into(),
        UiCsp::default(),
        UiPermissions::default(),
        None,
        None,
    )
    .unwrap()));
    // Admitted and completed are recorded; the issue is not.
    *fixture.audit.failing_after.lock().unwrap() = Some(2);
    let read = fixture
        .service
        .read_app_resource(
            fixture.id.clone(),
            caller("read-resource"),
            McpAppRead {
                app: fixture.app(INSTANCE),
                server: SERVER.into(),
                uri: URI.into(),
            },
        )
        .await;
    assert!(matches!(read, Err(ConversationError::Audit)));
    assert_eq!(fixture.tickets.issued.lock().unwrap().len(), 1);
    assert_eq!(
        fixture.tickets.discarded.lock().unwrap().clone(),
        ["t".repeat(43)]
    );
}
