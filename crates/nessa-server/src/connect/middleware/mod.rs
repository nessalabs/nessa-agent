//! Connect validation gates — fail with a wire error before success response.
//!
//! Each gate returns `Some(ResponseFrame)` on failure, `None` to continue.
//! Order is owned by `entrypoint::handle_connect` (not this module):
//!
//! ```text
//! ConnectParams (+ WsSession)
//!      │
//!      ├── already_connected                 (session flag)
//!      ├── protocol::validate_protocol_range (min/max vs PROTOCOL_VERSION)
//!      ├── metadata::validate_connect_metadata
//!      ├── challenge::validate_challenge_nonce (echo connect.challenge)
//!      └── auth::validate_auth_token         (client token vs server token)
//!              │
//!              ▼
//!         Ok → HelloOk + session.mark_connected()
//!         Err → ResponseFrame::failure on the wire
//! ```
//!
//! Note: `protocol` here is connect-specific validation, not the crate `protocol` module.

pub mod auth;
pub mod challenge;
pub mod metadata;
pub mod protocol;
