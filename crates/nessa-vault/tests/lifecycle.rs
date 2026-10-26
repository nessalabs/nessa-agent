use nessa_vault::{
    application::{Audit, AuditRecord, Caller, Clock, Repository, Vault},
    domain::{EntryName, Failure, Passphrase, Status},
    infrastructure::{EncryptedFileVault, FileAudit, OsEntropy},
};
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
const PASSWORD: &str = "a long vault passphrase for verification";
fn password() -> Passphrase {
    Passphrase::new(PASSWORD.into()).unwrap()
}
fn name() -> EntryName {
    EntryName::new("github-sign-in").unwrap()
}
struct TestClock(AtomicU64);
impl Clock for TestClock {
    fn seconds(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}
fn caller() -> Caller {
    Caller {
        principal: "owner".into(),
        credential: "surface".into(),
    }
}
struct Fixture {
    directory: tempfile::TempDir,
    repository: Arc<EncryptedFileVault>,
    audit: Arc<FileAudit>,
    clock: Arc<TestClock>,
    vault: Arc<Vault>,
}
impl Fixture {
    fn new() -> Self {
        let directory = tempfile::tempdir().unwrap();
        let repository = Arc::new(EncryptedFileVault::new(
            directory.path().join("vault"),
            Arc::new(OsEntropy),
        ));
        let audit = Arc::new(FileAudit::new(directory.path().join("audit")));
        let clock = Arc::new(TestClock(AtomicU64::new(0)));
        let vault = Vault::new(repository.clone(), audit.clone(), clock.clone());
        Self {
            directory,
            repository,
            audit,
            clock,
            vault,
        }
    }
}
#[test]
fn setup_seals_entries_and_a_request_clone_cannot_keep_the_connection_unlocked() {
    let f = Fixture::new();
    assert_eq!(f.vault.status(), Ok(Status::Unconfigured));
    let mut session = f.vault.session("app-one".into(), caller());
    let retained = session.lease();
    assert_eq!(
        f.vault.create(&retained, password(), &[]),
        Ok(Status::Unlocked)
    );
    f.vault
        .write(&name(), b"private-github-token-1132")
        .unwrap();
    let path = f.directory.path().join("vault/entries.enc");
    let encrypted = std::fs::read(&path).unwrap();
    assert!(!encrypted
        .windows(b"private-github-token-1132".len())
        .any(|bytes| bytes == b"private-github-token-1132"));
    assert!(!encrypted
        .windows(PASSWORD.len())
        .any(|bytes| bytes == PASSWORD.as_bytes()));
    assert_eq!(
        f.vault.read(&name()).unwrap().unwrap().as_slice(),
        b"private-github-token-1132"
    );
    session.close().unwrap();
    assert_eq!(f.vault.status(), Ok(Status::Locked));
    assert!(matches!(f.vault.read(&name()), Err(Failure::Locked)));
    assert_eq!(
        f.vault.unlock(&retained, password()),
        Err(Failure::SessionClosed)
    );
    let next = f.vault.session("app-two".into(), caller());
    f.vault.unlock(&next.lease(), password()).unwrap();
    assert_eq!(
        f.vault.read(&name()).unwrap().unwrap().as_slice(),
        b"private-github-token-1132"
    );
    let audit = std::fs::read_to_string(f.directory.path().join("audit/events.json")).unwrap();
    assert!(audit.contains("connection_closed"));
    assert!(!audit.contains(PASSWORD));
    assert!(!audit.contains("private-github-token"));
}
#[test]
fn another_socket_and_an_old_lease_cannot_seal_or_take_over_a_live_unlock() {
    let f = Fixture::new();
    let mut first = f.vault.session("first".into(), caller());
    let mut second = f.vault.session("second".into(), caller());
    f.vault.create(&first.lease(), password(), &[]).unwrap();
    assert_eq!(
        f.vault.unlock(&second.lease(), password()),
        Err(Failure::SessionOwned)
    );
    second.close().unwrap();
    assert_eq!(f.vault.status(), Ok(Status::Unlocked));
    f.vault.lock(&first.lease()).unwrap();
    let third = f.vault.session("third".into(), caller());
    f.vault.unlock(&third.lease(), password()).unwrap();
    first.close().unwrap();
    assert_eq!(f.vault.status(), Ok(Status::Unlocked));
}
#[test]
fn wrong_passphrases_are_bounded_without_rewriting_ciphertext() {
    let f = Fixture::new();
    let session = f.vault.session("session".into(), caller());
    f.vault.create(&session.lease(), password(), &[]).unwrap();
    f.vault.lock(&session.lease()).unwrap();
    let path = f.directory.path().join("vault/entries.enc");
    let bytes = std::fs::read(&path).unwrap();
    for _ in 0..5 {
        assert_eq!(
            f.vault.unlock(
                &session.lease(),
                Passphrase::new("wrong but long enough".into()).unwrap()
            ),
            Err(Failure::WrongPassphrase)
        );
    }
    assert_eq!(
        f.vault.unlock(&session.lease(), password()),
        Err(Failure::Cooldown)
    );
    assert_eq!(std::fs::read(&path).unwrap(), bytes);
    f.clock.0.store(30, Ordering::SeqCst);
    assert_eq!(
        f.vault.unlock(&session.lease(), password()),
        Ok(Status::Unlocked)
    );
}
struct Gated {
    inner: Arc<EncryptedFileVault>,
    started: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl Repository for Gated {
    fn configured(&self) -> Result<bool, Failure> {
        self.inner.configured()
    }
    fn create(
        &self,
        p: &Passphrase,
        e: &[(EntryName, nessa_vault::domain::Secret)],
    ) -> Result<nessa_vault::domain::VaultKey, Failure> {
        self.inner.create(p, e)
    }
    fn unlock(&self, p: &Passphrase) -> Result<nessa_vault::domain::VaultKey, Failure> {
        self.started.send(()).unwrap();
        self.resume.lock().unwrap().recv().unwrap();
        self.inner.unlock(p)
    }
    fn read(
        &self,
        k: &nessa_vault::domain::VaultKey,
        n: &EntryName,
    ) -> Result<Option<nessa_vault::domain::Secret>, Failure> {
        self.inner.read(k, n)
    }
    fn write(
        &self,
        k: &nessa_vault::domain::VaultKey,
        n: &EntryName,
        v: &[u8],
    ) -> Result<(), Failure> {
        self.inner.write(k, n, v)
    }
    fn delete(&self, k: &nessa_vault::domain::VaultKey, n: &EntryName) -> Result<(), Failure> {
        self.inner.delete(k, n)
    }
}
#[test]
fn a_closed_connection_fences_an_unlock_still_deriving_its_key() {
    let f = Fixture::new();
    let setup = f.vault.session("setup".into(), caller());
    f.vault.create(&setup.lease(), password(), &[]).unwrap();
    drop(setup);
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (resume_tx, resume_rx) = mpsc::sync_channel(1);
    let repository = Arc::new(Gated {
        inner: f.repository.clone(),
        started: started_tx,
        resume: Mutex::new(resume_rx),
    });
    let vault = Vault::new(repository, f.audit.clone(), f.clock.clone());
    let mut session = vault.session("blocked".into(), caller());
    let lease = session.lease();
    let worker = {
        let vault = vault.clone();
        let lease = lease.clone();
        std::thread::spawn(move || vault.unlock(&lease, password()))
    };
    started_rx
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert_eq!(vault.unlock(&lease, password()), Err(Failure::Busy));
    session.close().unwrap();
    resume_tx.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), Err(Failure::SessionClosed));
    assert_eq!(vault.status(), Ok(Status::Locked));
}
struct FailableAudit {
    inner: Arc<FileAudit>,
    fail: AtomicBool,
}
impl Audit for FailableAudit {
    fn record(&self, event: &AuditRecord) -> Result<(), Failure> {
        if self.fail.load(Ordering::SeqCst) {
            Err(Failure::AuditUnavailable)
        } else {
            self.inner.record(event)
        }
    }
}
#[test]
fn audit_failure_refuses_admission_and_cannot_prevent_key_clearing() {
    let f = Fixture::new();
    let audit = Arc::new(FailableAudit {
        inner: f.audit.clone(),
        fail: AtomicBool::new(true),
    });
    let vault = Vault::new(f.repository.clone(), audit.clone(), f.clock.clone());
    let mut session = vault.session("audit-failure".into(), caller());
    assert_eq!(
        vault.create(&session.lease(), password(), &[]),
        Err(Failure::AuditUnavailable)
    );
    assert_eq!(vault.status(), Ok(Status::Unconfigured));
    audit.fail.store(false, Ordering::SeqCst);
    vault.create(&session.lease(), password(), &[]).unwrap();
    audit.fail.store(true, Ordering::SeqCst);
    assert_eq!(session.close(), Err(Failure::AuditUnavailable));
    assert_eq!(vault.status(), Ok(Status::Locked));
    assert!(matches!(vault.read(&name()), Err(Failure::Locked)));
}
