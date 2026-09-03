//! Connect handshake — first RPC every client must complete.
//!
//! Validates protocol version and auth token, returns `HelloOk` with scopes and
//! Split into `middleware` (gates) and `entrypoint` (handler) so S2+
//! features can follow the same vertical layout.
//!
//! ```text
//! ClientRequest::Connect { params: ConnectParams }
//!        │
//!        ▼
//! middleware::protocol::validate_protocol_range
//!        │
//!        ▼
//! middleware::auth::validate_auth_token  (params.auth vs AppState)
//!        │
//!        ▼
//! entrypoint::handle_connect ──► OutgoingMessage (HelloOk)
//! ```

pub mod entrypoint;
pub mod middleware;
