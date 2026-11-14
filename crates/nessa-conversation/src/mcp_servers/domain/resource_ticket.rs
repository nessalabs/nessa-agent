//! What redeems an app resource's held bytes over HTTP (#348): a ticket of
//! 256 random bits, of which the gateway keeps only a digest.
use base64::{engine::general_purpose::URL_SAFE_NO_PAD, Engine as _};
use sha2::{Digest, Sha256};
use std::fmt;

/// A ticket as said: 32 random bytes, base64url without padding — 43
/// characters of `[A-Za-z0-9_-]`, the protocol schema's `McpReadResourceResult`
/// `ticket` pattern (`a_ticket_is_43_characters_of_the_schemas_alphabet`).
pub fn resource_ticket(bytes: [u8; 32]) -> String {
    URL_SAFE_NO_PAD.encode(bytes)
}

/// What the gateway keeps of a ticket it issued: its SHA-256. A redemption is
/// looked up by the digest of what it presented, so the ticket itself is
/// never compared, stored, or written down; and its audit records name it by
/// [`ResourceTicketDigest::to_hex`].
#[derive(Clone, Copy, PartialEq, Eq, Hash)]
pub struct ResourceTicketDigest([u8; 32]);
impl ResourceTicketDigest {
    /// The digest of `ticket`, whatever bytes it is: a malformed ticket has a
    /// digest too, which simply matches nothing.
    pub fn of(ticket: impl AsRef<[u8]>) -> Self {
        Self(Sha256::digest(ticket.as_ref()).into())
    }

    /// Lowercase hex, as an audit record's `ticket_digest`.
    pub fn to_hex(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
}
impl fmt::Debug for ResourceTicketDigest {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "ResourceTicketDigest({})", self.to_hex())
    }
}

#[cfg(test)]
#[path = "../../../tests/mcp_servers/resource_ticket.rs"]
mod tests;
