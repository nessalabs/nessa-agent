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
//! `change_watch` owns original watch permits and first task faults; normal host
//! cleanup in composition consumes its close/drain through ProductRouteState.

mod attachment;
pub(crate) mod catalogue_read;
mod change_watch;
mod event_sequence;
mod operational_limits;
pub(crate) mod passive_read;
pub(crate) mod record_read;
mod socket;
mod state;
mod subscription;
pub(crate) mod wire;

pub(crate) use operational_limits::ConfiguredLimits;
pub use operational_limits::{InvalidOperationalLimits, OperationalLimits};
pub use socket::handle_socket;
pub(crate) use socket::{RECORD_LANE, RECORD_SLOT, REFUSAL_LANE};
pub use state::{InvalidSessionSettings, ProductDependencies, ProductRouteState, SessionSettings};

mod conversation;
mod mcp_apps;
pub(crate) mod mcp_servers;
mod native;
mod pairing;

pub use native::{DeviceCredentials, NativeSessions};

mod agent_install;

pub use change_watch::WatchTaskFault;

#[cfg(test)]
pub(crate) use socket::HostWatchFixture;
