use crate::contract;
use zeroize::Zeroizing;
/// Secret bytes erase their allocation on drop and redact diagnostic formatting.
#[derive(Clone)]
pub struct Secret(Zeroizing<Vec<u8>>);
impl Secret {
    /// Take ownership of an allocation that will be erased on drop.
    pub fn new(value: Vec<u8>) -> Self {
        Self(Zeroizing::new(value))
    }
    /// Borrow secret bytes for their intended consumer; keep them out of logs.
    pub fn as_slice(&self) -> &[u8] {
        self.0.as_slice()
    }
    /// Transfer the erasing allocation into an existing internal byte-store port.
    pub fn into_zeroizing(self) -> Zeroizing<Vec<u8>> {
        self.0
    }
}
impl std::ops::Deref for Secret {
    type Target = [u8];
    fn deref(&self) -> &Self::Target {
        self.as_slice()
    }
}
impl From<Zeroizing<Vec<u8>>> for Secret {
    fn from(value: Zeroizing<Vec<u8>>) -> Self {
        Self(value)
    }
}
impl std::fmt::Debug for Secret {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("[REDACTED]")
    }
}
/// Validated user input. The shared contract owns strength and size bounds.
pub struct Passphrase(Zeroizing<String>);
impl Passphrase {
    /// Validate Unicode character count and UTF-8 byte bounds from the shared
    /// contract. Rejected input is erased by its owned zeroizing guard too.
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
    /// Admit a bounded lowercase ASCII entry identity, not a filesystem path.
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
    /// Borrow the validated application entry identity.
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
