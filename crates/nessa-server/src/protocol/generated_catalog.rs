//! Generated from `protocol/manifest.json` — do not edit by hand.
//!
//! **Source of truth:** `protocol/manifest.json` (method/event names).
//! Payload structs → `generated_types.rs` (from `protocol/schemas/v1/`).
//! Regenerate: `pnpm protocol:generate`
//!
//! To add a method: edit manifest + schemas, run `pnpm protocol:generate`,
//! wire the handler — do not invent string literals in client/server code.

#![allow(dead_code)]

/// RPC method names on `type: "req"` frames.
pub mod method {
    pub const CONNECT: &str = "connect";
    pub const CONVERSATION_ECHO: &str = "conversation.echo";
    pub const SERVER_HEALTH: &str = "server.health";
    pub const SERVER_PING: &str = "server.ping";
}

/// Server push event names on `type: "event"` frames.
pub mod event {
    pub const CONNECT_CHALLENGE: &str = "connect.challenge";
}
