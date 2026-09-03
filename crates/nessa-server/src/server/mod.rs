//! Transport layer — HTTP and WebSocket entrypoints.
//!
//! Owns how clients reach the server (Axum routes, upgrade, socket loop) but not
//! feature business rules. The WebSocket loop decodes protocol messages and
//! delegates to `connect` and `health` entrypoints.
//!
//! ```text
//! Client
//!   │  GET /health ──────────────► health::entrypoint (HTTP probe)
//!   │  WS  /      ──upgrade──► entrypoint::ws::handle_socket
//!   │                              │
//!   │                              ├── protocol::decode_client_request
//!   │                              └── dispatch → connect | health
//!   ▼
//! JSON frames on the wire
//! ```

pub mod entrypoint;
