//! Browser HTTP sign-in, cookie parsing, and origin checks.
//! HTTPS cookies and loopback development cookies remain separate.
mod http;

#[cfg(test)]
pub(crate) use http::encode_session_id;
pub use http::{check, login, logout, Login};
pub(crate) use http::{cookie, origin};
