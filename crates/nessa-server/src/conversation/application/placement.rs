//! Where each conversation runs: in this process, on an SSH host, or on a
//! paired environment, named when it was created (issues #699, #706).
//!
//! ```text
//! create(environment?) ──▶ Environments::place ──▶ ConversationPlacements::place
//!                            (refused unless the host is configured, (written before
//!                             or the paired environment connected)    the record)
//! start_slot ──▶ Environments::of(conversation) ──▶ placement ──▶ here | that host
//!                                                                 | that peer
//! delete ──▶ Environments::erase ──▶ ConversationPlacements::erase
//! ```
//!
//! Arrows are calls, in order. A placement is written once, before the
//! conversation's record exists, so a conversation never exists without
//! knowing where it runs; it is never moved. A conversation with no
//! placement runs here: that is every conversation unless a host was named,
//! and a gateway whose configuration names none never reaches a host's code.
//! A placement this build cannot read refuses that conversation's opening
//! alone (ADR 202, record scope). A placement naming a host the configuration
//! no longer names is refused the same way, never run somewhere else. A
//! paired environment is placed on only while it is connected, so a name
//! nobody serves is never kept; once placed, the conversation waits for it
//! to dial in again, and its key's credential is asked before every lease.
use super::{ConversationError, Environment, EnvironmentFuture};
use crate::conversation::domain::CommandPolicy;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{EnvironmentRef, PeerEnvironment};
use std::{collections::BTreeMap, sync::Arc};

/// Why a placement could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "raised by the Unix gateway's store")
)]
pub(crate) enum PlacementError {
    /// The placement exists and this build cannot read it.
    Unreadable,
    /// The store could not be used just now.
    Unavailable,
}

impl From<PlacementError> for ConversationError {
    fn from(error: PlacementError) -> Self {
        match error {
            PlacementError::Unreadable => Self::PlacementUnreadable,
            PlacementError::Unavailable => Self::Unavailable,
        }
    }
}

/// The durable record of which conversations run on a host, and which.
/// Implemented in infrastructure.
pub(crate) trait ConversationPlacements: Send + Sync {
    /// Record that `conversation`, not yet created, runs on `host`, or here
    /// when `None` (removing a placement a failed creation of the same
    /// identity left). Durable before it answers.
    fn place<'a>(
        &'a self,
        conversation: &'a ConversationId,
        host: Option<&'a str>,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
    /// The host `conversation` runs on, `None` for here.
    fn placement<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<Option<String>, PlacementError>>;
    /// Forget `conversation`'s placement, once its history is erased.
    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
}

/// The paired environments that dial this gateway. Implemented in
/// infrastructure.
pub(crate) trait PairedEnvironments: Send + Sync {
    /// The environment `peer` names, connected or not.
    fn environment(&self, peer: &PeerEnvironment) -> Arc<dyn Environment>;
    /// The paired environments connected now.
    fn connected(&self) -> Vec<PeerEnvironment>;
}

/// Where this gateway's conversations can run: here, each SSH host its
/// configuration names, and the paired environments that dial it.
pub(crate) struct Environments {
    here: Arc<dyn Environment>,
    hosts: BTreeMap<String, Arc<dyn Environment>>,
    /// `None` when this gateway pairs no environments.
    peers: Option<Arc<dyn PairedEnvironments>>,
    placements: Option<Arc<dyn ConversationPlacements>>,
    /// Which commands an agent may run, and on which hosts; `None` when this
    /// gateway grants none, and the environment tools are absent.
    commands: Option<CommandPolicy>,
}

impl From<Arc<dyn Environment>> for Environments {
    /// Here alone, with nowhere placements are kept: every conversation runs
    /// here. For tests; composition always keeps placements.
    fn from(here: Arc<dyn Environment>) -> Self {
        Self {
            here,
            hosts: BTreeMap::new(),
            peers: None,
            placements: None,
            commands: None,
        }
    }
}

