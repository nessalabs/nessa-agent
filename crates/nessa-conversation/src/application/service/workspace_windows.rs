//! What an agent reads of its person's open windows (`read_window`): the
//! windows as they published themselves, and, for each conversation a pane
//! shows, what the gateway says it is.
//!
//! ```text
//! read_window ──▶ admit_agent_tool (source turn acts as its person)
//!   ──▶ workspace.read on that person's workspace (Cedar) ── Deny ──▶ refused
//!   ──▶ WorkspaceWindows::of(person) ──▶ each conversation pane:
//!         metadata ──▶ conversation.read for the person (Cedar)
//!           ── Allow ──▶ title (summary), agent, model
//!           └─ Deny / missing ──▶ unavailable
//! ```
//!
//! And what an agent, or the person's own CLI, changes in one of those
//! windows (`control_window`): one path for both, the tools and the
//! methods only decoding their arguments into a [`WindowAction`].
//!
//! ```text
//! control_window(caller, via, target, action)
//!   ──▶ workspace.control on the person's workspace (Cedar) ── Deny ──▶ audited, refused
//!   ──▶ a conversation the action names ──▶ conversation.read for the person ── Deny ──▶ audited, refused
//!   ──▶ the window: named, else the newest showing the caller's conversation, else the newest
//!   ──▶ audit Requested ──▶ WorkspaceWindows::command ──▶ `workspace.command` to its connection
//!   ──▶ the window's answer, within WINDOW_ANSWER_TIMEOUT
//!         Applied / Unchanged { revision } ──▶ wait until that revision is held (or the
//!           window is withdrawn, or WINDOW_HELD_WAIT) ──▶ audit outcome: the change is made
//!         Refused { code } ──▶ audit refused ──▶ refused
//!         no answer in time, or the connection gone ──▶ audit unconfirmed (it may
//!           have happened) ──▶ window_not_answering / window_unavailable
//! ```
use super::{load_conversation, ConversationCaller, ConversationError, ConversationService};
use crate::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    CommandRefusal, PublishedWindow, RequestShape, WindowAnswer, WindowControlAuditRecord,
    WindowControlOutcome, WindowControlVia, WorkspaceWindows,
};
use crate::domain::{ConversationRefusal, PaneContent, WindowAction};
use nessa_protocol::conversation::domain::ConversationId;
use std::{collections::HashMap, sync::Arc, time::Duration};

/// How long a window has to answer a change. A window answers at once; past
/// this it is not reading.
pub const WINDOW_ANSWER_TIMEOUT: Duration = Duration::from_secs(5);

/// How long an answered change waits for the gateway to hold what the
/// window shows after it, before the caller hears anyway: the change is
/// made by then, and recorded as made.
pub const WINDOW_HELD_WAIT: Duration = Duration::from_secs(1);

/// Which window a change is for.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WindowTarget<'a> {
    /// The window with this id, as `read_window` names it.
    Named(&'a str),
    /// The most recently published window showing this conversation, else
    /// the most recently published one: where an agent's person is looking.
    Showing(&'a ConversationId),
    /// The most recently published window.
    Newest,
}

/// A change a window made, or found made already.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowControlled {
    pub window_id: String,
    /// The publication that shows it.
    pub revision: u64,
    /// False when it was so already.
    pub changed: bool,
}

/// Why a window change was not made.
#[derive(Debug)]
pub enum WindowControlError {
    /// Refused by policy, the service stopping, or the audit failing.
    Conversation(ConversationError),
    /// The conversation the action names is missing or not the person's to read.
    ConversationNotFound,
    /// No open window, or not the one named.
    NoWindow,
    /// The window owes too many answers.
    Busy,
    /// The window did not answer, or did not publish, in time.
    NotAnswering,
    /// The window could not do it, for its code.
    Refused(String),
}

impl WindowControlError {
    /// The code a tool or method answers.
    pub fn code(&self) -> String {
        match self {
            Self::Conversation(error) => crate::application::error_code(error).as_str().to_owned(),
            Self::ConversationNotFound => "conversation_not_found".into(),
            Self::NoWindow => "window_unavailable".into(),
            Self::Busy => "window_busy".into(),
            Self::NotAnswering => "window_not_answering".into(),
            Self::Refused(code) => code.clone(),
        }
    }
}

/// What one conversation a pane shows is, as the gateway says it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PaneConversation {
    Shown {
        title: Option<String>,
        /// `None` when the record names an agent this build cannot open.
        agent: Option<&'static str>,
        model: String,
    },
    /// Not found, deleted, or not the person's to read now.
    Unavailable,
}

