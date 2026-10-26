use crate::contract;
use zeroize::Zeroizing;
/// Secret bytes erase their owned allocation when dropped; Debug is absent.
pub type Secret = Zeroizing<Vec<u8>>;
/// Validated user input. The shared contract owns strength and size bounds.
pub struct Passphrase(Zeroizing<String>);
impl Passphrase {
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
    pub(crate) fn bytes(&self) -> &[u8] {
        self.0.as_bytes()
    }
}
/// Internal, application-chosen entry identity, never a caller-selected wire path.
#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct EntryName(String);
impl EntryName {
    pub fn new(name: impl Into<String>) -> Result<Self, Failure> {
        let name = name.into();
        if name.is_empty()
            || name.len() > 64
            || !name
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        {
            return Err(Failure::InvalidEntry);
        }
        Ok(Self(name))
    }
    pub fn as_str(&self) -> &str {
        &self.0
    }
}
/// Derived key is visible only to the encrypted repository implementation.
pub struct VaultKey(pub(crate) Zeroizing<[u8; 32]>);
/// State without any secret or stored entry metadata.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Unconfigured,
    Locked,
    Unlocked,
}
/// Redacted failures are typed; wrong input and damaged ciphertext do not reset data.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    Locked,
    PassphrasePolicy,
    WrongPassphrase,
    Unavailable,
    Busy,
    Cooldown,
    SessionClosed,
    SessionOwned,
    AlreadyConfigured,
    InvalidEntry,
    Capacity,
    AuditUnavailable,
    MigrationUnavailable,
}
