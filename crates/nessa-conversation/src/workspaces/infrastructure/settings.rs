//! Boat connection in the existing encrypted vault. Unlock generation
//! fences validation/write and reads; no second keychain or plaintext token
//! file. The Claude sign-in is the saved Claude login (`login.claude`,
//! issue #1260), handed to a VM only on the lease, after that handoff is
//! recorded.
use crate::workspaces::{
    application::Settings,
    domain::{BoatConnection, BoatGrant, CloudConnection, Failure},
};
use async_trait::async_trait;
use nessa_vault::{application::Vault, domain::EntryName};
use serde::{Deserialize, Serialize};
use std::sync::Arc;
use zeroize::Zeroizing;

pub struct VaultSettings(pub Arc<Vault>);
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Stored {
    token: String,
    environment: String,
    image: String,
    /// Only for Sign in with Boat; a pasted token has none.
    #[serde(default)]
    grant: Option<StoredGrant>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields, rename_all = "camelCase")]
struct StoredGrant {
    refresh: String,
    expires_at_ms: u64,
    login: String,
}
#[derive(Serialize)]
struct Borrowed<'a> {
    token: &'a str,
    environment: &'a str,
    image: &'a str,
    #[serde(skip_serializing_if = "Option::is_none")]
    grant: Option<BorrowedGrant<'a>>,
}
#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct BorrowedGrant<'a> {
    refresh: &'a str,
    expires_at_ms: u64,
    login: &'a str,
}
fn entry() -> EntryName {
    EntryName::new("boat.connection").expect("static vault entry")
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
    async fn read(&self) -> Result<CloudConnection, Failure> {
        let generation = self.generation()?;
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let bytes = vault
                .read_in(generation, &entry())
                .map_err(failed)?
                .ok_or(Failure::NotConfigured)?;
            let stored: Stored = serde_json::from_slice(&bytes).map_err(|_| Failure::Storage)?;
            let connection = BoatConnection::new(stored.token, stored.environment, stored.image)
                .map_err(|_| Failure::Storage)?;
            let connection = match stored.grant {
                None => connection,
                Some(grant) => connection.with_grant(
                    BoatGrant::new(grant.refresh, grant.expires_at_ms, grant.login)
                        .map_err(|_| Failure::Storage)?,
                ),
            };
            Ok(CloudConnection::Boat(connection))
        })
        .await
        .map_err(|_| Failure::Storage)?
    }
    async fn save(&self, generation: u64, connection: CloudConnection) -> Result<(), Failure> {
        let CloudConnection::Boat(connection) = connection else {
            return Err(Failure::InvalidInput);
        };
        let vault = self.0.clone();
        tokio::task::spawn_blocking(move || {
            let token = std::str::from_utf8(connection.account.token.as_slice())
                .map_err(|_| Failure::InvalidInput)?;
            let grant = match &connection.grant {
                None => None,
                Some(grant) => Some(BorrowedGrant {
                    refresh: std::str::from_utf8(grant.refresh.as_slice())
                        .map_err(|_| Failure::InvalidInput)?,
                    expires_at_ms: grant.expires_at_ms,
                    login: &grant.login,
                }),
            };
            let bytes = Zeroizing::new(
                serde_json::to_vec(&Borrowed {
                    token,
                    environment: &connection.environment,
                    image: &connection.image,
                    grant,
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
