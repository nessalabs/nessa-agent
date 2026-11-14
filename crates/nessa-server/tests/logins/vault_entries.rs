//! Saved logins kept in the real vault: a Claude sign-in Boat saved under an
//! older entry name moves to the saved Claude login, encrypted at rest.
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
fn passphrase() -> Passphrase {
    Passphrase::new("a disposable vault test passphrase".into()).unwrap()
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
    assert_eq!(login.read(), Err(crate::secrets::SecretUnavailable::Locked));
    vault.create(&session.lease(), passphrase(), &[]).unwrap();
    assert!(login.read().unwrap().is_none());
    // What an earlier build saved for Boat is found, and kept as the saved
    // Claude login from then on.
    let older = nessa_vault::domain::EntryName::new("boat.claude-sign-in").unwrap();
    vault.write(&older, b"sk-ant-oat01-older").unwrap();
    assert_eq!(
        login.read().unwrap().unwrap().as_slice(),
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
    login.delete().unwrap();
    assert!(login.read().unwrap().is_none());
}
