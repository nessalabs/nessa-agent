//! Secret Service with encrypted D-Bus sessions and no interactive unlock path.
//! Zero prompt timeout also covers a collection locking after admission.
use super::{KeychainSecret, SecretStore, SecretUnavailable};
use dbus_secret_service::{EncryptionType, SecretService};
use std::collections::HashMap;
use zeroize::Zeroizing;

impl KeychainSecret {
    fn attributes(&self) -> HashMap<&str, &str> {
        HashMap::from([
            ("service", self.service.as_str()),
            ("username", self.purpose),
        ])
    }
    fn service(&self) -> Result<SecretService, SecretUnavailable> {
        SecretService::connect_with_max_prompt_timeout(EncryptionType::Dh, 0)
            .map_err(|_| SecretUnavailable::Unavailable)
    }
}
impl SecretStore for KeychainSecret {
    fn read(&self) -> Result<Option<Zeroizing<Vec<u8>>>, SecretUnavailable> {
        let service = self.service()?;
        let collection = service
            .get_default_collection()
            .map_err(|_| SecretUnavailable::Unavailable)?;
        collection
            .ensure_unlocked()
            .map_err(|_| SecretUnavailable::Unavailable)?;
        let items = collection
            .search_items(self.attributes())
            .map_err(|_| SecretUnavailable::Unavailable)?;
        match items.as_slice() {
            [] => Ok(None),
            [item] => {
                item.ensure_unlocked()
                    .map_err(|_| SecretUnavailable::Unavailable)?;
                item.get_secret()
                    .map(Zeroizing::new)
                    .map(Some)
                    .map_err(|_| SecretUnavailable::Unavailable)
            }
            _ => Err(SecretUnavailable::Unavailable),
        }
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        let service = self.service()?;
        let collection = service
            .get_default_collection()
            .map_err(|_| SecretUnavailable::Unavailable)?;
        collection
            .ensure_unlocked()
            .map_err(|_| SecretUnavailable::Unavailable)?;
        let items = collection
            .search_items(self.attributes())
            .map_err(|_| SecretUnavailable::Unavailable)?;
        match items.as_slice() {
            [] => {
                collection
                    .create_item(
                        "Nessa GitHub",
                        self.attributes(),
                        bytes,
                        false,
                        "application/octet-stream",
                    )
                    .map_err(|_| SecretUnavailable::Unavailable)?;
                Ok(())
            }
            [item] => {
                item.ensure_unlocked()
                    .map_err(|_| SecretUnavailable::Unavailable)?;
                item.set_secret(bytes, "application/octet-stream")
                    .map_err(|_| SecretUnavailable::Unavailable)
            }
            _ => Err(SecretUnavailable::Unavailable),
        }
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        let service = self.service()?;
        let collection = service
            .get_default_collection()
            .map_err(|_| SecretUnavailable::Unavailable)?;
        collection
            .ensure_unlocked()
            .map_err(|_| SecretUnavailable::Unavailable)?;
        let items = collection
            .search_items(self.attributes())
            .map_err(|_| SecretUnavailable::Unavailable)?;
        match items.as_slice() {
            [] => Ok(()),
            [item] => {
                item.ensure_unlocked()
                    .map_err(|_| SecretUnavailable::Unavailable)?;
                item.delete().map_err(|_| SecretUnavailable::Unavailable)
            }
            _ => Err(SecretUnavailable::Unavailable),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    #[ignore = "locks the default collection; requires the disposable CI Secret Service"]
    fn locked_disposable_service_refuses_without_prompting() {
        assert_eq!(
            std::env::var("NESSA_TEST_SECRET_SERVICE").as_deref(),
            Ok("disposable")
        );
        let root = tempfile::tempdir().unwrap();
        let entry = KeychainSecret::new(root.path(), "verification").unwrap();
        entry.write(b"disposable-only").unwrap();
        assert_eq!(
            entry.read().unwrap().unwrap().as_slice(),
            b"disposable-only"
        );
        let service = entry.service().unwrap();
        let collection = service.get_default_collection().unwrap();
        collection.lock().unwrap();
        let before = std::time::Instant::now();
        assert_eq!(entry.read(), Err(SecretUnavailable::Unavailable));
        assert_eq!(entry.write(b"refused"), Err(SecretUnavailable::Unavailable));
        assert_eq!(entry.delete(), Err(SecretUnavailable::Unavailable));
        assert!(before.elapsed() < std::time::Duration::from_secs(10));
        // The CI job owns this collection and destroys its VM after the check.
    }
}
