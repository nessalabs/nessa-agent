//! Mandatory authenticated product WebSocket profile served at `/session`.

mod generated;
mod socket;
mod state;
mod wire;

pub use socket::handle_socket;
pub use state::{InvalidSessionSettings, ProductDependencies, ProductRouteState, SessionSettings};
pub use wire::{SessionAuthenticateParams, SessionChallenge, SessionReady};

mod conversation;
