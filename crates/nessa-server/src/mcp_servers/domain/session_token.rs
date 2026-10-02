//! What ties a stand-in to the conversation its harness was opened for: a
//! token the gateway issues for each open, of which it keeps only a digest.
use sha2::{Digest, Sha256};
use std::fmt;

/// The environment variable a stand-in finds its session's token in. It is
/// set only in the stand-in's own environment, never in its arguments, so it
/// stays out of a harness's context fingerprint.
pub const SESSION_VARIABLE: &str = "NESSA_MCP_SESSION";

/// A token as said: 32 random bytes, lowercase hex.
pub fn session_token(bytes: [u8; 32]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

/// What the gateway's registry of grants keeps of a token it issued: its
/// SHA-256. A hello's token is looked up by its digest; the token itself
/// lives only in the grant handed to the SDK, and in the stand-ins'
/// environments.
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct TokenDigest([u8; 32]);
impl TokenDigest {
    /// The digest of `token`, whatever it is.
    pub fn of(token: &str) -> Self {
        Self(Sha256::digest(token.as_bytes()).into())
    }
}
impl fmt::Debug for TokenDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TokenDigest(..)")
    }
}
