//! Connect handshake — first RPC every client must complete.
//!
//! Validates protocol version, metadata, challenge nonce, and auth token, then
//! returns `HelloOk`. Split into `middleware` (gates) and `entrypoint` (handler)
//! so S2+ features can follow the same vertical layout.
//!
//! ```text
//! ClientRequest::Connect { params: ConnectParams }
//!        │
//!        ▼
//! entrypoint::handle_connect(session, …)
//!        │
//!        ├── already_connected?
//!        ├── middleware::protocol::validate_protocol_range
//!        ├── middleware::metadata::validate_connect_metadata
//!        ├── middleware::challenge::validate_challenge_nonce
//!        └── middleware::auth::validate_auth_token
//!                │
//!                ▼
//!           OutgoingMessage (HelloOk) + session.mark_connected()
//! ```

pub mod entrypoint;
pub mod middleware;
