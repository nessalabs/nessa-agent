#![deny(missing_docs)]

use crate::application::agent_execution::agents::AgentError;
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use std::{future::Future, pin::Pin};

/// What became of an agent's own record of a session when it was asked to
/// delete it, through [`ProviderSessionDeleter::delete_session`].
///
/// An agent keeps its own transcript of each session in its own store, and
/// what it does there when asked is the agent's. Which of these an answer
/// means is decided by that agent's own binding, from what its adapter is
/// known to do on a successful delete; nothing here is a claim the exchange
/// did not confirm.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ProviderSessionDeletion {
    /// The agent accepted the delete, and its binding knows its adapter
    /// erases the session's record on it.
    Deleted,
    /// The agent accepted the delete, and its binding knows its adapter
    /// archives the session rather than erasing it: the transcript stays in
    /// the agent's store.
    Archived,
    /// The agent accepted the delete, and what its adapter does on it is not
    /// known to this binding. Nothing is claimed about the record.
    Acknowledged,
    /// The agent refused to delete this session, and its list of sessions,
    /// for the workspace, read in full after the refusal, does not name it:
    /// what an agent says of a session it no longer has, as after a deletion
    /// interrupted once the agent had deleted. The same for every agent,
    /// decided by the shared exchange. Only its own error answer counts as a
    /// refusal.
    NotListed,
    /// The agent does not offer deleting a session, so nothing was asked and
    /// its record is still there. For an ACP agent, `initialize` did not
    /// advertise `agentCapabilities.sessionCapabilities.delete`.
    NotSupported,
}

/// A pending [`ProviderSessionDeleter::delete_session`].
///
/// Dropping it before it resolves abandons the exchange; an ACP binding still
/// stops the connection's process and releases what its launch made, or hands
/// them to the supervisor that keeps retrying.
pub type ProviderSessionDeletionFuture<'a> =
    Pin<Box<dyn Future<Output = Result<ProviderSessionDeletion, AgentError>> + Send + 'a>>;

/// One agent's way of deleting its own record of a session nothing runs any
/// more.
///
/// Each agent binding implements this in its own module, because what a
/// successful delete means is that agent's: the ACP bindings share the
/// exchange and each says what the answer means for its agent. A host
/// dispatches to it by agent; nothing here chooses between agents.
pub trait ProviderSessionDeleter: Send + Sync {
    /// Ask the agent to delete its own record of `session`, a provider context
    /// this process no longer runs, and report what that confirmed.
    ///
    /// For erasing a conversation for good: the caller has already stopped
    /// every [`Agent`](crate::application::agent_execution::agents::Agent)
    /// attached to `session`, and reads `session` from saved history
    /// ([`ProviderContext::recorded`](crate::application::agent_execution::sessions::storage::ProviderContext::recorded)).
    /// Deleting a session an agent is still running is the caller's error to
    /// avoid; nothing here can tell.
    ///
    /// The ACP bindings open a connection of their own for this, launched
    /// exactly as their provider launches one: `initialize`, then — only when
    /// the agent advertised `agentCapabilities.sessionCapabilities.delete` —
    /// `session/delete { sessionId }`, then the connection's process is
    /// stopped. The session is never loaded or resumed, and nothing else is
    /// sent under it. An accepted delete never depends on anything else. Only
    /// when the agent refuses it, and advertises `sessionCapabilities.list`,
    /// is it then asked for its sessions in the configured workspace, to read
    /// the refusal: see [`ProviderSessionDeletion::NotListed`]. A process
    /// whose stop is not confirmed is handed to the same
    /// supervisor that keeps retrying an abandoned provider process, and does
    /// not change the answer.
    ///
    /// An ACP binding answers within
    /// [`AcpConfig::session_deletion_limit`](crate::infrastructure::acp::sessions::AcpConfig::session_deletion_limit),
    /// which states the sum. Dropping the future ends the exchange, and the
    /// binding still stops its process and releases what its launch made.
    ///
    /// # Errors
    /// The typed reason the agent could not be asked, or refused, from the
    /// exchange up to and including the delete's answer: a configuration the
    /// binding cannot launch with ([`AgentError::Configuration`]), a launch
    /// failure ([`AgentError::Transport`]), a budget running out
    /// ([`AgentError::Deadline`]), an `initialize` or a delete answer the
    /// binding does not accept ([`AgentError::Protocol`]), the agent's error
    /// response ([`AgentError::Provider`]) to `initialize` or to
    /// `session/delete`, or the exchange ending with no answer to give
    /// ([`AgentError::Closed`]) — its connection closed, or its task ended
    /// without one.
    ///
    /// [`AgentError::Provider`] is only ever the agent's error answer to one
    /// of those two, never to the list. The list read after a refusal of the
    /// delete never supplies an error of its own: a list that names the
    /// session, or that could not be read in full — refused, past its budget,
    /// too large, a page without `sessions`, an entry without a string
    /// `sessionId`, a `nextCursor` that is neither a string nor null or
    /// repeats one already followed, too many pages — leaves the refusal standing, unchanged
    /// (`a_refused_delete_the_list_cannot_explain_stays_the_refusal`). None
    /// of these errors says whether the agent's record still exists; asking
    /// again is how to find out.
    fn delete_session(&self, session: ExecutionSessionId) -> ProviderSessionDeletionFuture<'_>;

    /// Resolves once every deletion this binding started, and whose caller
    /// stopped waiting, has stopped its process and released what its launch
    /// made — or once stopping them has had its whole budget. A host awaits
    /// this before its runtime ends: a runtime that ends drops those tasks
    /// mid-cleanup. The ACP bindings wait at most `shutdown_grace` + 4 ×
    /// `kill_timeout`. The default has nothing to wait for.
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }

    /// Whether a deletion this binding started, and whose caller stopped
    /// waiting, still has a process to stop. Unlike [`Self::settled`] this is
    /// never bounded: it stays true after `settled` gave up waiting, until the
    /// process is actually stopped. The ACP bindings answer from the same count
    /// `settled` waits on
    /// (`an_outstanding_deletion_is_reported_until_its_process_stops`). The
    /// default has nothing outstanding.
    fn cleanup_outstanding(&self) -> bool {
        false
    }
}
