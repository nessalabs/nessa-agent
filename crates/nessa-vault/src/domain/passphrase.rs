use super::Failure;
use crate::contract;
use zeroize::Zeroizing;

const COMMON_PASSPHRASES: &str = include_str!("../../data/common-passphrases.txt");

/// Bounded existing input. Setup strength is separate so accepted old vaults
/// remain unlockable (`setup_strength.rs::legacy_weak_passphrase_still_unlocks`).
pub struct Passphrase(Zeroizing<String>);
impl Passphrase {
    /// Apply the existing Unicode character and byte bounds, erasing rejected input.
    pub fn new(value: String) -> Result<Self, Failure> {
        let value = Zeroizing::new(value);
        if value.len() > contract::MAXIMUM_PASSPHRASE_BYTES
            || value.chars().count() < contract::MINIMUM_CHARACTERS
            || value.chars().all(char::is_whitespace)
        {
            return Err(Failure::PassphrasePolicy);
        }
        Ok(Self(value))
    }
    /// New setup/change input additionally excludes known common passwords and
    /// periodic repetition. This finite check is not an entropy guarantee.
    pub(crate) fn validate_setup(&self) -> Result<(), Failure> {
        let comparable = self.0.trim();
        if comparable.chars().count() < contract::MINIMUM_SETUP_CHARACTERS
            || COMMON_PASSPHRASES
                .lines()
                .any(|known| known.eq_ignore_ascii_case(comparable))
            || repeated(comparable)
        {
            return Err(Failure::PassphrasePolicy);
        }
        Ok(())
    }
    pub(crate) fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}

fn repeated(text: &str) -> bool {
    let bytes = text.as_bytes();
    (1..=bytes.len() / 2).any(|length| {
        bytes.len().is_multiple_of(length)
            && text.is_char_boundary(length)
            && bytes
                .chunks_exact(length)
                .all(|chunk| chunk.eq_ignore_ascii_case(&bytes[..length]))
    })
}
