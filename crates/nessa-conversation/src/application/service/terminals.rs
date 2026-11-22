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
use crate::domain::{ChannelId, Cloud, Place};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, SshDestination};
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
    /// A configured SSH host, in `folder` on that host.
    Ssh {
        host: SshDestination,
        folder: PathBuf,
    },
    /// A cloud VM belonging to `conversation`.
    CloudVm {
        conversation: ConversationId,
        channel: Option<ChannelId>,
        cloud: Option<Cloud>,
        folder: Option<PathBuf>,
    },
    /// Somewhere else (a paired computer or pool). Not served yet.
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
            (EnvironmentRef::Ssh(host), Some(folder)) => TerminalPlace::Ssh {
                host: host.clone(),
                folder: PathBuf::from(folder),
            },
            (EnvironmentRef::Ssh(_), None) => return,
            (EnvironmentRef::Boat, folder) => TerminalPlace::CloudVm {
                conversation: id.clone(),
                channel: None,
                cloud: None,
                folder: folder.map(PathBuf::from),
            },
            (EnvironmentRef::Peer(_), _) => TerminalPlace::Elsewhere,
        };
        self.inner.workplaces.note(id, place);
    }

    /// Where a terminal `caller` asks for from `origin` runs.
    ///
    /// Cedar `terminal.use` decides: a person, on their own catalogue for a
    /// channel's terminal (which must also be a channel they may use), or on
    /// a conversation of theirs for Run. A terminal runs as the owner, in the
    /// owner's folder.
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
        match origin {
            TerminalOrigin::Channel(channel) => {
                self.admit_terminal(
                    caller,
                    AccessResource::Catalogue {
                        organization: &caller.organization_id,
                        owner: &caller.principal_id,
                    },
                )?;
                self.channel_terminal_place(caller, channel.as_ref(), None)
                    .await
            }
            TerminalOrigin::Conversation(id) => {
                let record = load_conversation(self.inner.metadata.as_ref(), id)
                    .await?
                    .ok_or(ConversationError::NotFound)?;
                // Cedar decides on the conversation's own facts.
                self.admit_terminal(caller, AccessResource::of(&record))?;
                if let Some(place) = self.inner.workplaces.get(id) {
                    return match place {
                        TerminalPlace::CloudVm {
                            conversation,
                            folder,
                            ..
                        } => {
                            let (channel, cloud) = self.conversation_cloud(caller, id).await?;
                            Ok(TerminalPlace::CloudVm {
                                conversation,
                                channel,
                                cloud,
                                folder,
                            })
                        }
                        other => Ok(other),
                    };
                }
                // Not open now: where its agent would work, by its channel.
                let channel = self.channel_id_of(id).await?;
                self.channel_terminal_place(caller, channel.as_ref(), Some(id))
                    .await
            }
        }
    }

    async fn conversation_cloud(
        &self,
        caller: &ConversationCaller,
        conversation: &ConversationId,
    ) -> Result<(Option<ChannelId>, Option<Cloud>), ConversationError> {
        let Some(channel_id) = self
            .channel_id_of(conversation)
            .await?
            .filter(|id| !id.is_default())
        else {
            return Ok((None, None));
        };
        let channel = self
            .channel_for_action(caller, &channel_id, AccessAction::ChannelUse)
            .await?;
        let cloud = if *conversation == channel.coordinator_id() {
            channel.execution().coordinator_cloud()
        } else {
            channel.execution().threads_cloud()
        };
        Ok((Some(channel_id), cloud))
    }

    async fn channel_terminal_place(
        &self,
        caller: &ConversationCaller,
        channel: Option<&ChannelId>,
        conversation: Option<&ConversationId>,
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
                let is_coordinator =
                    conversation.is_none_or(|conv| *conv == channel.coordinator_id());
                let (place, cloud) = if is_coordinator {
                    (
                        channel.execution().coordinator(),
                        channel.execution().coordinator_cloud(),
                    )
                } else {
                    (
                        channel.execution().threads(),
                        channel.execution().threads_cloud(),
                    )
                };
                Ok(match place {
                    Place::Here => TerminalPlace::Here {
                        folder: channel.folder().path.as_path().to_path_buf(),
                    },
                    Place::Host(host) => TerminalPlace::Ssh {
                        host: host.clone(),
                        folder: channel.folder().path.as_path().to_path_buf(),
                    },
                    Place::CloudVm => TerminalPlace::CloudVm {
                        conversation: conversation
                            .cloned()
                            .unwrap_or_else(|| channel.coordinator_id()),
                        channel: Some(channel.id().clone()),
                        cloud,
                        folder: None,
                    },
                    Place::Peer(_) | Place::Pool => TerminalPlace::Elsewhere,
                })
            }
        }
    }

    fn admit_terminal(
        &self,
        caller: &ConversationCaller,
        resource: AccessResource<'_>,
    ) -> Result<(), ConversationError> {
        let ask = AccessAsk {
            subject: AccessSubject::Person {
                organization: &caller.organization_id,
                principal: &caller.principal_id,
            },
            action: AccessAction::TerminalUse,
            resource,
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
