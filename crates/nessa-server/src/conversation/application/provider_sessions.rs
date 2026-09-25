use super::{ConversationError, ConversationFuture};
use crate::agents::domain::AgentId;
use crate::conversation::domain::ProviderSessionErasure;
use futures_util::future::join_all;
use nessa_sdk::domain::agent_execution::sessions::ExecutionSessionId;
use std::{collections::HashMap, future::Future, pin::Pin, sync::Arc};

/// Asks one agent to delete its own record of a provider session nothing runs
/// any more, and says what that settled.
///
/// The port a conversation's deletion reaches an agent's store through. What
/// an answer means is the agent's binding's to say, so an implementation
/// reports it as that binding reports it and claims nothing more: an agent
/// that archives is not reported as having erased.
///
/// # Errors
/// [`ConversationError::Agent`] with the typed reason the agent could not be
/// asked or refused; the deletion stays unfinished and is asked again.
pub trait ProviderSessionEraser: Send + Sync {
    fn erase(&self, session: ExecutionSessionId) -> ConversationFuture<'_, ProviderSessionErasure>;
    /// Resolves once every ask this eraser started, and whose caller stopped
    /// waiting, has finished cleaning up after itself — within the eraser's
    /// own bound. Awaited by retirement before the runtime ends. The default
    /// has nothing outstanding.
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(async {})
    }
}

/// Every agent's [`ProviderSessionEraser`], keyed by agent: the one authority
/// a deletion asks about the agent's own record, and which decides who is
/// asked. Composition registers one per agent it builds; nothing else names an
/// agent here.
///
/// Two agents are never asked, and they are told apart. A conversation naming
/// an agent this build has no adapter for is answered
/// [`ProviderSessionErasure::NoHandler`]: final, since no run of this build
/// can ask it. An agent this build does have an adapter for — every
/// [`AgentId`] — that composition did not build this run, because it is not
/// configured or its runtime is missing, is
/// [`ConversationError::AgentNotConfigured`]: temporary, so the deletion stays
/// unfinished and a later start, with the agent built, asks it
/// (`erasure_is_dispatched_by_agent_and_an_agent_with_no_handler_is_answered_no_handler`,
/// `an_agent_not_built_this_run_leaves_the_deletion_unfinished_until_it_is`).
#[derive(Clone, Default)]
pub struct ProviderSessionErasers {
    erasers: HashMap<AgentId, Arc<dyn ProviderSessionEraser>>,
}
impl ProviderSessionErasers {
    /// Make `eraser` the one asked for conversations on `agent`, in place of
    /// any registered before it.
    pub fn register(&mut self, agent: AgentId, eraser: Arc<dyn ProviderSessionEraser>) {
        self.erasers.insert(agent, eraser);
    }
    /// Whether an eraser is registered for `agent`.
    #[cfg(test)]
    pub(crate) fn handles(&self, agent: AgentId) -> bool {
        self.erasers.contains_key(&agent)
    }
    /// Until every registered eraser has settled what it started.
    pub async fn settled(&self) {
        join_all(self.erasers.values().map(|eraser| eraser.settled())).await;
    }
    /// Who is asked about a session of a conversation on `agent`: the
    /// decision, taken before anybody is asked, so a caller that has to spend
    /// something to ask — an agent launch — spends it only when somebody will
    /// be.
    ///
    /// `agent` is `None` for a conversation naming an agent this build has no
    /// adapter for, which is answered [`ProviderSessionHandler::NoHandler`].
    ///
    /// # Errors
    /// [`ConversationError::AgentNotConfigured`] for an agent not built this
    /// run.
    pub fn handler(
        &self,
        agent: Option<AgentId>,
    ) -> Result<ProviderSessionHandler, ConversationError> {
        let Some(agent) = agent else {
            return Ok(ProviderSessionHandler::NoHandler);
        };
        self.erasers
            .get(&agent)
            .cloned()
            .map(ProviderSessionHandler::Ask)
            .ok_or(ConversationError::AgentNotConfigured)
    }
}

/// Who [`ProviderSessionErasers::handler`] says is asked.
pub enum ProviderSessionHandler {
    /// Nobody can ask this conversation's agent, ever: record
    /// [`ProviderSessionErasure::NoHandler`].
    NoHandler,
    /// Ask this eraser.
    Ask(Arc<dyn ProviderSessionEraser>),
}
