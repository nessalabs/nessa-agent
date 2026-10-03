//! Substitutes for what an MCP App's calls go through — the conversation's
//! MCP session, the audit, the ticket store — and a conversation whose one
//! turn called a tool with a UI, for the service's tests and the socket's.
use crate::agents::domain::AgentId;
use crate::conversation::application::{
    conversation_session, ConversationCaller, ConversationDependencies, ConversationError,
    ConversationFuture, ConversationLimits, ConversationMessageStatus, ConversationPermission,
    ConversationPermissionOrigin, ConversationService, HeldResource, McpAppAudit, McpAppAuditPhase,
    McpAppAuditRecord, McpAppCall, McpAppFailure, McpAppFuture, McpAppInitiator, McpAppMessage,
    McpAppModelContext, McpAppPorts, McpAppRef, McpApps, McpToolUis, ProviderSessionErasers,
    RequestedConversation, ResourceTickets, SubmissionMode, SubmittedMessage, TicketEnd,
    TicketRefusal,
};
use crate::conversation::application::{ConversationAgent, ConversationAgents};
use crate::conversation::domain::ConversationId;
use crate::conversation_test_support::{
    AcceptingCreationAudit, AcceptingDeletionAudit, MemoryRepository, MemorySummaries, Provider,
    ProviderFactory, RecordingFileLinkAudit, RecordingModeAudit, TestClock, Unlisted,
    DELETION_BUDGETS,
};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_sdk::application::agent_execution::executions::ExecutionUpdate;
use nessa_sdk::domain::agent_execution::sessions::SessionId;
use nessa_sdk::domain::agent_execution::tools::{McpTool, ToolCallId, ToolCallUpdate};
use nessa_sdk::domain::mcp_apps::{
    ListedTool, ToolHints, ToolUi, UiResource, UiResourceUri, UiVisibility,
};
use nessa_sdk::infrastructure::session_storage::{InMemoryStorage, RuntimeMessageCommitClock};
use serde_json::{json, Value};
use std::collections::HashMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;
use uuid::Uuid;

pub(crate) const SERVER: &str = "charts";
pub(crate) const UI_TOOL: &str = "show";
pub(crate) const URI: &str = "ui://charts/show.html";
pub(crate) const INSTANCE: &str = "6f1d6c0e-8f8c-4a52-9b8e-1f6c3d2a4b5c";
pub(crate) const OTHER_INSTANCE: &str = "0d6c3e7a-1b2c-4d5e-8f90-a1b2c3d4e5f6";

/// The agent's own audit of what it was given: accepting, until told to fail.
#[derive(Default)]
pub(crate) struct ExecutionRecords {
    pub(crate) failing: AtomicBool,
}
impl nessa_sdk::application::agent_execution::executions::ExecutionAudit for ExecutionRecords {
    fn record(
        &self,
        _record: nessa_sdk::application::agent_execution::executions::ExecutionAuditRecord,
    ) -> nessa_sdk::application::agent_execution::agents::AgentFuture<'_, ()> {
        let failing = self.failing.load(Ordering::SeqCst);
        Box::pin(async move {
            if failing {
                Err(nessa_sdk::application::agent_execution::agents::AgentError::AuditFailure)
            } else {
                Ok(())
            }
        })
    }
}

/// The UI the call `charts/show` declared, for the conversation's session.
pub(crate) struct Uis(String);
impl McpToolUis for Uis {
    fn resource_uri(&self, session: &SessionId, call: &McpTool) -> Option<UiResourceUri> {
        (session.as_str() == self.0 && (call.server(), call.tool()) == (SERVER, UI_TOOL))
            .then(|| UiResourceUri::new(URI).unwrap())
    }
}