impl Environments {
    /// Here, the configured hosts by the destination they are named by, and
    /// where placements are kept.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn new(
        here: Arc<dyn Environment>,
        hosts: BTreeMap<String, Arc<dyn Environment>>,
        placements: Arc<dyn ConversationPlacements>,
    ) -> Self {
        Self {
            here,
            hosts,
            peers: None,
            placements: Some(placements),
            commands: None,
        }
    }

    /// These environments, with the paired environments `peers`.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_peers(mut self, peers: Arc<dyn PairedEnvironments>) -> Self {
        self.peers = Some(peers);
        self
    }

    /// These environments, with commands granted by `policy`.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub(crate) fn with_commands(mut self, policy: CommandPolicy) -> Self {
        self.commands = Some(policy);
        self
    }

    /// Which commands an agent may run, and where; `None` when none.
    pub(crate) fn command_policy(&self) -> Option<&CommandPolicy> {
        self.commands.as_ref()
    }

    /// The configured hosts, by the destination each is named by.
    pub(crate) fn hosts(&self) -> &BTreeMap<String, Arc<dyn Environment>> {
        &self.hosts
    }

    /// The paired environments connected now, by name.
    pub(crate) fn connected_peers(&self) -> Vec<(String, Arc<dyn Environment>)> {
        self.peers.as_ref().map_or_else(Vec::new, |peers| {
            peers
                .connected()
                .iter()
                .map(|peer| (peer.name(), peers.environment(peer)))
                .collect()
        })
    }

    /// The environment away from here that `name` names: a configured host,
    /// or a paired environment, connected or not.
    pub(crate) fn named(&self, name: &str) -> Option<Arc<dyn Environment>> {
        match PeerEnvironment::parse(name) {
            Ok(peer) => self.peers.as_ref().map(|peers| peers.environment(&peer)),
            Err(_) => self.hosts.get(name).cloned(),
        }
    }

    /// Record where a conversation about to be created runs: on `requested`,
    /// which must be a configured host or a connected paired environment,
    /// or here.
    pub(crate) async fn place(
        &self,
        conversation: &ConversationId,
        requested: Option<&str>,
    ) -> Result<(), ConversationError> {
        if let Some(name) = requested {
            let known = match PeerEnvironment::parse(name) {
                Ok(peer) => self
                    .peers
                    .as_ref()
                    .is_some_and(|peers| peers.connected().contains(&peer)),
                Err(_) => self.hosts.contains_key(name),
            };
            if !known {
                return Err(ConversationError::EnvironmentNotConfigured);
            }
        }
        match &self.placements {
            Some(placements) => Ok(placements.place(conversation, requested).await?),
            None if requested.is_none() => Ok(()),
            None => Err(ConversationError::EnvironmentNotConfigured),
        }
    }

    /// The host `conversation` was placed on, `None` for here.
    pub(crate) async fn placement(
        &self,
        conversation: &ConversationId,
    ) -> Result<Option<String>, ConversationError> {
        match &self.placements {
            Some(placements) => Ok(placements.placement(conversation).await?),
            None => Ok(None),
        }
    }

    /// The environment `conversation` runs in.
    pub(crate) async fn of(
        &self,
        conversation: &ConversationId,
    ) -> Result<Arc<dyn Environment>, ConversationError> {
        match self.placement(conversation).await? {
            None => Ok(self.here.clone()),
            Some(name) => self
                .named(&name)
                .ok_or(ConversationError::EnvironmentNotConfigured),
        }
    }

    /// Whether `conversation` runs where this machine's files are its own,
    /// so a path on this machine names the same file there: only here. A
    /// host has files of its own, and would read whatever it has at that
    /// path, or nothing.
    pub(crate) async fn reads_this_machine(
        &self,
        conversation: &ConversationId,
    ) -> Result<bool, ConversationError> {
        Ok(matches!(
            self.of(conversation).await?.declaration().environment,
            EnvironmentRef::Here
        ))
    }

    /// Forget where `conversation` ran.
    pub(crate) async fn erase(
        &self,
        conversation: &ConversationId,
    ) -> Result<(), ConversationError> {
        match &self.placements {
            Some(placements) => Ok(placements.erase(conversation).await?),
            None => Ok(()),
        }
    }
}
