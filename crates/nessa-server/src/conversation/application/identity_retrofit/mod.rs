//! The one-shot retrofit of conversations saved under the restoration
//! fingerprint that still hashed MCP servers (#391, ADR 344).
//!
//! **Temporary.** This module, its adapters, the SDK's earlier fingerprint
//! and every `previous_identity` that reads it are deleted together in the
//! follow-up #471 ("Delete the #391 one-shot fingerprint retrofit"). The
//! durable fact it appends, `SessionChange::ProviderIdentity`, is permanent
//! and stays.
//!
//! Composition runs it once as the gateway starts, before the conversation
//! service is built:
//!
//! ```text
//!   marker ── done? ──▶ stop
//!     │ not done
//!     ▼
//!   RetrofitConversations ─▶ each id ─▶ ConversationRepository::load
//!                                         │ tombstoned / unsupported ─▶ outcome
//!                                         ▼
//!                           SessionStorage::open_existing ─▶ SavedProviderIdentity::load
//!                                         │ no history ─▶ outcome
//!                                         ▼
//!                           RestorationIdentitySource (current, previous)
//!                                         │ current / foreign ─▶ outcome
//!                                         ▼
//!       IdentityRetrofitAudit: rewriting ─▶ SavedProviderIdentity::move_to ─▶ rewritten | left
//!     ▼
//!   IdentityRetrofitAudit: summary ─▶ nothing transient left ─▶ marker written
//! ```
//!
//! Arrows are the order of calls. Every outside thing is a port here, with its
//! adapter in `conversation::infrastructure` (`DurableIdentityRetrofitAudit`,
//! `FileIdentityRetrofitMarker`, `LocalConversationStore`) or composition
//! (`CurrentAgentResolver`). The state table it implements, and the tests
//! named after each row, are in `docs/design/mcp-connections.md`; ADR 344
//! records the decision.
mod outcome;
mod ports;
mod runner;

pub use outcome::{
    ConversationRetrofit, Leftover, PermanentLeftover, RetrofitRun, RetrofitSummary,
    TransientLeftover,
};
pub use ports::{
    EveryConversation, IdentityRetrofitAudit, IdentityRetrofitMarker, MarkerFuture,
    MarkerUnavailable, RestorationIdentities, RestorationIdentitySource, RetrofitAuditRecord,
    RetrofitCause, RetrofitConversations, RetrofitInitiator, RetrofitMove,
};
pub use runner::{run_identity_retrofit, IdentityRetrofitPorts};

#[cfg(test)]
#[path = "../../../../tests/conversation/identity_retrofit.rs"]
mod tests;
