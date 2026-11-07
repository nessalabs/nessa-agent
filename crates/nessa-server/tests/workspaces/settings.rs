//! Boat's adapter uses the real vault's encryption and unlock-generation fence.
use crate::workspaces::{
    application::{ClaudeSignIns, Settings},
    domain::{BoatConnection, Failure},
    infrastructure::{VaultClaudeSignIns, VaultSettings},
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
    settings.save(generation, connection()).await.unwrap();
    let encrypted = std::fs::read(root.path().join("vault/entries.enc")).unwrap();
    assert!(!encrypted
        .windows(b"boat-fixture-secret-token".len())
        .any(|bytes| bytes == b"boat-fixture-secret-token"));
    vault.lock(&session.lease()).unwrap();
    assert!(matches!(settings.read().await, Err(Failure::Locked)));
    assert_eq!(settings.clear().await, Err(Failure::Locked));
    vault.unlock(&session.lease(), passphrase()).unwrap();
    assert_eq!(
        settings.save(generation, connection()).await,
        Err(Failure::Locked)
    );
    let saved = settings.read().await.unwrap();
    assert_eq!(saved.token.as_slice(), b"boat-fixture-secret-token");
    assert_eq!(saved.environment, "nessa");
    session.close().unwrap();
    assert!(matches!(settings.read().await, Err(Failure::Locked)));
}

#[tokio::test]
async fn a_claude_sign_in_is_the_saved_claude_login_and_an_older_entry_moves_there() {
    use nessa_protocol::agents::AgentId;
    let root = tempfile::tempdir().unwrap();
    let vault = Vault::new(
        Arc::new(EncryptedFileVault::new(
            root.path().join("vault"),
            Arc::new(OsEntropy),
        )),
        Arc::new(FileAudit::new(root.path().join("audit"), Arc::new(Time))),
        Arc::new(Time),
    );
    let session = vault.session(
        "fixture-socket".into(),
        Caller {
            principal: "fixture-owner".into(),
            credential: "fixture-credential".into(),
        },
    );
    let login = crate::logins::infrastructure::login_stores(&vault)[&AgentId::Claude].clone();
    let sign_ins = VaultClaudeSignIns(login.clone());
    assert!(matches!(sign_ins.read().await, Err(Failure::Locked)));
    vault.create(&session.lease(), passphrase(), &[]).unwrap();
    assert!(sign_ins.read().await.unwrap().is_none());
    // What an earlier build saved for Boat is found, and kept as the saved
    // Claude login from then on.
    let older = nessa_vault::domain::EntryName::new("boat.claude-sign-in").unwrap();
    vault.write(&older, b"sk-ant-oat01-older").unwrap();
    assert_eq!(
        sign_ins.read().await.unwrap().unwrap().as_bytes(),
        b"sk-ant-oat01-older"
    );
    assert!(vault.read(&older).unwrap().is_none());
    let claude = crate::logins::domain::login_entry(AgentId::Claude);
    assert_eq!(
        vault
            .read(&claude)
            .unwrap()
            .unwrap()
            .into_zeroizing()
            .as_slice(),
        b"sk-ant-oat01-older"
    );
    login.write(b"sk-ant-oat01-second").unwrap();
    let encrypted = std::fs::read(root.path().join("vault/entries.enc")).unwrap();
    assert!(!encrypted
        .windows(b"sk-ant-oat01".len())
        .any(|w| w == b"sk-ant-oat01"));
    // One fact: Saved logins reads what Boat saved, and a removal there is
    // gone for Boat too.
    assert_eq!(
        login.read().unwrap().unwrap().as_slice(),
        b"sk-ant-oat01-second"
    );
    // An API key saved as the Claude login is not Boat's sign-in: each
    // lease hands it over instead.
    login.write(b"sk-ant-api03-key").unwrap();
    assert!(sign_ins.read().await.unwrap().is_none());
    login.delete().unwrap();
    assert!(sign_ins.read().await.unwrap().is_none());
}
