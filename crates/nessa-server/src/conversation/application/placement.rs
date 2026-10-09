//! Where each conversation runs: in this process, or on an SSH host named
//! when it was created (issue #699).
//!
//! ```text
//! create(environment?) ──▶ Environments::place ──▶ ConversationPlacements::place
//!                            (refused unless the host is configured)    (written before
//!                                                                         the record)
//! start_slot ──▶ Environments::of(conversation) ──▶ placement ──▶ here | that host
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
//! no longer names is refused the same way, never run somewhere else.
use super::{ConversationError, Environment, EnvironmentFuture};
use nessa_protocol::conversation::domain::ConversationId;
use std::{collections::BTreeMap, sync::Arc};

/// Why a placement could not be read or written.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
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

/// Where this gateway's conversations can run: here, and each SSH host its
/// configuration names.
pub(crate) struct Environments {
    here: Arc<dyn Environment>,
    hosts: BTreeMap<String, Arc<dyn Environment>>,
    placements: Option<Arc<dyn ConversationPlacements>>,
}

impl From<Arc<dyn Environment>> for Environments {
    /// Here alone, with nowhere placements are kept: every conversation runs
    /// here. For tests; composition always keeps placements.
    fn from(here: Arc<dyn Environment>) -> Self {
        Self {
            here,
            hosts: BTreeMap::new(),
            placements: None,
        }
    }
}

impl Environments {
    /// Here, the configured hosts by the destination they are named by, and
    /// where placements are kept.
    pub(crate) fn new(
        here: Arc<dyn Environment>,
        hosts: BTreeMap<String, Arc<dyn Environment>>,
        placements: Arc<dyn ConversationPlacements>,
    ) -> Self {
        Self {
            here,
            hosts,
            placements: Some(placements),
        }
    }

    /// Record where a conversation about to be created runs: on `requested`,
    /// which must be a configured host, or here.
    pub(crate) async fn place(
        &self,
        conversation: &ConversationId,
        requested: Option<&str>,
    ) -> Result<(), ConversationError> {
        if let Some(host) = requested {
            if !self.hosts.contains_key(host) {
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
            Some(host) => self
                .hosts
                .get(&host)
                .cloned()
                .ok_or(ConversationError::EnvironmentNotConfigured),
        }
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
