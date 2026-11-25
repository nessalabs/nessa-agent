//! Where a terminal the person opens runs (issue #1398, ADR 1417,
//! `docs/design/terminals.md`, "Where it runs"). A request names a channel
//! (⌘J) or a conversation (Run), never a folder or a host: each gateway
//! serves native local terminals (`Here`) for channels and conversations on
//! its own machine; remote machines run their own `nessa-server` gateway.
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
    /// Somewhere other than this gateway; open the terminal on that machine's
    /// own gateway directly (ADR 1417).
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
        if let Some(place) = workplace_for(environment, workspace) {
            self.inner.workplaces.note(id, place);
        }
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
                        app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
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
                    return Ok(place);
                }
                // Not open now: where its agent would work, by its channel.
                let channel = self.channel_id_of(id).await?;
                self.channel_terminal_place(caller, channel.as_ref(), Some(id))
                    .await
            }
        }
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
                let place = if is_coordinator {
                    channel.execution().coordinator()
                } else {
                    channel.execution().threads()
                };
                Ok(place_for(place, channel.folder().path.as_path()))
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
                app_namespace: &nessa_auth::domain::AppNamespace::DEFAULT,
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

fn workplace_for(
    environment: &EnvironmentRef,
    workspace: Option<&String>,
) -> Option<TerminalPlace> {
    match (environment, workspace) {
        (EnvironmentRef::Here, Some(folder)) => Some(TerminalPlace::Here {
            folder: PathBuf::from(folder),
        }),
        (EnvironmentRef::Here, None) => None,
        (EnvironmentRef::Ssh(_) | EnvironmentRef::Boat | EnvironmentRef::Peer(_), _) => {
            Some(TerminalPlace::Elsewhere)
        }
    }
}

fn place_for(place: &Place, folder: &std::path::Path) -> TerminalPlace {
    match place {
        Place::Here => TerminalPlace::Here {
            folder: folder.to_path_buf(),
        },
        Place::Host(_) | Place::CloudVm | Place::Peer(_) | Place::Pool => TerminalPlace::Elsewhere,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_sdk::domain::agent_execution::leases::{PeerEnvironment, SshDestination};
    use std::path::Path;

    #[test]
    fn workplace_maps_here_with_folder_and_refuses_non_local_environments() {
        let folder = "/tmp/project".to_owned();
        let host = SshDestination::new("devbox").unwrap();
        let peer = PeerEnvironment::new([1; 32]);
        assert_eq!(
            workplace_for(&EnvironmentRef::Here, Some(&folder)),
            Some(TerminalPlace::Here {
                folder: PathBuf::from("/tmp/project"),
            })
        );
        assert_eq!(workplace_for(&EnvironmentRef::Here, None), None);
        assert_eq!(
            workplace_for(&EnvironmentRef::Ssh(host), Some(&folder)),
            Some(TerminalPlace::Elsewhere)
        );
        assert_eq!(
            workplace_for(&EnvironmentRef::Boat, Some(&folder)),
            Some(TerminalPlace::Elsewhere)
        );
        assert_eq!(
            workplace_for(&EnvironmentRef::Peer(peer), None),
            Some(TerminalPlace::Elsewhere)
        );
    }

    #[test]
    fn channel_place_maps_here_and_refuses_non_local_places() {
        let folder = Path::new("/tmp/channel");
        let host = SshDestination::new("devbox").unwrap();
        let peer = PeerEnvironment::new([1; 32]);
        assert_eq!(
            place_for(&Place::Here, folder),
            TerminalPlace::Here {
                folder: PathBuf::from("/tmp/channel"),
            }
        );
        assert_eq!(
            place_for(&Place::Host(host), folder),
            TerminalPlace::Elsewhere
        );
        assert_eq!(place_for(&Place::CloudVm, folder), TerminalPlace::Elsewhere);
        assert_eq!(
            place_for(&Place::Peer(peer), folder),
            TerminalPlace::Elsewhere
        );
        assert_eq!(place_for(&Place::Pool, folder), TerminalPlace::Elsewhere);
    }
}
