//! Boat's adapter uses the real vault's encryption and unlock-generation fence.
use crate::workspaces::{
    application::Settings,
    domain::{BoatConnection, CloudConnection, Failure},
    infrastructure::VaultSettings,
};
use nessa_vault::{
    application::{Caller, Clock, ObservationClock, Vault},
    domain::Passphrase,
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
use std::sync::Arc;
struct Time;
impl Clock for Time {
    fn seconds(&self) -> u64 {
        0
    }
}
impl ObservationClock for Time {
    fn unix_millis(&self) -> Result<u64, nessa_vault::domain::Failure> {
        Ok(0)
    }
}
fn connection() -> BoatConnection {
    BoatConnection::new(
        "boat-fixture-secret-token".into(),
        "nessa".into(),
        "fixture-worker".into(),
    )
    .unwrap()
}
fn passphrase() -> Passphrase {
    Passphrase::new("a disposable vault test passphrase".into()).unwrap()
}

#[tokio::test]
async fn tokens_are_encrypted_and_stale_unlocks_cannot_publish_or_clear_the_new_connection() {
    let root = tempfile::tempdir().unwrap();
    let vault = Vault::new(
        Arc::new(EncryptedFileVault::new(
            root.path().join("vault"),
            Arc::new(OsEntropy),
        )),
        Arc::new(FileAudit::new(root.path().join("audit"), Arc::new(Time))),
        Arc::new(Time),
    );
    let mut session = vault.session(
        "fixture-socket".into(),
        Caller {
            principal: "fixture-owner".into(),
            credential: "fixture-credential".into(),
        },
    );
    let settings = VaultSettings(vault.clone());
    assert!(matches!(settings.read().await, Err(Failure::Locked)));
    vault.create(&session.lease(), passphrase(), &[]).unwrap();
    let generation = settings.generation().unwrap();
    settings
        .save(generation, CloudConnection::Boat(connection()))
        .await
        .unwrap();
    let encrypted = std::fs::read(root.path().join("vault/entries.enc")).unwrap();
    assert!(!encrypted
        .windows(b"boat-fixture-secret-token".len())
        .any(|bytes| bytes == b"boat-fixture-secret-token"));
    vault.lock(&session.lease()).unwrap();
    assert!(matches!(settings.read().await, Err(Failure::Locked)));
    assert_eq!(settings.clear().await, Err(Failure::Locked));
    vault.unlock(&session.lease(), passphrase()).unwrap();
    assert_eq!(
        settings
            .save(generation, CloudConnection::Boat(connection()))
            .await,
        Err(Failure::Locked)
    );
    let Ok(CloudConnection::Boat(saved)) = settings.read().await else {
        panic!("the saved Boat connection was not read back");
    };
    assert_eq!(saved.account.token.as_slice(), b"boat-fixture-secret-token");
    assert_eq!(saved.environment, "nessa");
    session.close().unwrap();
    assert!(matches!(settings.read().await, Err(Failure::Locked)));
}

#[tokio::test]
async fn a_sign_in_grant_is_saved_encrypted_and_read_back_whole() {
    let root = tempfile::tempdir().unwrap();
    let vault = Vault::new(
        Arc::new(EncryptedFileVault::new(
            root.path().join("vault"),
            Arc::new(OsEntropy),
        )),
        Arc::new(FileAudit::new(root.path().join("audit"), Arc::new(Time))),
        Arc::new(Time),
    );
    let mut session = vault.session(
        "fixture-socket".into(),
        Caller {
            principal: "fixture-owner".into(),
            credential: "fixture-credential".into(),
        },
    );
    let settings = VaultSettings(vault.clone());
    vault.create(&session.lease(), passphrase(), &[]).unwrap();
    let generation = settings.generation().unwrap();
    let signed_in = connection().with_grant(
        crate::workspaces::domain::BoatGrant::new(
            "boatrt_fixture-refresh".into(),
            1_234_567,
            "octo".into(),
        )
        .unwrap(),
    );
    settings
        .save(generation, CloudConnection::Boat(signed_in))
        .await
        .unwrap();
    let encrypted = std::fs::read(root.path().join("vault/entries.enc")).unwrap();
    assert!(!encrypted
        .windows(b"boatrt_fixture-refresh".len())
        .any(|bytes| bytes == b"boatrt_fixture-refresh"));
    let Ok(CloudConnection::Boat(saved)) = settings.read().await else {
        panic!("the signed-in Boat connection was not read back");
    };
    let grant = saved.grant.expect("the grant was kept");
    assert_eq!(grant.refresh.as_slice(), b"boatrt_fixture-refresh");
    assert_eq!(grant.expires_at_ms, 1_234_567);
    assert_eq!(grant.login, "octo");
    // A pasted token saved over it leaves no grant behind.
    settings
        .save(generation, CloudConnection::Boat(connection()))
        .await
        .unwrap();
    let Ok(CloudConnection::Boat(saved)) = settings.read().await else {
        panic!("the pasted token was not read back");
    };
    assert!(saved.grant.is_none());
    session.close().unwrap();
}
