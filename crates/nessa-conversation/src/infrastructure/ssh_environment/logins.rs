//! Which saved login, if any, this gateway hands a harness it starts on a
//! host (issue #1260).
//!
//! ```text
//! open(lease) ──▶ Grant ──▶ Granted{terms}
//!   terms.takesLogin? (the host's owner's grant, never this gateway's choice)
//!     no  ──▶ every Start under the lease carries none
//!     yes ──▶ SavedLogins::login(environment, agent, lease)
//!               the policy: may this environment receive logins? (Cedar)
//!               no ──▶ none
//!               yes ──▶ the vault's saved login for the agent, if one is saved
//!   ──▶ Start{lease, channel, environment, login}
//! ```
//!
//! Arrows are calls, in order. The vault is the one place a login is kept:
//! it is read once per lease and held only by that lease's host in memory,
//! so a login replaced or removed applies from the next lease. A login that
//! expires (a ChatGPT access token) says when, and the lease's hold answers
//! it as `renew_by`, by which the parking sweep stops the agent once idle. Which login
//! goes to which host is decided in composition, over the vault and the
//! policy; this module only asks.
use crate::application::EnvironmentFuture;
use nessa_protocol::{agents::AgentId, lease::Login};
use nessa_sdk::domain::agent_execution::leases::EnvironmentRef;

/// A login handed to one lease, and when its agent should be started again
/// with a fresh one: Unix milliseconds, set for a ChatGPT access token
/// (`docs/design/chatgpt-sign-in.md`), `None` for a login that does not
/// expire under the agent.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct HandedLogin {
    pub login: Login,
    pub renew_by_ms: Option<u64>,
}

/// The saved login a harness of `agent` started on `environment` is given,
/// when the policy lets that environment receive one and one is saved.
/// `lease` is recorded with the answer, so each handoff is traced to the
/// lease that asked. Implemented in composition over the vault and the
/// policy.
pub trait SavedLogins: Send + Sync {
    fn login<'a>(
        &'a self,
        environment: &'a EnvironmentRef,
        agent: AgentId,
        lease: &'a str,
    ) -> EnvironmentFuture<'a, Option<HandedLogin>>;
}
