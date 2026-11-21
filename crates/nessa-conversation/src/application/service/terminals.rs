//! Where a terminal the person opens runs (issue #1398,
//! `docs/design/terminals.md`, "Where it runs"). A request names a channel
//! (⌘J) or a conversation (Run), never a folder or a host: the place is
//! resolved here, by the same rules an agent of that channel or conversation
//! works by.
use super::{load_conversation, ConversationCaller, ConversationError, ConversationService};
use crate::application::{
    AccessAction, AccessAsk, AccessDecision, AccessDenial, AccessResource, AccessSubject,
    RequestShape,
};
use crate::domain::{ChannelId, Place};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::EnvironmentRef;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Mutex;

/// What a terminal is asked to start from.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalOrigin {
    /// ⌘J in a channel: the channel's folder, where its main agent runs.
    /// `None` is the default channel.
    Channel(Option<ChannelId>),
    /// Run in a conversation: where that conversation's agent works.
    Conversation(ConversationId),
}

/// Where a terminal runs.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalPlace {
    /// This gateway's machine, in `folder`.
    Here { folder: PathBuf },
    /// Somewhere else (an SSH host, a paired computer, a cloud VM). Not
    /// served yet: slice T5.
    Elsewhere,
}

/// Where each open conversation's agent works now, as its opening found it.
#[derive(Default)]
pub(super) struct AgentWorkplaces(Mutex<HashMap<ConversationId, TerminalPlace>>);

impl AgentWorkplaces {
    fn note(&self, id: &ConversationId, place: TerminalPlace) {
        if let Ok(mut places) = self.0.lock() {
            places.insert(id.clone(), place);
        }
    }

    fn get(&self, id: &ConversationId) -> Option<TerminalPlace> {
        self.0
            .lock()
            .ok()
            .and_then(|places| places.get(id).cloned())
    }
}

impl ConversationService {
    /// Remember where `id`'s agent works, as its opening found it.
    pub(super) fn note_workplace(
        &self,
        id: &ConversationId,
        environment: &EnvironmentRef,
        workspace: Option<&String>,
    ) {
        let place = match (environment, workspace) {
            (EnvironmentRef::Here, Some(folder)) => TerminalPlace::Here {
                folder: PathBuf::from(folder),
            },
            (EnvironmentRef::Here, None) => return,
            _ => TerminalPlace::Elsewhere,
        };
        self.inner.workplaces.note(id, place);
    }

    /// Where a terminal `caller` asks for from `origin` runs.
    ///
    /// Only a person may open one, on their own catalogue (Cedar
    /// `terminal.use`), and only in a channel they may use or a conversation
    /// they own: a terminal runs as the owner, in the owner's folder.
    ///
    /// # Errors
    /// `NotFound` when the policy refuses or the conversation is not the
    /// caller's; `ChannelNotFound` when the channel is not; `Unavailable`
    /// when there is no folder to start in.
    pub async fn terminal_place(
        &self,
        caller: &ConversationCaller,
        origin: &TerminalOrigin,
    ) -> Result<TerminalPlace, ConversationError> {
        self.admit_terminal(caller)?;
        match origin {
            TerminalOrigin::Channel(channel) => {
                self.channel_terminal_place(caller, channel.as_ref()).await
            }
            TerminalOrigin::Conversation(id) => {
                let record = load_conversation(self.inner.metadata.as_ref(), id)
                    .await?
                    .ok_or(ConversationError::NotFound)?;
                if record.organization() != &caller.organization_id
                    || record.owner() != &caller.principal_id
                {
                    return Err(ConversationError::NotFound);
                }
                if let Some(place) = self.inner.workplaces.get(id) {
                    return Ok(place);
                }
                // Not open now: where its agent would work, by its channel.
                let channel = self.channel_id_of(id).await?;
                self.channel_terminal_place(caller, channel.as_ref()).await
            }
        }
    }

    async fn channel_terminal_place(
        &self,
        caller: &ConversationCaller,
        channel: Option<&ChannelId>,
    ) -> Result<TerminalPlace, ConversationError> {
        match channel.filter(|id| !id.is_default()) {
            None => self
                .inner
                .workspace
                .as_ref()
                .map(|folder| TerminalPlace::Here {
                    folder: PathBuf::from(folder),
                })
                .ok_or(ConversationError::Unavailable),
            Some(id) => {
                let channel = self
                    .channel_for_action(caller, id, AccessAction::ChannelUse)
                    .await?;
                Ok(match channel.execution().coordinator() {
                    Place::Here => TerminalPlace::Here {
                        folder: channel.folder().path.as_path().to_path_buf(),
                    },
                    _ => TerminalPlace::Elsewhere,
                })
            }
        }
    }

    fn admit_terminal(&self, caller: &ConversationCaller) -> Result<(), ConversationError> {
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: &caller.organization_id,
                principal: &caller.principal_id,
            },
            action: AccessAction::TerminalUse,
            resource: AccessResource::Catalogue {
                organization: &caller.organization_id,
                owner: &caller.principal_id,
            },
            held: None,
            request: RequestShape::default(),
            target: None,
        };
        match self.inner.access_policy.evaluate(&ask) {
            AccessDecision::Allow { .. } => Ok(()),
            AccessDecision::Deny(AccessDenial::Unavailable) => Err(ConversationError::Unavailable),
            AccessDecision::Deny(_) => Err(ConversationError::NotFound),
        }
    }
}
