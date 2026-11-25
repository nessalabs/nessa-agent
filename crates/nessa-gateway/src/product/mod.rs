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
//!
//! `turn_timing` notes, on this machine only, how long a send waited for the
//! first reply text. `nessa doctor` reads that file; nothing is sent.
//!
//! `shared_chat` reads a peer cache into `conversation.list` and
//! `conversation.read` for the owner's own surface. The cache stays its own
//! database. A write on that chat is refused here; the grantor is not called.

mod admission;
mod attachment;
pub mod catalogue_read;
mod event_sequence;
mod operational_limits;
mod owner_phone;
pub mod passive_read;
mod phone;
mod phone_endpoint;
mod read_access;
pub mod record_read;
mod refusal_causes;
pub mod shared_chat;
mod socket;
mod state;
mod subscription;
pub mod turn_timing;
mod webhook_reset;
pub mod wire;
mod workspace;

pub use operational_limits::ConfiguredLimits;
pub use operational_limits::{InvalidOperationalLimits, OperationalLimits};
pub use socket::handle_socket;
pub use socket::{RECORD_LANE, RECORD_SLOT, REFUSAL_LANE};
pub use state::ConnectedEnvironments;
pub use state::{InvalidSessionSettings, ProductDependencies, ProductRouteState, SessionSettings};

mod agent37;
mod boat;
mod channel;
mod cloud;
mod cloud_boxes;
mod conversation;
mod device_setup;
mod github;
mod logins;
mod mcp_apps;
pub mod mcp_servers;
mod native;
mod pairing;
mod peers;
mod pool;
mod reply_style;
mod terminal;
mod voice;

pub use github::GithubRoutes;
pub use native::{DeviceCredentials, NativeSessions};

mod agent_install;

#[cfg(test)]
pub use conversation::dispatch as dispatch_conversation;

#[cfg(any(test, feature = "test-support"))]
pub use socket::HostProductFixture;

mod trusted_apps;
mod vault;
pub use vault::VaultRoutes;
