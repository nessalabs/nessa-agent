//! Optional HTTP adapter. It opens no listener and persists no signing keys.
mod http;
pub use http::{router, InvalidSigningKey, SigningKey, MAX_EVENT_BYTES};