/// The windows a person has open, and the conversations they show.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct WindowReading {
    /// Each window once, the most recently published first.
    pub windows: Vec<PublishedWindow>,
    pub conversations: HashMap<ConversationId, PaneConversation>,
}

impl ConversationService {
    /// Where every connection's window is held, for the socket to publish into.
    pub fn workspace_windows(&self) -> Arc<WorkspaceWindows> {
        self.inner.workspace_windows.clone()
    }

    /// The windows `caller` has open, each conversation pane described.
    ///
    /// # Errors
    /// `NotFound` when the policy refuses `caller` their workspace, and
    /// `Unavailable` when it, or the metadata a pane needs, cannot be read.
    pub async fn read_windows(
        &self,
        caller: &ConversationCaller,
    ) -> Result<WindowReading, ConversationError> {
        let _admission = self.admit().await?;
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: &caller.organization_id,
                principal: &caller.principal_id,
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
            },
            action: AccessAction::WorkspaceRead,
            resource: AccessResource::Workspace {
                organization: &caller.organization_id,
                owner: &caller.principal_id,
            },
            held: None,
            request: RequestShape::default(),
            target: None,
        };
        match self.inner.access_policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => {}
            AccessDecision::Deny(AccessDenial::Unavailable) => {
                return Err(ConversationError::Unavailable)
            }
            AccessDecision::Deny(_) => return Err(ConversationError::NotFound),
        }
        let windows = self
            .inner
            .workspace_windows
            .of(&caller.organization_id, &caller.principal_id);
        let mut conversations = HashMap::new();
        for published in &windows {
            for place in published.window.places() {
                let PaneContent::Conversation(id) = place.pane.content() else {
                    continue;
                };
                if conversations.contains_key(id) {
                    continue;
                }
                let described = self.pane_conversation(id, caller).await?;
                conversations.insert(id.clone(), described);
            }
        }
        Ok(WindowReading {
            windows,
            conversations,
        })
    }

    /// Records what window changes ask and what comes of them. Unbound,
    /// every change is refused before it is sent.
    pub fn bind_window_control_audit(
        &self,
        audit: Arc<dyn crate::application::WindowControlAudit>,
    ) {
        let _ = self.inner.window_control_audit.set(audit);
    }

    /// Asks one of `caller`'s windows to make `action`, and waits for it to
    /// say it has, and to publish what it shows after it.
    ///
    /// # Errors
    /// See [`WindowControlError`]. Every attempt, refused or not, is audited
    /// first; an audit that cannot be written refuses the change.
    pub async fn control_window(
        &self,
        caller: &ConversationCaller,
        via: WindowControlVia,
        target: WindowTarget<'_>,
        action: WindowAction,
    ) -> Result<WindowControlled, WindowControlError> {
        let audit = self
            .inner
            .window_control_audit
            .get()
            .cloned()
            .ok_or(WindowControlError::Conversation(ConversationError::Audit))?;
        let mut record = WindowControlAuditRecord {
            operation_id: uuid::Uuid::new_v4().to_string(),
            caller: caller.clone(),
            via,
            window_id: None,
            action: action.clone(),
            policies: Vec::new(),
            outcome: WindowControlOutcome::Requested,
        };
        let refused = |mut record: WindowControlAuditRecord, error: WindowControlError| {
            let audit = audit.clone();
            async move {
                record.outcome = WindowControlOutcome::Refused { code: error.code() };
                audit
                    .record(record)
                    .await
                    .map_err(WindowControlError::Conversation)?;
                Err(error)
            }
        };
        let window = {
            let _admission = self
                .admit()
                .await
                .map_err(|halt| WindowControlError::Conversation(halt.into()))?;
            let ask = AccessAsk {
                subject: AccessSubject::Person {
                    organization: &caller.organization_id,
                    principal: &caller.principal_id,
                    app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
                },
                action: AccessAction::WorkspaceControl,
                resource: AccessResource::Workspace {
                    organization: &caller.organization_id,
                    owner: &caller.principal_id,
                },
                held: None,
                request: RequestShape::default(),
                target: None,
            };
            match self.inner.access_policy.evaluate(&ask) {
                AccessDecision::Allow { policies } => record.policies = policies,
                AccessDecision::Deny(AccessDenial::Unavailable) => {
                    let error = WindowControlError::Conversation(ConversationError::Unavailable);
                    return refused(record, error).await;
                }
                AccessDecision::Deny(_) => {
                    let error = WindowControlError::Conversation(ConversationError::NotFound);
                    return refused(record, error).await;
                }
            }
            if let Some(conversation) = action.conversation() {
                match self.pane_conversation(conversation, caller).await {
                    Ok(PaneConversation::Shown { .. }) => {}
                    Ok(PaneConversation::Unavailable) => {
                        return refused(record, WindowControlError::ConversationNotFound).await
                    }
                    Err(error) => {
                        return refused(record, WindowControlError::Conversation(error)).await
                    }
                }
            }
            let windows = self
                .inner
                .workspace_windows
                .of(&caller.organization_id, &caller.principal_id);
            let chosen = match target {
                WindowTarget::Named(id) => windows.into_iter().find(|window| window.id == id),
                WindowTarget::Showing(conversation) => {
                    let showing = windows.iter().position(|window| {
                        window.window.places().iter().any(|place| {
                            matches!(place.pane.content(), PaneContent::Conversation(id) if id == conversation)
                        })
                    });
                    let index = showing.unwrap_or(0);
                    windows.into_iter().nth(index)
                }
                WindowTarget::Newest => windows.into_iter().next(),
            };
            let Some(window) = chosen else {
                return refused(record, WindowControlError::NoWindow).await;
            };
            window.id
        };
        record.window_id = Some(window.clone());
        audit
            .record(record.clone())
            .await
            .map_err(WindowControlError::Conversation)?;
        let answer = match self.inner.workspace_windows.command(
            &caller.organization_id,
            &caller.principal_id,
            &window,
            action,
        ) {
            Ok(answer) => answer,
            Err(CommandRefusal::Gone) => {
                return refused(record, WindowControlError::NoWindow).await
            }
            Err(CommandRefusal::Busy) => return refused(record, WindowControlError::Busy).await,
        };
        // Sent: from here a missing answer is not a refusal. The window may
        // still carry the change out, so it is recorded as unconfirmed.
        let unconfirmed = |mut record: WindowControlAuditRecord, error: WindowControlError| {
            let audit = audit.clone();
            async move {
                record.outcome = WindowControlOutcome::Unconfirmed { code: error.code() };
                audit
                    .record(record)
                    .await
                    .map_err(WindowControlError::Conversation)?;
                Err(error)
            }
        };
        let answered = match tokio::time::timeout(WINDOW_ANSWER_TIMEOUT, answer).await {
            Ok(Ok(answered)) => answered,
            // The window's connection closed before it answered.
            Ok(Err(_)) => return unconfirmed(record, WindowControlError::NoWindow).await,
            Err(_) => return unconfirmed(record, WindowControlError::NotAnswering).await,
        };
        let (revision, changed) = match answered {
            WindowAnswer::Applied { revision } => (revision, true),
            WindowAnswer::Unchanged { revision } => (revision, false),
            WindowAnswer::Refused { code } => {
                return refused(record, WindowControlError::Refused(code)).await
            }
        };
        // Answered, the change is made, and is recorded as made whatever
        // follows. What the window shows after it is what a read must find,
        // so the caller hears once it is held: the window answers only after
        // its publication was taken, so this is at once unless that
        // connection has since closed (withdrawn) or a later one is slow.
        let held = self.inner.workspace_windows.published(
            &caller.organization_id,
            &caller.principal_id,
            &window,
            revision,
        );
        let _ = tokio::time::timeout(WINDOW_HELD_WAIT, held).await;
        record.outcome = if changed {
            WindowControlOutcome::Applied { revision }
        } else {
            WindowControlOutcome::Unchanged { revision }
        };
        audit
            .record(record)
            .await
            .map_err(WindowControlError::Conversation)?;
        Ok(WindowControlled {
            window_id: window,
            revision,
            changed,
        })
    }

    /// What `id` is, when `caller` may read it now.
    async fn pane_conversation(
        &self,
        id: &ConversationId,
        caller: &ConversationCaller,
    ) -> Result<PaneConversation, ConversationError> {
        let Some(record) = load_conversation(self.inner.metadata.as_ref(), id).await? else {
            return Ok(PaneConversation::Unavailable);
        };
        match self.access().admit_to(
            AccessAction::Read,
            &record,
            &caller.organization_id,
            &caller.principal_id,
        ) {
            Ok(()) => {}
            Err(ConversationRefusal::Unavailable) => return Err(ConversationError::Unavailable),
            Err(_) => return Ok(PaneConversation::Unavailable),
        }
        Ok(PaneConversation::Shown {
            title: self.title(id).await,
            agent: record.agent().map(|agent| agent.name()),
            model: record.model().as_str().to_owned(),
        })
    }
}
