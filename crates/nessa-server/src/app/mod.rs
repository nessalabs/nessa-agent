//! Shared runtime state passed to every entrypoint.
//!
//! Built once in `composition` from [`crate::env::Environment`] and cloned into Axum handlers.
//! Holds server-side secrets and metrics (auth token, uptime) — not client session
//! state (that stays in [`crate::server::entrypoint::session::WsSession`] per WebSocket).
//!
//! ```text
//! composition
//!      │
//!      ▼
//! AppState ──clone──► connect::entrypoint
//!              └────► health::entrypoint
//!              └────► server::entrypoint::ws (dispatch)
//! ```

pub mod state;
