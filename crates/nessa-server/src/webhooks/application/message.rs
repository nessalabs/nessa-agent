use std::fmt;

use nessa_auth::application::ports::CredentialEvidence;
use uuid::Uuid;

/// HTTP message capacity, checked before parsing a record or retaining a body.
pub const MAX_BODY_BYTES: usize = 128 * 1024;

/// Reuses auth's redacted secret storage. No serialization or cloning of keys.
pub struct MacKey(CredentialEvidence);

impl MacKey {
    pub fn new(bytes: [u8; 32]) -> Self {
        Self(CredentialEvidence::new(bytes.to_vec()).expect("32-byte MAC key"))
    }

    /// Borrow for the cryptographic adapter; never include in audit or logs.
    pub fn expose_bytes(&self) -> &[u8] {
        self.0.expose_bytes()
    }
}

impl fmt::Debug for MacKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MacKey([REDACTED])")
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct InvalidMessage;

/// Exact persisted message for one delivery. Debug hides conversation content.
pub struct Message {
    id: Uuid,
    timestamp_ms: u64,
    body: Vec<u8>,
}

impl Message {
    /// Infrastructure supplies an encoded, bounded envelope. Construction does
    /// not grant access to the conversation whose record it carries.
    pub(crate) fn encoded(
        id: Uuid,
        timestamp_ms: u64,
        body: Vec<u8>,
    ) -> Result<Self, InvalidMessage> {
        if body.len() > MAX_BODY_BYTES {
            return Err(InvalidMessage);
        }
        Ok(Self {
            id,
            timestamp_ms,
            body,
        })
    }

    pub fn id(&self) -> Uuid {
        self.id
    }
    pub fn timestamp_ms(&self) -> u64 {
        self.timestamp_ms
    }
    pub fn body(&self) -> &[u8] {
        &self.body
    }
}

impl fmt::Debug for Message {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("Message([REDACTED])")
    }
}
