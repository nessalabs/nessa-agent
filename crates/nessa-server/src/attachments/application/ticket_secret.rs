use crate::attachments::domain::TicketFingerprint;
use sha2::{Digest, Sha256};
use std::fmt;

/// The secret half of a ticket: 32 random bytes, written as 64 lowercase
/// hexadecimal digits. It exists only on its way to the caller and on its way
/// back; the service keeps its fingerprint. It has no `Display`, and `Debug`
/// prints nothing, so it cannot reach a log by formatting.
#[derive(Clone, PartialEq, Eq)]
pub struct TicketSecret([u8; 32]);
impl TicketSecret {
    pub fn from_bytes(bytes: [u8; 32]) -> Self {
        Self(bytes)
    }
    /// Parse exactly 64 lowercase hexadecimal digits; anything else is not a ticket.
    pub fn parse(value: &str) -> Option<Self> {
        if value.len() != 64 {
            return None;
        }
        let mut bytes = [0_u8; 32];
        for (byte, [high, low]) in bytes.iter_mut().zip(value.as_bytes().as_chunks::<2>().0) {
            *byte = (nibble(*high)? << 4) | nibble(*low)?;
        }
        Some(Self(bytes))
    }
    /// The text to hand to the caller, once.
    pub fn expose(&self) -> String {
        self.0.iter().map(|byte| format!("{byte:02x}")).collect()
    }
    pub fn fingerprint(&self) -> TicketFingerprint {
        TicketFingerprint::from_bytes(Sha256::digest(self.0).into())
    }
}
impl fmt::Debug for TicketSecret {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TicketSecret(..)")
    }
}

fn nibble(digit: u8) -> Option<u8> {
    match digit {
        b'0'..=b'9' => Some(digit - b'0'),
        b'a'..=b'f' => Some(digit - b'a' + 10),
        _ => None,
    }
}
