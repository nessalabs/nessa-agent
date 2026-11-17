//! Health handlers — HTTP probe and WebSocket RPC.
//!
//! `handler::handle_http_health` — minimal HTTP liveness.
//! `handler::handle_health_check` — typed `server.health` reply for clients.

pub mod handler;
