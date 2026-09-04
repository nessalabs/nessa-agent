//! Nessa Client API server — WebSocket control plane (S1: connect + health +
//! optional `server.ping` when stage is `dev`).
//!
//! Crate layout mirrors livelance-style verticals: a thin composition root wires
//! config and shared state, then HTTP/WebSocket entrypoints route typed protocol
//! messages to feature handlers.
//!
//! ```text
//!                    ┌─────────────┐
//!                    │  core::run  │
//!                    └──────┬──────┘
//!                           │
//!                    ┌──────▼──────────┐
//!                    │  composition    │  wires env → state → router → bind
//!                    └──────┬──────────┘
//!           ┌────────────────┼────────────────┐
//!           │                │                │
//!    ┌──────▼──────┐  ┌──────▼──────┐  ┌──────▼──────┐
//!    │     env     │  │     app     │  │   server    │
//!    │  (config)   │  │   (state)   │  │ (transport) │
//!    └─────────────┘  └──────┬──────┘  └──────┬──────┘
//!                            │                │
//!                     ┌──────┴──────┐         │ decode / encode
//!                     │ connect     │◄────────┤
//!                     │ health      │◄────────┤
//!                     │ ping (dev)  │◄────────┘
//!                     └──────┬──────┘
//!                            │
//!                     ┌──────▼──────┐
//!                     │  protocol   │  typed wire frames (SSOT with protocol/)
//!                     └─────────────┘
//! ```

pub mod app;
pub mod composition;
pub mod connect;
pub mod conversation;
pub mod core;
pub mod env;
pub mod health;
pub mod ping;
pub mod protocol;
pub mod server;

pub use core::run;
