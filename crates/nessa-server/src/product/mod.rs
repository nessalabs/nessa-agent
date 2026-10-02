//! Mandatory authenticated product WebSocket profile served at `/session`.
//!
//! `socket` owns authenticated receive admission and a separate bounded writer.
//! Its queue/deadline ordering tests live in `tests/product/socket/writer.rs`.
//! `record_read` owns the physical page's product JSON/base64 conversion and
//! response mapping; `passive_read` owns the shared encoded byte ceiling. Conversation application and infrastructure own read
//! authority and SDK source work; this module does not infer that authority.
//!
//! ```text
//! request -> socket -> conversation application -> record source
//! response <- writer <- passive_read writer <- record_read codec <- page
//! ```
//! Arrows show calls and returned data, not shared ownership of the source.

mod attachment;
pub(crate) mod catalogue_read;
pub(crate) mod generated;
pub(crate) mod passive_read;
pub(crate) mod record_read;
mod socket;
mod state;
pub(crate) mod wire;

pub use socket::handle_socket;
pub use state::{InvalidSessionSettings, ProductDependencies, ProductRouteState, SessionSettings};
pub use wire::{SessionAuthenticateParams, SessionChallenge, SessionReady};

mod conversation;
mod mcp_apps;

mod agent_install;
