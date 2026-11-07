//! Boat connection, and the Claude sign-in made for its VMs, in the existing
//! encrypted vault. Unlock generation fences validation/write and reads; no
//! second keychain or plaintext token file.
use crate::workspaces::{
    application::{ClaudeSignIns, Settings},
    domain::{BoatConnection, ClaudeSignIn, Failure},
};
use async_trait::async_trait;
use nessa_vault::{application::Vault, domain::EntryName};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zeroize::Zeroizing;

pub(crate) struct VaultSettings(pub(crate) Arc<Vault>);
/// The Claude sign-in for Boat VMs, in the same vault and fence.
pub(crate) struct VaultClaudeSignIns(pub(crate) Arc<Vault>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    token: String,
    environment: String,
    image: String,
}
#[derive(Serialize)]
struct Borrowed<'a> {
    token: &'a str,
    environment: &'a str,
    image: &'a str,
}
fn entry() -> EntryName {
    EntryName::new("boat.connection").expect("static vault entry")
}
fn claude_entry() -> EntryName {
    EntryName::new("boat.claude-sign-in").expect("static vault entry")
}
fn failed(error: nessa_vault::domain::Failure) -> Failure {
    if error == nessa_vault::domain::Failure::Locked {
        Failure::Locked
    } else {
        Failure::Storage
    }
}
#[async_trait]
impl Settings for VaultSettings {
    fn generation(&self) -> Result<u64, Failure> {
        self.0.unlocked_generation().ok_or(Failure::Locked)
    }
    async fn read(&self) -> Result<BoatConnection, Failure> {
        let generation = self.generation()?;
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = vault
                .read_in(generation, &entry())
                .map_err(failed)?
                .ok_or(Failure::NotConfigured)?;
            let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| Failure::Storage)?;
            BoatConnection::new(stored.token, stored.environment, stored.image)
                .map_err(|_| Failure::Storage)
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn save(&self, generation: u64, connection: BoatConnection) -> Result<(), Failure> {
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let token = std::str::from_utf8(connection.token.as_slice())
                .map_err(|_| Failure::InvalidInput)?;
            let bytes = Zeroizing::new(
                serde_json::to_vec(&Borrowed {
                    token,
                    environment: &connection.environment,
                    image: &connection.image,
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
#[async_trait]
impl ClaudeSignIns for VaultClaudeSignIns {
    async fn read(&self) -> Result<Option<ClaudeSignIn>, Failure> {
        let generation = self.generation()?;
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            vault
                .read_in(generation, &claude_entry())
                .map_err(failed)?
                .map(|bytes| {
                    let token = String::from_utf8(bytes.to_vec()).map_err(|_| Failure::Storage)?;
                    ClaudeSignIn::new(token).map_err(|_| Failure::Storage)
                })
                .transpose()
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn save(&self, sign_in: ClaudeSignIn) -> Result<(), Failure> {
        let generation = self.generation()?;
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            vault
                .write_in(generation, &claude_entry(), sign_in.as_bytes())
                .map_err(failed)
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn clear(&self) -> Result<(), Failure> {
        let generation = self.generation()?;
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            vault.delete_in(generation, &claude_entry()).map_err(failed)
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
}
impl VaultClaudeSignIns {
    fn generation(&self) -> Result<u64, Failure> {
        self.0.unlocked_generation().ok_or(Failure::Locked)
    }
}
