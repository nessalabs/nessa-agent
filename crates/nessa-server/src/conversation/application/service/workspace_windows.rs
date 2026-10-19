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
use super::{load_conversation, ConversationCaller, ConversationError, ConversationService};
use crate::conversation::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    PublishedWindow, RequestShape, WorkspaceWindows,
};
use crate::conversation::domain::{ConversationRefusal, PaneContent};
use nessa_protocol::conversation::domain::ConversationId;
use std::{collections::HashMap, sync::Arc};

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