/// The server's tools and answers, as its session gives them.
#[derive(Default)]
pub(crate) struct Apps {
    pub(crate) listed: Mutex<HashMap<String, ListedTool>>,
    /// Answers in turn; a plain `{content: []}` when none is left.
    pub(crate) answers: Mutex<Vec<Result<Value, McpAppFailure>>>,
    pub(crate) resource: Mutex<Option<Result<UiResource, McpAppFailure>>>,
    pub(crate) calls: Mutex<Vec<(String, Option<Value>)>>,
    /// While set, a call waits for the gate to open before it answers.
    pub(crate) hold: AtomicBool,
    /// Resources asked for, held or not.
    pub(crate) reads: std::sync::atomic::AtomicUsize,
    pub(crate) gate: Gate,
    /// When set, every listing fails so.
    pub(crate) listing_fails: Mutex<Option<McpAppFailure>>,
}
/// Closed until a test opens it.
pub(crate) struct Gate(pub(crate) tokio::sync::Semaphore);
impl Default for Gate {
    fn default() -> Self {
        Self(tokio::sync::Semaphore::new(0))
    }
}
impl Apps {
    pub(crate) fn list(&self, tool: &str, ui: Option<UiVisibility>, hints: ToolHints) {
        let listed = ListedTool::new(
            McpTool::new(SERVER, tool).unwrap(),
            ui.map(|visibility| ToolUi::new(UiResourceUri::new(URI).unwrap(), visibility)),
        )
        .with_hints(hints);
        self.listed.lock().unwrap().insert(tool.into(), listed);
    }
    pub(crate) fn calls(&self) -> usize {
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
        if let Some(failure) = self.listing_fails.lock().unwrap().clone() {
            return Err(failure);
        }
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
            self.reads.fetch_add(1, Ordering::SeqCst);
            if self.hold.load(Ordering::SeqCst) {
                drop(self.gate.0.acquire().await.unwrap());
            }
            self.resource
                .lock()
                .unwrap()
                .clone()
                .unwrap_or(Err(McpAppFailure::NoSession))
        })
    }
}

#[derive(Default)]
pub(crate) struct Audit {
    pub(crate) records: Mutex<Vec<McpAppAuditRecord>>,
    pub(crate) failing: AtomicBool,
    /// Fails every record from this many taken on, when set.
    pub(crate) failing_after: Mutex<Option<usize>>,
    /// When set, every record takes this long to commit.
    pub(crate) slow: Mutex<Option<Duration>>,
    /// When set, the next record waits for the test to let it commit,
    /// saying first that it is waiting.
    pub(crate) hold: Mutex<Option<Hold>>,
}
/// A record held mid-commit until the test lets it go.
#[derive(Clone)]
pub(crate) struct Hold {
    /// Told when a record is waiting.
    pub(crate) waiting: Arc<tokio::sync::Notify>,
    /// One permit lets one waiting record commit.
    pub(crate) go: Arc<tokio::sync::Semaphore>,
}
impl Default for Hold {
    fn default() -> Self {
        Self {
            waiting: Arc::default(),
            go: Arc::new(tokio::sync::Semaphore::new(0)),
        }
    }
}
impl Audit {
    pub(crate) fn phases(&self) -> Vec<McpAppAuditPhase> {
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
        let slow = *self.slow.lock().unwrap();
        let hold = self.hold.lock().unwrap().take();
        Box::pin(async move {
            // On record only once committed.
            if let Some(slow) = slow {
                tokio::time::sleep(slow).await;
            }
            if let Some(hold) = hold {
                hold.waiting.notify_one();
                drop(hold.go.acquire().await.unwrap());
            }
            if failing {
                Err(ConversationError::Audit)
            } else {
                self.records.lock().unwrap().push(record);
                Ok(())
            }
        })
    }
}

#[derive(Default)]
pub(crate) struct Tickets {
    pub(crate) issued: Mutex<Vec<HeldResource>>,
    pub(crate) activated: Mutex<Vec<String>>,
    pub(crate) released_apps: Mutex<Vec<(McpAppRef, McpAppInitiator)>>,
    pub(crate) released_conversations: Mutex<Vec<McpAppInitiator>>,
    pub(crate) discarded: Mutex<Vec<String>>,
    pub(crate) full: AtomicBool,
    /// When set, the next activation answers that its ticket ended first.
    pub(crate) ended_first: Mutex<Option<(TicketEnd, McpAppInitiator)>>,
}
impl ResourceTickets for Tickets {
    fn issue(&self, resource: HeldResource) -> Result<String, TicketRefusal> {
        if self.full.load(Ordering::SeqCst) {
            return Err(TicketRefusal::Capacity);
        }
        self.issued.lock().unwrap().push(resource);
        Ok("t".repeat(43))
    }
    fn activate(&self, ticket: &str) -> Result<(), (TicketEnd, McpAppInitiator)> {
        if let Some(ended) = self.ended_first.lock().unwrap().take() {
            return Err(ended);
        }
        self.activated.lock().unwrap().push(ticket.into());
        Ok(())
    }
    fn discard(&self, ticket: &str) {
        self.discarded.lock().unwrap().push(ticket.into());
    }
    fn release_conversation(&self, _: &ConversationId, by: &McpAppInitiator) {
        self.released_conversations.lock().unwrap().push(by.clone());
    }
    fn release_app(&self, _: &ConversationId, app: &McpAppRef, by: &McpAppInitiator) {
        self.released_apps
            .lock()
            .unwrap()
            .push((app.clone(), by.clone()));
    }
}

