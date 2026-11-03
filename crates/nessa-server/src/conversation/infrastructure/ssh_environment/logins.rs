//! Which saved login, if any, this gateway hands a harness it starts on a
//! host (issue #1260).
//!
//! ```text
//! open(lease) ──▶ Grant ──▶ Granted{terms}
//!   terms.takesLogin? (the host's owner's grant, never this gateway's choice)
//!     no  ──▶ every Start under the lease carries none
//!     yes ──▶ SavedLogins::login(environment, agent)
//!               the policy: may this environment receive logins? (Cedar)
//!               no ──▶ none
//!               yes ──▶ the vault's saved login for the agent, if one is saved
//!   ──▶ Start{lease, channel, environment, login}
//! ```
//!
//! Arrows are calls, in order. The vault is the one place a login is kept:
//! it is read once per lease and held only by that lease's host in memory,
//! so a login replaced or removed applies from the next lease. Which login
//! goes to which host is decided in composition, over the vault and the
//! policy; this module only asks.
use crate::conversation::application::EnvironmentFuture;
use nessa_protocol::{agents::AgentId, lease::Login};
use nessa_sdk::domain::agent_execution::leases::EnvironmentRef;

/// The saved login a harness of `agent` started on `environment` is given,
/// when the policy lets that environment receive one and one is saved.
/// Implemented in composition over the vault and the policy.
pub(crate) trait SavedLogins: Send + Sync {
    fn login<'a>(
        &'a self,
        environment: &'a EnvironmentRef,
        agent: AgentId,
    ) -> EnvironmentFuture<'a, Option<Login>>;
}
