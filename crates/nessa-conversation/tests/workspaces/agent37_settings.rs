//! Agent37's adapter uses the real vault's encryption and unlock fence, and
//! keeps the API key and the SSH private key only inside it.
use super::*;
use crate::workspaces::domain::BoatConnection;
use nessa_vault::{
    application::{Caller, Clock, ObservationClock},
    domain::Passphrase,
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
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
const KEY: &str = "sk_live_agent37-fixture-secret";
const PRIVATE: &[u8] = b"-----BEGIN OPENSSH PRIVATE KEY-----\nfixture-private-bytes\n-----END OPENSSH PRIVATE KEY-----\n";
fn connection() -> CloudConnection {
    CloudConnection::Agent37(
        Agent37Connection::new(
            KEY.into(),
            PRIVATE.to_vec(),
            "ssh-ed25519 AAAAC3NzaC1lZDI1NTE5AAAAIAcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcHBwcH nessa".into(),
            "nessa-runtime@1".into(),
        )
        .unwrap(),
    )
}

#[tokio::test]
async fn the_connection_is_kept_encrypted_and_read_back_whole() {
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
    let settings = Agent37VaultSettings(vault.clone());
    assert!(matches!(settings.read().await, Err(Failure::Locked)));
    vault
        .create(
            &session.lease(),
            Passphrase::new("a disposable vault test passphrase".into()).unwrap(),
            &[],
        )
        .unwrap();
    assert!(matches!(settings.read().await, Err(Failure::NotConfigured)));
    let generation = settings.generation().unwrap();
    let boat = BoatConnection::new("boat-token".into(), "nessa".into(), "image".into()).unwrap();
    assert_eq!(
        settings.save(generation, CloudConnection::Boat(boat)).await,
        Err(Failure::InvalidInput)
    );
    settings.save(generation, connection()).await.unwrap();
    let encrypted = std::fs::read(root.path().join("vault/entries.enc")).unwrap();
    for secret in [KEY.as_bytes(), b"fixture-private-bytes".as_slice()] {
        assert!(!encrypted.windows(secret.len()).any(|bytes| bytes == secret));
    }
    let Ok(CloudConnection::Agent37(saved)) = settings.read().await else {
        panic!("the Agent37 connection was not read back");
    };
    assert_eq!(saved.account.key.as_slice(), KEY.as_bytes());
    assert_eq!(saved.account.ssh_private.as_slice(), PRIVATE);
    assert_eq!(saved.template, "nessa-runtime@1");
    assert!(!format!("{saved:?}").contains("fixture-private-bytes"));
    settings.clear().await.unwrap();
    assert!(matches!(settings.read().await, Err(Failure::NotConfigured)));
    session.close().unwrap();
}
