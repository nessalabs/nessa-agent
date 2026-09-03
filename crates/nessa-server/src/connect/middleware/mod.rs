//! Connect validation gates — run before the handler, fail with a wire error.
//!
//! Each gate returns `Some(ResponseFrame)` on failure, `None` to continue.
//! Order matters: protocol range first, then auth.
//!
//! ```text
//! ConnectParams
//!      │
//!      ├── protocol::validate_protocol_range  (min/max vs PROTOCOL_VERSION)
//!      │
//!      ├── challenge::validate_challenge_nonce (echo connect.challenge)
//!      │
//!      └── auth::validate_auth_token         (client token vs server token)
//!              │
//!              ▼
//!         Ok → entrypoint::handle_connect
//!         Err → ResponseFrame::failure on the wire
//! ```
//!
//! Note: `protocol` here is connect-specific validation, not the crate `protocol` module.

pub mod auth;
pub mod challenge;
pub mod metadata;
pub mod protocol;
