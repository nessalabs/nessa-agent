use super::{SecretStore, SecretUnavailable};
use sha2::{Digest, Sha256};
use std::path::Path;
use zeroize::Zeroizing;

/// Persistent native storage: Keychain, Credential Manager, or Secret Service.
/// Namespace bytes include stage and instance through the host's canonical path.
pub struct KeychainSecret {
    service: String,
    purpose: &'static str,
}
impl KeychainSecret {
    pub fn new(namespace: &Path, purpose: &'static str) -> Self {
        Self {
            service: format!(
                "ai.nessalabs.nessa.{:x}",
                Sha256::digest(namespace.as_os_str().as_encoded_bytes())
            ),
            purpose,
        }
    }
    #[cfg(not(target_os = "macos"))]
    fn entry(&self) -> Result<keyring::Entry, SecretUnavailable> {
        #[cfg(target_os = "linux")]
        use keyring::secret_service as platform;
        #[cfg(target_os = "windows")]
        use keyring::windows as platform;
        #[cfg(any(target_os = "linux", target_os = "windows"))]
        {
            let credential = platform::default_credential_builder()
                .build(None, &self.service, self.purpose)
                .map_err(|_| SecretUnavailable)?;
            Ok(keyring::Entry::new_with_credential(credential))
        }
        #[cfg(not(any(target_os = "linux", target_os = "windows")))]
        {
            Err(SecretUnavailable)
        }
    }
}
#[cfg(not(target_os = "macos"))]
impl SecretStore for KeychainSecret {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        match self.entry()?.get_secret() {
            Ok(bytes) => Ok(Some(Zeroizing::new(bytes))),
            Err(keyring::Error::NoEntry) => Ok(None),
            Err(_) => Err(SecretUnavailable),
        }
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        self.entry()?
            .set_secret(bytes)
            .map_err(|_| SecretUnavailable)
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        match self.entry()?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => Ok(()),
            Err(_) => Err(SecretUnavailable),
        }
    }
}

#[cfg(target_os = "macos")]
impl SecretStore for KeychainSecret {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        super::read_macos(&self.service, self.purpose)
            .map(|bytes| bytes.map(Zeroizing::new))
            .map_err(|_| SecretUnavailable)
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        security_framework::passwords::set_generic_password(&self.service, self.purpose, bytes)
            .map_err(|_| SecretUnavailable)
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        match security_framework::passwords::delete_generic_password(&self.service, self.purpose) {
            Ok(()) => Ok(()),
            Err(error) if error.code() == security_framework_sys::base::errSecItemNotFound => {
                Ok(())
            }
            Err(_) => Err(SecretUnavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "writes and deletes a disposable native OS credential"]
    fn native_secret_survives_adapter_recreation_and_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let entry = KeychainSecret::new(root.path(), "verification");
        struct Cleanup<'a>(&'a KeychainSecret);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.delete();
            }
        }
        let cleanup = Cleanup(&entry);
        assert!(entry.read().unwrap().is_none());
        entry.write(b"disposable-nessa-boundary-check").unwrap();
        let recreated = KeychainSecret::new(root.path(), "verification");
        assert_eq!(
            recreated.read().unwrap().unwrap().as_slice(),
            b"disposable-nessa-boundary-check"
        );
        entry.delete().unwrap();
        assert!(recreated.read().unwrap().is_none());
        drop(cleanup);
    }
}
