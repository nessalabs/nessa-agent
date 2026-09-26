//! The clock an ACP binding measures its protocol budgets on.
//!
//! A budget is how long the binding waits for an agent's answer before that
//! wait is a [`AgentError::Deadline`](crate::application::agent_execution::agents::AgentError::Deadline).
//! Composition supplies [`RuntimeClock`]; a test supplies a clock whose
//! budgets run out when the test says, so what a budget running out means is
//! tested without waiting for one, and nothing else a test checks can be cut
//! short by a slow machine (`a_refused_delete_the_list_cannot_explain_stays_the_refusal`).
//!
//! Session deletion is measured on it. Opening a session and executing on one
//! are not yet (#209): their budgets are still read from the runtime directly.
//! The process's own stop budgets (`shutdown_grace`, `kill_timeout`) stay
//! there, since they wait on the operating system rather than the agent.
#![deny(missing_docs)]

use std::{future::Future, pin::Pin, time::Duration};

/// Resolves once the budget it was started for has run out.
pub type BudgetExpiry = Pin<Box<dyn Future<Output = ()> + Send>>;

/// Where an ACP binding's protocol budgets are measured.
pub trait AcpClock: Send + Sync {
    /// Start a budget of `limit` now. The future resolves when it has run out;
    /// it is not polled again after that.
    fn budget(&self, limit: Duration) -> BudgetExpiry;
}

/// The async runtime's own clock: a budget runs out `limit` after it starts.
#[derive(Clone, Copy, Debug, Default)]
pub struct RuntimeClock;
impl AcpClock for RuntimeClock {
    fn budget(&self, limit: Duration) -> BudgetExpiry {
        Box::pin(tokio::time::sleep(limit))
    }
}
