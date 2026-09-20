//! Joins the conversation context's readiness port to the warm-up that
//! satisfies it. The two contexts stay independent: the conversation owns the
//! port, the warm-up knows nothing about conversations, and composition is the
//! only place that knows both.
use crate::agent_warm_up::application::AgentWarmUp;
use crate::conversation::application::RuntimeReadiness;
use std::{future::Future, pin::Pin};

pub(super) struct PreparedRuntime(pub(super) AgentWarmUp);

impl RuntimeReadiness for PreparedRuntime {
    fn wait(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.0.wait_until_settled())
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/prepared_runtime.rs"]
mod tests;
