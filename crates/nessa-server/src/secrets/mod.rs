//! OS-held secrets. Application owners receive a single scoped entry, never an
//! ambient credential-store lookup or a plaintext-file fallback.
use zeroize::Zeroizing;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SecretUnavailable;

/// One named secret. OS failures remain distinct from a missing entry.
pub trait SecretStore: Send + Sync {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable>;
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable>;
    fn delete(&self) -> Result<(), SecretUnavailable>;
}
mod keychain;
pub use keychain::KeychainSecret;

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub(crate) use macos::{read as read_macos, SecretReadFailure};

#[cfg(target_os = "linux")]
mod linux;
