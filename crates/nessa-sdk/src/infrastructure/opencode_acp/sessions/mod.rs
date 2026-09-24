//! Configures and opens an Opencode context through shared ACP sessions.
//!
//! ```text
//! host config -> OpencodeAcpProvider -> private process root -> ACP session
//!                                         |
//!                          Opencode selections and their checks
//! ```
//! Arrows show construction and validation. Process startup, resume, and the
//! launch-input fingerprint that identifies a restorable context remain shared
//! ACP responsibilities, not provider-specific lifecycle implementations. The
//! private process root isolates code-loading configuration and account data;
//! composition supplies the one credential admitted for the process.
mod binding;
mod profile;
pub use binding::OpencodeAcpProvider;
