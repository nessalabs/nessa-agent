use nessa_vault::{
    application::{Audit, AuditRecord, Caller, Clock, Repository, Vault},
    domain::{EntryName, Failure, Passphrase, Status},
    infrastructure::{EncryptedFileVault, OsEntropy},
};
use std::sync::Arc;
struct ClockZero;
impl Clock for ClockZero {
    fn seconds(&self) -> u64 {
        0
    }
}
struct NoAudit;
impl Audit for NoAudit {
    fn record(&self, _: &AuditRecord) -> Result<(), Failure> {
        Ok(())
    }
}
fn phrase() -> Passphrase {
    Passphrase::new("independent cobalt lantern orchard verification".into()).unwrap()
}
#[test]
fn independent_namespaces_have_separate_keys_entries_and_session_lifetimes() {
    let root = tempfile::tempdir().unwrap();
    let a = Arc::new(EncryptedFileVault::new(
        root.path().join("gateway-a"),
        Arc::new(OsEntropy),
    ));
    let b = Arc::new(EncryptedFileVault::new(
        root.path().join("gateway-b"),
        Arc::new(OsEntropy),
    ));
    // The same chosen phrase does not make independently salted files share keys.
    let key_a = a.create(&phrase(), &[]).unwrap();
    let key_b = b.create(&phrase(), &[]).unwrap();
    let entry = EntryName::new("synthetic-entry").unwrap();
    a.write(&key_a, &entry, b"gateway-a-synthetic").unwrap();
    b.write(&key_b, &entry, b"gateway-b-synthetic").unwrap();
    assert!(matches!(a.read(&key_b, &entry), Err(Failure::Unavailable)));
    assert!(matches!(b.read(&key_a, &entry), Err(Failure::Unavailable)));
    let a = Vault::new(a, Arc::new(NoAudit), Arc::new(ClockZero));
    let b = Vault::new(b, Arc::new(NoAudit), Arc::new(ClockZero));
    let caller = || Caller {
        principal: "synthetic-owner".into(),
        credential: "synthetic-device".into(),
    };
    let mut owner_a = a.session("same-session-name".into(), caller());
    let owner_b = b.session("same-session-name".into(), caller());
    a.unlock(&owner_a.lease(), phrase()).unwrap();
    b.unlock(&owner_b.lease(), phrase()).unwrap();
    owner_a.close().unwrap();
    assert_eq!(a.status(), Ok(Status::Locked));
    assert_eq!(b.status(), Ok(Status::Unlocked));
    assert_eq!(
        b.read(&entry).unwrap().unwrap().as_slice(),
        b"gateway-b-synthetic"
    );
}
