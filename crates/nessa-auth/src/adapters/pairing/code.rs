use super::PairingCryptoError;
use crate::domain::pairing::MANUAL_CODE_BYTES;
use opaque_ke::rand::{CryptoRng, RngCore};
use std::fmt;
use zeroize::Zeroizing;

const ALPHABET: &[u8; 32] = b"ABCDEFGHJKLMNPQRSTUVWXYZ23456789";
/// Owned 40-bit invitation secret. Diagnostics redact it and owned bytes erase on drop.
pub struct ManualCode(Zeroizing<[u8; MANUAL_CODE_BYTES]>);
impl ManualCode {
    /// Adopted fixed canonical symbol count, derived from owner contract data.
    pub const LENGTH: usize = MANUAL_CODE_BYTES;
    /// Grouped display byte count including the separator.
    pub const DISPLAY_LENGTH: usize = Self::LENGTH + 1;
    const GROUP_LENGTH: usize = Self::LENGTH / 2;
    /// Generate eight uniformly selected symbols from injected cryptographic entropy.
    pub fn generate(rng: &mut (impl RngCore + CryptoRng)) -> Self {
        let mut bytes = Zeroizing::new([0; Self::LENGTH]);
        rng.fill_bytes(bytes.as_mut());
        for byte in bytes.iter_mut() {
            *byte = ALPHABET[usize::from(*byte & 31)];
        }
        Self(bytes)
    }
    /// Parse exactly eight ASCII symbols or `XXXX-XXXX`; lowercase is accepted.
    /// No whitespace, aliases, Unicode normalization, or trimming is performed.
    pub fn parse(input: &[u8]) -> Result<Self, PairingCryptoError> {
        let mut bytes = Zeroizing::new([0; Self::LENGTH]);
        if input.len() != Self::LENGTH
            && !(input.len() == Self::DISPLAY_LENGTH && input[Self::GROUP_LENGTH] == b'-')
        {
            return Err(PairingCryptoError::InvalidCode);
        }
        for (index, byte) in bytes.iter_mut().enumerate() {
            let input_index = index
                + usize::from(input.len() == Self::DISPLAY_LENGTH && index >= Self::GROUP_LENGTH);
            *byte = input[input_index].to_ascii_uppercase();
            if !ALPHABET.contains(byte) {
                return Err(PairingCryptoError::InvalidCode);
            }
        }
        Ok(Self(bytes))
    }
    /// Borrow the canonical password for the selected PAKE. Never log this slice.
    pub fn expose_bytes(&self) -> &[u8] {
        self.0.as_ref()
    }
    /// Produce owned display bytes for the authorized local secret sink.
    /// The caller must erase any copies made by its output or UI implementation.
    pub fn display_bytes(&self) -> Zeroizing<[u8; Self::DISPLAY_LENGTH]> {
        let mut result = Zeroizing::new([0; Self::DISPLAY_LENGTH]);
        result[..Self::GROUP_LENGTH].copy_from_slice(&self.0[..Self::GROUP_LENGTH]);
        result[Self::GROUP_LENGTH] = b'-';
        result[Self::GROUP_LENGTH + 1..].copy_from_slice(&self.0[Self::GROUP_LENGTH..]);
        result
    }
}
impl fmt::Debug for ManualCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("ManualCode([REDACTED])")
    }
}
#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_normalization_does_not_accept_aliases_or_whitespace() {
        assert_eq!(
            ManualCode::parse(b"abcd-2345").unwrap().expose_bytes(),
            b"ABCD2345"
        );
        for input in [
            b" ABCD2345".as_slice(),
            b"ABCD2345\n",
            b"ABCD2340",
            b"ABCD2341",
            b"ABCD234I",
            b"ABCD234O",
            b"AB-CD2345",
            "ABCD２３４５".as_bytes(),
        ] {
            assert!(ManualCode::parse(input).is_err());
        }
        assert_eq!(
            format!("{:?}", ManualCode::parse(b"ABCD2345").unwrap()),
            "ManualCode([REDACTED])"
        );
    }
}