pub(crate) struct Fixture {
    /// Whose conversation it is: every call the fixture makes is theirs.
    pub(crate) owner: ConversationCaller,
    pub(crate) service: ConversationService,
    pub(crate) id: ConversationId,
    pub(crate) apps: Arc<Apps>,
    pub(crate) audit: Arc<Audit>,
    pub(crate) tickets: Arc<Tickets>,
    /// The conversation's agent: what each turn it ran was given.
    pub(crate) provider: Arc<ProviderFactory>,
    /// The conversation's own record.
    pub(crate) repository: Arc<MemoryRepository>,
    /// The agent's audit of what it was given.
    pub(crate) execution_audit: Arc<ExecutionRecords>,
    /// The tool call whose UI the app is.
    pub(crate) execution_id: String,
    pub(crate) tool_id: String,
}
impl Fixture {
    /// A conversation whose one turn called `charts/show`, a tool with a
    /// UI, and finished.
    pub(crate) async fn new() -> Self {
        Self::for_owner(caller("fixture")).await
    }

    /// As [`Self::new`], in `owner`'s conversation.
    pub(crate) async fn for_owner(owner: ConversationCaller) -> Self {
        let as_owner = |action: &str| ConversationCaller {
            action_id: action.into(),
            ..owner.clone()
        };
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
        let execution_audit = Arc::new(ExecutionRecords::default());
        let service = ConversationService::with_mcp_apps(
            ConversationDependencies {
                agents: ConversationAgents::new(
                    HashMap::from([(
                        AgentId::Claude,
                        ConversationAgent {
                            provider: Arc::new(Provider::new(provider.clone())),
                            execution_audit: execution_audit.clone(),
                            reserved_output_tokens: 4096,
                            readiness: None,
                        },
                    )]),
                    AgentId::Claude,
                )
                .unwrap(),
                storage: Arc::new(InMemoryStorage::new()),
                metadata: repository.clone(),
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
                as_owner("create"),
                RequestedConversation::default(),
            )
            .await
            .unwrap();
        service
            .submit(
                id.clone(),
                as_owner("send"),
                "send".into(),
                SubmittedMessage {
                    text: "show me".into(),
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
        let view =
            tokio::time::timeout(Duration::from_secs(5), async {
                loop {
                    let view = service.read(id.clone(), as_owner("read")).await.unwrap();
                    if view.messages.first().is_some_and(|message| {
                        message.status == ConversationMessageStatus::Completed
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
            owner,
            execution_id: tool.execution_id.clone(),
            tool_id: tool.tool_id.clone(),
            service,
            id,
            apps,
            audit,
            tickets,
            provider,
            repository,
            execution_audit,
        }
    }

    /// The app at the mount `instance` sends `text` into the conversation.
    pub(crate) async fn send_message(
        &self,
        instance: &str,
        text: &str,
    ) -> Result<String, ConversationError> {
        // Each its own request: the same request again is the same turn.
        self.service
            .send_app_message(
                self.id.clone(),
                self.caller(&format!("app-message-{}", Uuid::new_v4())),
                McpAppMessage {
                    app: self.app(instance),
                    server: SERVER.into(),
                    text: text.into(),
                },
            )
            .await
    }

    /// The app at the mount `instance` gives the model `text` and
    /// `structured` content.
    pub(crate) async fn update_context(
        &self,
        instance: &str,
        text: Option<&str>,
        structured: Option<&str>,
    ) -> Result<(), ConversationError> {
        self.service
            .update_app_model_context(
                self.id.clone(),
                self.caller("app-context"),
                McpAppModelContext {
                    app: self.app(instance),
                    server: SERVER.into(),
                    text: text.map(str::to_owned),
                    structured_content_json: structured.map(str::to_owned),
                },
            )
            .await
    }

    /// The person sends `text`, as `execution`.
    pub(crate) async fn person_sends(&self, execution: &str, text: &str) {
        self.service
            .submit(
                self.id.clone(),
                self.caller(execution),
                execution.into(),
                SubmittedMessage {
                    text: text.into(),
                    ..SubmittedMessage::default()
                },
                SubmissionMode::Queue,
            )
            .await
            .unwrap();
    }

    /// The message the agent was given as `execution`, once it was.
    pub(crate) async fn given(
        &self,
        execution: &str,
    ) -> nessa_sdk::domain::agent_execution::prompts::UserMessage {
        let executions = || self.provider.executions.lock().unwrap().clone();
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                if let Some(at) = executions().iter().position(|id| id == execution) {
                    break self.provider.messages.lock().unwrap()[at].clone();
                }
                tokio::task::yield_now().await;
            }
        })
        .await
        .unwrap()
    }

    /// The app's message started in the background, with the review it
    /// waits on once the conversation shows it.
    pub(crate) async fn held_message(
        &self,
        instance: &str,
        text: &str,
    ) -> (
        tokio::task::JoinHandle<Result<String, ConversationError>>,
        ConversationPermission,
    ) {
        let before = self.app_reviews().await.len();
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller(&format!("held-message-{}", Uuid::new_v4()));
        let message = McpAppMessage {
            app: self.app(instance),
            server: SERVER.into(),
            text: text.into(),
        };
        let task = tokio::spawn(async move { service.send_app_message(id, caller, message).await });
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

    /// The owner, acting by `action`.
    pub(crate) fn caller(&self, action: &str) -> ConversationCaller {
        ConversationCaller {
            action_id: action.into(),
            ..self.owner.clone()
        }
    }

    pub(crate) fn app(&self, instance: &str) -> McpAppRef {
        McpAppRef {
            execution_id: self.execution_id.clone(),
            tool_id: self.tool_id.clone(),
            instance_id: instance.into(),
        }
    }

    pub(crate) fn call(&self, tool: &str, arguments: Option<&str>) -> McpAppCall {
        McpAppCall {
            app: self.app(INSTANCE),
            server: SERVER.into(),
            tool: tool.into(),
            arguments_json: arguments.map(str::to_owned),
        }
    }

    pub(crate) async fn call_tool(&self, call: McpAppCall) -> Result<String, ConversationError> {
        self.service
            .call_app_tool(self.id.clone(), self.caller("app-call"), call)
            .await
    }

    /// The call started in the background, with the review it waits on once
    /// the conversation shows it.
    pub(crate) async fn held(
        &self,
        call: McpAppCall,
    ) -> (
        tokio::task::JoinHandle<Result<String, ConversationError>>,
        ConversationPermission,
    ) {
        let before = self.app_reviews().await.len();
        let service = self.service.clone();
        let id = self.id.clone();
        let caller = self.caller("held");
        let task = tokio::spawn(async move { service.call_app_tool(id, caller, call).await });
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

    pub(crate) async fn app_reviews(&self) -> Vec<ConversationPermission> {
        self.service
            .read(self.id.clone(), self.caller("read"))
            .await
            .unwrap()
            .permissions
            .into_iter()
            .filter(|review| matches!(review.origin, ConversationPermissionOrigin::App { .. }))
            .collect()
    }

    pub(crate) async fn answer(&self, review: &ConversationPermission, option: &str) {
        self.service
            .answer(
                self.id.clone(),
                self.caller("answer"),
                review.execution_id.clone(),
                review.permission_id.clone(),
                option.into(),
            )
            .await
            .unwrap();
    }
}

pub(crate) fn caller(action: &str) -> ConversationCaller {
    ConversationCaller {
        organization_id: OrganizationId::new("org").unwrap(),
        principal_id: PrincipalId::new("person").unwrap(),
        surface_id: "panel".into(),
        action_id: action.into(),
    }
}
