//! A named encrypted-vault entry behind the existing consuming secret-store seam.
use super::{SecretStore, SecretUnavailable};
use nessa_vault::{application::Vault, domain::EntryName};
use std::sync::Arc;
pub struct VaultSecret {
    vault: Arc<Vault>,
    name: EntryName,
}
impl VaultSecret {
    pub fn new(vault: Arc<Vault>, name: EntryName) -> Self {
        Self { vault, name }
    }
}
impl SecretStore for VaultSecret {
    fn read(&self) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, SecretUnavailable> {
        self.vault
            .read(&self.name)
            .map(|value| value.map(|secret| secret.into_zeroizing()))
            .map_err(secret_failure)
    }
    fn write(&self, bytes: &[u8]) -> Result<(), SecretUnavailable> {
        self.vault.write(&self.name, bytes).map_err(secret_failure)
    }
    fn delete(&self) -> Result<(), SecretUnavailable> {
        self.vault.delete(&self.name).map_err(secret_failure)
    }
}

fn secret_failure(problem: nessa_vault::domain::Failure) -> SecretUnavailable {
    if problem == nessa_vault::domain::Failure::Locked {
        SecretUnavailable::Locked
    } else {
        SecretUnavailable::Unavailable
    }
}

impl crate::github::application::GithubSecret for VaultSecret {
    fn read(
        &self,
        generation: u64,
    ) -> Result<Option<zeroize::Zeroizing<Vec<u8>>>, crate::github::application::StoreUnavailable>
    {
        self.vault
            .read_in(generation, &self.name)
            .map(|value| value.map(|secret| secret.into_zeroizing()))
            .map_err(|_| crate::github::application::StoreUnavailable)
    }
    fn write(
        &self,
        generation: u64,
        bytes: &[u8],
    ) -> Result<(), crate::github::application::StoreUnavailable> {
        self.vault
            .write_in(generation, &self.name, bytes)
            .map_err(|_| crate::github::application::StoreUnavailable)
    }
    fn delete(&self, generation: u64) -> Result<(), crate::github::application::StoreUnavailable> {
        self.vault
            .delete_in(generation, &self.name)
            .map_err(|_| crate::github::application::StoreUnavailable)
    }
}
