//! Axum router and WebSocket session loop.
//!
//! `http` registers routes and shared `AppState`. `ws` runs per-connection
//! lifecycle: send `connect.challenge`, read requests, dispatch RPCs.
//! `session` tracks per-socket seq and whether connect succeeded.
//!
//! ```text
//! http::router(AppState)
//!   ├── /health  → health::entrypoint::handle_http_health
//!   └── /        → ws_upgrade → ws::handle_socket
//!                                    │
//!                                    ├── session::WsSession
//!                                    └── dispatch_client_request
//! ```

pub mod http;
pub mod origin;
pub mod session;
pub mod ws;
