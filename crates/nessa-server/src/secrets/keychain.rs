#[cfg(not(target_os = "linux"))]
use super::SecretStore;
use super::SecretUnavailable;
use sha2::{Digest, Sha256};
use std::path::Path;
#[cfg(not(target_os = "linux"))]
use zeroize::Zeroizing;

/// Persistent native storage: Keychain, Credential Manager, or Secret Service.
/// Namespace bytes include stage and instance through the host's canonical path.
pub struct KeychainSecret {
    pub(super) service: String,
    pub(super) purpose: &'static str,
}
impl KeychainSecret {
    pub fn new(namespace: &Path, purpose: &'static str) -> Result<Self, SecretUnavailable> {
        let namespace = std::fs::canonicalize(namespace).map_err(|_| SecretUnavailable)?;
        Ok(Self {
            service: format!(
                "ai.nessalabs.nessa.{:x}",
                Sha256::digest(namespace.as_os_str().as_encoded_bytes())
            ),
            purpose,
        })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    fn entry(&self) -> Result<keyring::Entry, SecretUnavailable> {
        #[cfg(target_os = "windows")]
        use keyring::windows as platform;
        #[cfg(target_os = "windows")]
        {
            let credential = platform::default_credential_builder()
                .build(None, &self.service, self.purpose)
                .map_err(|_| SecretUnavailable)?;
            Ok(keyring::Entry::new_with_credential(credential))
        }
        #[cfg(not(target_os = "windows"))]
        {
            Err(SecretUnavailable)
        }
    }
}
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
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
        let keychain = super::macos::default_keychain()?;
        super::macos::read_in(&keychain, &self.service, self.purpose)
            .map(|bytes| bytes.map(Zeroizing::new))
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        let keychain = super::macos::default_keychain()?;
        super::macos::write_in(&keychain, &self.service, self.purpose, bytes)
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        let keychain = super::macos::default_keychain()?;
        super::macos::delete_in(&keychain, &self.service, self.purpose)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(target_os = "linux")]
    use crate::secrets::SecretStore;
    #[test]
    #[cfg(unix)]
    fn aliases_share_an_identity_and_missing_namespaces_refuse() {
        let root = tempfile::tempdir().unwrap();
        let namespace = root.path().join("namespace");
        std::fs::create_dir(&namespace).unwrap();
        let alias = root.path().join("alias");
        std::os::unix::fs::symlink(&namespace, &alias).unwrap();
        let original = KeychainSecret::new(&namespace, "verification").unwrap();
        let aliased = KeychainSecret::new(&alias, "verification").unwrap();
        assert_eq!(original.service, aliased.service);
        assert!(matches!(
            KeychainSecret::new(&root.path().join("missing"), "verification"),
            Err(SecretUnavailable)
        ));
    }
    #[test]
    #[ignore = "writes and deletes a disposable native OS credential"]
    fn native_secret_survives_adapter_recreation_and_is_deleted() {
        let root = tempfile::tempdir().unwrap();
        let entry = KeychainSecret::new(root.path(), "verification").unwrap();
        struct Cleanup<'a>(&'a KeychainSecret);
        impl Drop for Cleanup<'_> {
            fn drop(&mut self) {
                let _ = self.0.delete();
            }
        }
        let cleanup = Cleanup(&entry);
        assert!(entry.read().unwrap().is_none());
        entry.write(b"disposable-nessa-boundary-check").unwrap();
        let recreated = KeychainSecret::new(root.path(), "verification").unwrap();
        assert_eq!(
            recreated.read().unwrap().unwrap().as_slice(),
            b"disposable-nessa-boundary-check"
        );
        entry.delete().unwrap();
        assert!(recreated.read().unwrap().is_none());
        drop(cleanup);
    }
}
