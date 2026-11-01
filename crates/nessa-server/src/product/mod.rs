//! Mandatory authenticated product WebSocket profile served at `/session`.
//!
//! `socket` owns authenticated receive admission and a separate bounded writer.
//! Its queue/deadline ordering tests live in `tests/product/socket/writer.rs`.
//! `record_read` and `catalogue_read` dispatch admitted reads; the page codecs,
//! the shared encoded byte ceiling, the generated DTOs and the handshake rules
//! are `nessa_protocol::product`, which a device client reads with too.
//! Conversation application and infrastructure own read authority and SDK
//! source work; this module does not infer that authority.
//!
//! ```text
//! request -> socket -> conversation application -> record source
//! response <- writer <- nessa_protocol passive_read writer <- record_read codec <- page
//! ```
//! Arrows show calls and returned data, not shared ownership of the source.
//!
//! `native` serves the same session over a protected native connection
//! (`device_pairing`'s `openProduct`): only the credential verifier differs,
//! through `socket::SessionProof`.
//!
//! `mcp_servers` translates `mcpServers.list`, `.save`, `.remove` and
//! `.inspect` into `mcp_servers::application::McpServerSettings` (#391).
//!
//! `subscription` serves `conversation.subscribe*` and `agents.subscribe`: the
//! one live read path a client follows a conversation, its list or the agent
//! catalog by. Its frames share the socket's one writer and its one event
//! numbering (`event_sequence`) with permission review notices.

mod attachment;
pub(crate) mod catalogue_read;
mod event_sequence;
mod operational_limits;
mod owner_phone;
pub(crate) mod passive_read;
mod phone;
mod phone_endpoint;
mod read_access;
pub(crate) mod record_read;
mod refusal_causes;
mod socket;
mod state;
mod subscription;
mod webhook_reset;
pub(crate) mod wire;
mod workspace;

pub(crate) use operational_limits::ConfiguredLimits;
pub use operational_limits::{InvalidOperationalLimits, OperationalLimits};
pub use socket::handle_socket;
pub(crate) use socket::{RECORD_LANE, RECORD_SLOT, REFUSAL_LANE};
pub(crate) use state::ConnectedEnvironments;
pub use state::{InvalidSessionSettings, ProductDependencies, ProductRouteState, SessionSettings};

mod boat;
mod channel;
mod conversation;
mod github;
mod mcp_apps;
pub(crate) mod mcp_servers;
mod native;
mod pairing;
mod peers;
mod voice;

pub use github::GithubRoutes;
pub use native::{DeviceCredentials, NativeSessions};

mod agent_install;

#[cfg(test)]
pub(crate) use conversation::dispatch as dispatch_conversation;

#[cfg(test)]
pub(crate) use socket::HostProductFixture;

mod vault;
pub use vault::VaultRoutes;
