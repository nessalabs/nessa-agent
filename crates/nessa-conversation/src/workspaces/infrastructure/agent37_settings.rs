//! Agent37 connection in the existing encrypted vault, as the Boat one is:
//! the workspace API key, the install's SSH key pair and the pinned
//! template release. Unlock generation fences validation/write and reads;
//! nothing of it is on disk outside the vault.
use crate::workspaces::{
    application::Settings,
    domain::{Agent37Connection, CloudConnection, Failure},
};
use async_trait::async_trait;
use base64::Engine;
use nessa_vault::{application::Vault, domain::EntryName};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zeroize::Zeroizing;

pub struct Agent37VaultSettings(pub Arc<Vault>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    key: String,
    ssh_private: String,
    ssh_public: String,
    template: String,
}
#[derive(Serialize)]
struct Borrowed<'a> {
    key: &'a str,
    ssh_private: &'a str,
    ssh_public: &'a str,
    template: &'a str,
}
fn entry() -> EntryName {
    EntryName::new("agent37.connection").expect("static vault entry")
}
fn failed(error: nessa_vault::domain::Failure) -> Failure {
    if error == nessa_vault::domain::Failure::Locked {
        Failure::Locked
    } else {
        Failure::Storage
    }
}
#[async_trait]
impl Settings for Agent37VaultSettings {
    fn generation(&self) -> Result<u64, Failure> {
        self.0.unlocked_generation().ok_or(Failure::Locked)
    }
    async fn read(&self) -> Result<CloudConnection, Failure> {
        let generation = self.generation()?;
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = vault
                .read_in(generation, &entry())
                .map_err(failed)?
                .ok_or(Failure::NotConfigured)?;
            let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| Failure::Storage)?;
            let encoded = Zeroizing::new(stored.ssh_private);
            let private = base64::engine::general_purpose::STANDARD
                .decode(encoded.as_bytes())
                .map_err(|_| Failure::Storage)?;
            Agent37Connection::new(stored.key, private, stored.ssh_public, stored.template)
                .map(CloudConnection::Agent37)
                .map_err(|_| Failure::Storage)
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn save(&self, generation: u64, connection: CloudConnection) -> Result<(), Failure> {
        let CloudConnection::Agent37(connection) = connection else {
            return Err(Failure::InvalidInput);
        };
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let account = &connection.account;
            let key =
                std::str::from_utf8(account.key.as_slice()).map_err(|_| Failure::InvalidInput)?;
            let private = Zeroizing::new(
                base64::engine::general_purpose::STANDARD.encode(account.ssh_private.as_slice()),
            );
            let bytes = Zeroizing::new(
                serde_json::to_vec(&Borrowed {
                    key,
                    ssh_private: &private,
                    ssh_public: &account.ssh_public,
                    template: &connection.template,
                })
                .map_err(|_| Failure::Storage)?,
            );
            vault.write_in(generation, &entry(), &bytes).map_err(failed)
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn clear(&self) -> Result<(), Failure> {
        let generation = self.generation()?;
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || vault.delete_in(generation, &entry()).map_err(failed))
            .await
            .map_err(|_| Failure::Storage)?
    }
}

#[cfg(test)]
#[path = "../../../tests/workspaces/agent37_settings.rs"]
mod tests;
