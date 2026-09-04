//! Dev-only `server.ping` echo (ADR 0006).
//!
//! Composed into WebSocket dispatch only when the process stage is `dev`.

pub mod entrypoint;
