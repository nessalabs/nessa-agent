//! Where each conversation runs: here, on this gateway's own machine. Every
//! gateway runs only its own agents (ADR 1417); another machine is another
//! gateway, which the person reaches directly.
//!
//! ```text
//! start_slot ──▶ Environments::for_lease(conversation) ──▶ ConversationPlaces::read
//!   ──▶ here ──▶ the in-process environment
//!   ──▶ placed elsewhere by an earlier build ──▶ EnvironmentNotConfigured
//! delete ──▶ Environments::erase ──▶ ConversationPlaces::erase
//! ```
//!
//! Arrows are calls, in order. Earlier builds could place a conversation on
//! an SSH host, a paired computer, any paired computer, or a cloud VM of its
//! own, and kept that in a place record. Such a conversation is never run
//! here instead: its working folder and provider session are on that other
//! machine. It stays readable, and refuses to open its agent. A record this
//! build cannot read refuses that conversation's opening alone (ADR 202,
//! record scope).
use super::{ConversationError, Environment, EnvironmentFuture};
use nessa_protocol::conversation::domain::ConversationId;
use std::sync::Arc;

/// Why a placement could not be read.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
#[cfg_attr(
    not(unix),
    allow(dead_code, reason = "raised by the Unix gateway's store")
)]
pub enum PlacementError {
    /// The placement exists and this build cannot read it.
    Unreadable,
    /// The store could not be used just now.
    Unavailable,
    /// An earlier build placed the conversation away from this machine.
    Elsewhere,
}

impl From<PlacementError> for ConversationError {
    fn from(error: PlacementError) -> Self {
        match error {
            PlacementError::Unreadable => Self::PlacementUnreadable,
            PlacementError::Unavailable => Self::Unavailable,
            PlacementError::Elsewhere => Self::EnvironmentNotConfigured,
        }
    }
}

/// The place records earlier builds kept. Implemented in infrastructure.
pub trait ConversationPlaces: Send + Sync {
    /// `Ok` when `conversation` runs here: it has no record, or its record
    /// says here.
    ///
    /// # Errors
    /// [`PlacementError::Elsewhere`] for one an earlier build placed on
    /// another machine.
    fn read<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
    /// Forget where `conversation` ran, once its history is erased.
    fn erase<'a>(
        &'a self,
        conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>>;
}

/// Where this gateway's conversations run: here.
pub struct Environments {
    here: Arc<dyn Environment>,
    places: Arc<dyn ConversationPlaces>,
}

#[cfg(any(test, feature = "test-support"))]
impl From<Arc<dyn Environment>> for Environments {
    /// Here, with no place records. For tests.
    fn from(here: Arc<dyn Environment>) -> Self {
        Self::new(here, Arc::new(NoPlaces))
    }
}

/// A place store for tests built with [`Environments::from`]: every
/// conversation runs here.
#[cfg(any(test, feature = "test-support"))]
struct NoPlaces;

#[cfg(any(test, feature = "test-support"))]
impl ConversationPlaces for NoPlaces {
    fn read<'a>(
        &'a self,
        _conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        Box::pin(async { Ok(()) })
    }
    fn erase<'a>(
        &'a self,
        _conversation: &'a ConversationId,
    ) -> EnvironmentFuture<'a, Result<(), PlacementError>> {
        Box::pin(async { Ok(()) })
    }
}

impl Environments {
    /// Here, and where earlier builds' place records are kept.
    #[cfg_attr(not(unix), allow(dead_code, reason = "the Unix gateway composes it"))]
    pub fn new(here: Arc<dyn Environment>, places: Arc<dyn ConversationPlaces>) -> Self {
        Self { here, places }
    }

    /// The environment `conversation`'s agent lease is asked of: here.
    ///
    /// # Errors
    /// [`ConversationError::EnvironmentNotConfigured`] for a conversation an
    /// earlier build placed on another machine; the placement store's own
    /// failures as [`PlacementError`] says.
    pub async fn for_lease(
        &self,
        conversation: &ConversationId,
    ) -> Result<Arc<dyn Environment>, ConversationError> {
        self.places.read(conversation).await?;
        Ok(self.here.clone())
    }

    /// Whether an earlier build placed `conversation` on another machine,
    /// where its agent's provider session and files are kept.
    ///
    /// # Errors
    /// The placement store's own failures, as [`PlacementError`] says.
    pub async fn elsewhere(
        &self,
        conversation: &ConversationId,
    ) -> Result<bool, ConversationError> {
        match self.places.read(conversation).await {
            Ok(()) => Ok(false),
            Err(PlacementError::Elsewhere) => Ok(true),
            Err(error) => Err(error.into()),
        }
    }

    /// Forget where `conversation` ran, once its history is erased.
    ///
    /// # Errors
    /// The placement store could not be used.
    pub async fn erase(&self, conversation: &ConversationId) -> Result<(), ConversationError> {
        self.places.erase(conversation).await.map_err(Into::into)
    }
}
