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
        let audit = Arc::new(FileAudit::new(
            directory.path().join("audit"),
            Arc::new(TestClock(AtomicU64::new(0))),
        ));
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
    fn change_passphrase(
        &self,
        key: &nessa_vault::domain::VaultKey,
        passphrase: &Passphrase,
    ) -> nessa_vault::domain::ChangePublication {
        self.inner.change_passphrase(key, passphrase)
    }
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
#[test]
fn an_explicit_lock_fences_an_unlock_still_deriving_its_key() {
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
    let session = vault.session("blocked".into(), caller());
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
    assert_eq!(vault.lock(&lease), Ok(Status::Locked));
    resume_tx.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), Err(Failure::Locked));
    assert_eq!(vault.status(), Ok(Status::Locked));
}
#[test]
fn another_connection_cannot_cancel_an_unlock_still_deriving_its_key() {
    let f = Fixture::new();
    f.repository.create(&password(), &[]).unwrap();
    let (started, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let vault = Vault::new(
        Arc::new(Gated {
            inner: f.repository.clone(),
            started,
            resume: Mutex::new(released),
        }),
        f.audit.clone(),
        f.clock.clone(),
    );
    let owner = vault.session("pending-owner".into(), caller());
    let stranger = vault.session("other-connection".into(), caller());
    let opening = {
        let vault = vault.clone();
        let lease = owner.lease();
        std::thread::spawn(move || vault.unlock(&lease, password()))
    };
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let refused = vault.lock(&stranger.lease());
    // Release the worker even when testing the pre-fix behavior.
    resume.send(()).unwrap();
    let opened = opening.join().unwrap();
    assert_eq!(refused, Err(Failure::SessionOwned));
    assert_eq!(opened, Ok(Status::Unlocked));
    assert_eq!(vault.lock(&stranger.lease()), Err(Failure::SessionOwned));
    assert_eq!(vault.status(), Ok(Status::Unlocked));
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

impl nessa_vault::application::ObservationClock for TestClock {
    fn unix_millis(&self) -> Result<u64, nessa_vault::domain::Failure> {
        Ok(1793030400000)
    }
}

#[test]
fn durable_audit_retains_injected_observation_time_across_sessions_and_secrets_redact() {
    let f = Fixture::new();
    let session = f.vault.session("first".into(), caller());
    f.vault.create(&session.lease(), password(), &[]).unwrap();
    drop(session);
    let session = f.vault.session("second".into(), caller());
    f.vault.unlock(&session.lease(), password()).unwrap();
    f.vault.write(&name(), b"do not print this").unwrap();
    let value = f.vault.read(&name()).unwrap().unwrap();
    assert_eq!(format!("{value:?}"), "[REDACTED]");
    let rows: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.directory.path().join("audit/events.json")).unwrap(),
    )
    .unwrap();
    let rows = rows.as_array().unwrap();
    assert!(rows
        .iter()
        .all(|row| row["observedAtUnixMs"] == 1793030400000_u64));
    assert!(rows
        .windows(2)
        .all(|pair| pair[1]["sequence"].as_u64().unwrap()
            == pair[0]["sequence"].as_u64().unwrap() + 1));
}

struct FailPublication(Arc<FileAudit>);
impl Audit for FailPublication {
    fn record(&self, event: &AuditRecord) -> Result<(), Failure> {
        if event.result == "published" {
            Err(Failure::AuditUnavailable)
        } else {
            self.0.record(event)
        }
    }
}
#[test]
fn a_failed_publication_receipt_clears_the_key_and_preserves_the_encrypted_vault() {
    let f = Fixture::new();
    let vault = Vault::new(
        f.repository.clone(),
        Arc::new(FailPublication(f.audit.clone())),
        f.clock.clone(),
    );
    let session = vault.session("publication-failure".into(), caller());
    assert_eq!(
        vault.create(&session.lease(), password(), &[]),
        Err(Failure::AuditUnavailable)
    );
    assert_eq!(vault.status(), Ok(Status::Locked));
    assert!(matches!(vault.read(&name()), Err(Failure::Locked)));
    let session = f.vault.session("retry".into(), caller());
    assert_eq!(
        f.vault.unlock(&session.lease(), password()),
        Ok(Status::Unlocked)
    );
}

struct GatedPublication {
    inner: Arc<FileAudit>,
    entered: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl Audit for GatedPublication {
    fn record(&self, event: &AuditRecord) -> Result<(), Failure> {
        if event.result == "published" {
            self.entered.send(()).unwrap();
            self.resume.lock().unwrap().recv().unwrap();
        }
        self.inner.record(event)
    }
}
#[test]
fn close_during_publication_audit_retains_the_key_clearing_transition() {
    let f = Fixture::new();
    let session = f.vault.session("setup".into(), caller());
    f.vault.create(&session.lease(), password(), &[]).unwrap();
    drop(session);
    let (entered, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let vault = Vault::new(
        f.repository.clone(),
        Arc::new(GatedPublication {
            inner: f.audit.clone(),
            entered,
            resume: Mutex::new(released),
        }),
        f.clock.clone(),
    );
    let mut session = vault.session("raced".into(), caller());
    let lease = session.lease();
    let opening = {
        let vault = vault.clone();
        let lease = lease.clone();
        std::thread::spawn(move || vault.unlock(&lease, password()))
    };
    waiting
        .recv_timeout(std::time::Duration::from_secs(5))
        .unwrap();
    let closing = std::thread::spawn(move || session.close());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    while lease.is_active() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    resume.send(()).unwrap();
    assert_eq!(opening.join().unwrap(), Err(Failure::SessionClosed));
    closing.join().unwrap().unwrap();
    assert_eq!(vault.status(), Ok(Status::Locked));
    let rows: serde_json::Value = serde_json::from_slice(
        &std::fs::read(f.directory.path().join("audit/events.json")).unwrap(),
    )
    .unwrap();
    let rows = rows.as_array().unwrap();
    let events: Vec<_> = rows.iter().filter(|row| row["lease"] == "raced").collect();
    assert_eq!(events[events.len() - 2]["result"], "published");
    assert_eq!(events.last().unwrap()["result"], "key_cleared");
    assert_eq!(events.last().unwrap()["after"], "locked");
}

#[test]
fn stale_consumer_reads_writes_and_rollbacks_cannot_touch_a_new_unlock() {
    let f = Fixture::new();
    let first = f.vault.session("first".into(), caller());
    f.vault.create(&first.lease(), password(), &[]).unwrap();
    let old = f.vault.unlocked_generation().unwrap();
    f.vault.write_in(old, &name(), b"old").unwrap();
    drop(first);
    let second = f.vault.session("second".into(), caller());
    f.vault.unlock(&second.lease(), password()).unwrap();
    let current = f.vault.unlocked_generation().unwrap();
    assert_ne!(old, current);
    f.vault.write_in(current, &name(), b"new").unwrap();
    assert!(matches!(
        f.vault.read_in(old, &name()),
        Err(Failure::Locked)
    ));
    assert_eq!(
        f.vault.write_in(old, &name(), b"stale"),
        Err(Failure::Locked)
    );
    assert_eq!(f.vault.delete_in(old, &name()), Err(Failure::Locked));
    assert_eq!(
        f.vault
            .read_in(current, &name())
            .unwrap()
            .unwrap()
            .as_slice(),
        b"new"
    );
}

#[test]
fn failed_opening_releases_its_reservation_for_another_connection() {
    let f = Fixture::new();
    f.repository.create(&password(), &[]).unwrap();
    let first = f.vault.session("failed-opening".into(), caller());
    let second = f.vault.session("next-opening".into(), caller());
    assert_eq!(
        f.vault.unlock(
            &first.lease(),
            Passphrase::new("wrong synthetic passphrase".into()).unwrap()
        ),
        Err(Failure::WrongPassphrase)
    );
    assert_eq!(f.vault.lock(&second.lease()), Ok(Status::Locked));
    assert_eq!(
        f.vault.unlock(&second.lease(), password()),
        Ok(Status::Unlocked)
    );
}

#[test]
fn failed_opening_audit_releases_its_reservation_for_another_connection() {
    let f = Fixture::new();
    let audit = Arc::new(FailableAudit {
        inner: f.audit.clone(),
        fail: AtomicBool::new(true),
    });
    let vault = Vault::new(f.repository.clone(), audit.clone(), f.clock.clone());
    let first = vault.session("audit-refused-opening".into(), caller());
    let second = vault.session("after-audit-refusal".into(), caller());
    assert_eq!(
        vault.create(&first.lease(), password(), &[]),
        Err(Failure::AuditUnavailable)
    );
    assert_eq!(vault.lock(&second.lease()), Ok(Status::Unconfigured));
    audit.fail.store(false, Ordering::SeqCst);
    assert_eq!(
        vault.create(&second.lease(), password(), &[]),
        Ok(Status::Unlocked)
    );
}

struct QuietAudit;
impl Audit for QuietAudit {
    fn record(&self, _: &AuditRecord) -> Result<(), Failure> {
        Ok(())
    }
}
struct RejectGuesses {
    inner: Arc<EncryptedFileVault>,
    reject: AtomicBool,
}
impl Repository for RejectGuesses {
    fn change_passphrase(
        &self,
        key: &nessa_vault::domain::VaultKey,
        passphrase: &Passphrase,
    ) -> nessa_vault::domain::ChangePublication {
        self.inner.change_passphrase(key, passphrase)
    }
    fn configured(&self) -> Result<bool, Failure> {
        self.inner.configured()
    }
    fn create(
        &self,
        passphrase: &Passphrase,
        initial: &[(EntryName, nessa_vault::domain::Secret)],
    ) -> Result<nessa_vault::domain::VaultKey, Failure> {
        self.inner.create(passphrase, initial)
    }
    fn unlock(&self, passphrase: &Passphrase) -> Result<nessa_vault::domain::VaultKey, Failure> {
        if self.reject.load(Ordering::SeqCst) {
            Err(Failure::WrongPassphrase)
        } else {
            self.inner.unlock(passphrase)
        }
    }
    fn read(
        &self,
        key: &nessa_vault::domain::VaultKey,
        name: &EntryName,
    ) -> Result<Option<nessa_vault::domain::Secret>, Failure> {
        self.inner.read(key, name)
    }
    fn write(
        &self,
        key: &nessa_vault::domain::VaultKey,
        name: &EntryName,
        value: &[u8],
    ) -> Result<(), Failure> {
        self.inner.write(key, name, value)
    }
    fn delete(&self, key: &nessa_vault::domain::VaultKey, name: &EntryName) -> Result<(), Failure> {
        self.inner.delete(key, name)
    }
}
fn controller(principal: &str, credential: &str) -> Caller {
    Caller {
        principal: principal.into(),
        credential: credential.into(),
    }
}
fn rejecting_vault(f: &Fixture) -> (Arc<Vault>, Arc<RejectGuesses>) {
    let repository = Arc::new(RejectGuesses {
        inner: f.repository.clone(),
        reject: AtomicBool::new(true),
    });
    (
        Vault::new(repository.clone(), Arc::new(QuietAudit), f.clock.clone()),
        repository,
    )
}
#[test]
fn controller_cooldowns_are_independent_and_survive_reconnect_and_other_success() {
    let f = Fixture::new();
    let (vault, repository) = rejecting_vault(&f);
    let owner = vault.session("owner-first".into(), controller("owner", "desktop"));
    vault.create(&owner.lease(), password(), &[]).unwrap();
    vault.lock(&owner.lease()).unwrap();
    let adversary = vault.session("bad-first".into(), controller("owner", "other-device"));
    for _ in 0..5 {
        assert_eq!(
            vault.unlock(&adversary.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    drop(adversary);
    let adversary = vault.session(
        "bad-reconnected".into(),
        controller("owner", "other-device"),
    );
    assert_eq!(
        vault.unlock(&adversary.lease(), password()),
        Err(Failure::Cooldown)
    );
    repository.reject.store(false, Ordering::SeqCst);
    assert_eq!(
        vault.unlock(&owner.lease(), password()),
        Ok(Status::Unlocked)
    );
    vault.lock(&owner.lease()).unwrap();
    // A different successful controller must not reset this penalty.
    assert_eq!(
        vault.unlock(&adversary.lease(), password()),
        Err(Failure::Cooldown)
    );
    f.clock.0.store(30, Ordering::SeqCst);
    assert_eq!(
        vault.unlock(&adversary.lease(), password()),
        Ok(Status::Unlocked)
    );
}
#[test]
fn controller_partial_failures_survive_reconnect_and_distinct_principals_are_separate() {
    let f = Fixture::new();
    let (vault, _) = rejecting_vault(&f);
    let initial = vault.session("first".into(), controller("first-owner", "surface"));
    for _ in 0..4 {
        assert_eq!(
            vault.unlock(&initial.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    drop(initial);
    let next = vault.session("reconnected".into(), controller("first-owner", "surface"));
    assert_eq!(
        vault.unlock(&next.lease(), password()),
        Err(Failure::WrongPassphrase)
    );
    assert_eq!(
        vault.unlock(&next.lease(), password()),
        Err(Failure::Cooldown)
    );
    let independent = vault.session("independent".into(), controller("other-owner", "surface"));
    assert_eq!(
        vault.unlock(&independent.lease(), password()),
        Err(Failure::WrongPassphrase)
    );
}
#[test]
fn bounded_caller_capacity_preserves_existing_controllers_and_penalties() {
    let f = Fixture::new();
    let (vault, repository) = rejecting_vault(&f);
    let owner = vault.session("owner".into(), caller());
    vault.create(&owner.lease(), password(), &[]).unwrap();
    vault.lock(&owner.lease()).unwrap();
    // This published library bound includes the successful owner above.
    for index in 0..nessa_vault::contract::MAXIMUM_COOLDOWN_CALLERS - 1 {
        let session = vault.session(
            format!("lease-{index}"),
            controller("other", &format!("credential-{index}")),
        );
        assert_eq!(
            vault.unlock(&session.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    let first = vault.session(
        "first-reconnected".into(),
        controller("other", "credential-0"),
    );
    for _ in 0..4 {
        assert_eq!(
            vault.unlock(&first.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    let overflow = vault.session("overflow".into(), controller("other", "untracked"));
    assert_eq!(
        vault.unlock(&overflow.lease(), password()),
        Err(Failure::Capacity)
    );
    assert_eq!(
        vault.unlock(&first.lease(), password()),
        Err(Failure::Cooldown)
    );
    repository.reject.store(false, Ordering::SeqCst);
    assert_eq!(
        vault.unlock(&owner.lease(), password()),
        Ok(Status::Unlocked)
    );
    vault.lock(&owner.lease()).unwrap();
    // Quiet unverified slots expire, while the successful controller stays known.
    f.clock.0.store(30, Ordering::SeqCst);
    assert_eq!(
        vault.unlock(&overflow.lease(), password()),
        Ok(Status::Unlocked)
    );
}
#[test]
fn caller_budget_identity_allocation_is_bounded_before_derivation() {
    let f = Fixture::new();
    let (vault, _) = rejecting_vault(&f);
    let session = vault.session(
        "oversized".into(),
        controller(
            &"a".repeat(nessa_vault::contract::MAXIMUM_COOLDOWN_IDENTITY_BYTES + 1),
            "credential",
        ),
    );
    assert_eq!(
        vault.unlock(&session.lease(), password()),
        Err(Failure::Capacity)
    );
    let maximum = nessa_vault::contract::MAXIMUM_COOLDOWN_IDENTITY_BYTES;
    let accepted = vault.session(
        "bounded".into(),
        controller(&"a".repeat(maximum / 2), &"b".repeat(maximum - maximum / 2)),
    );
    assert_eq!(
        vault.unlock(&accepted.lease(), password()),
        Err(Failure::WrongPassphrase)
    );
}

#[test]
fn caller_quiet_window_expires_partial_failures_without_erasing_active_cooldown() {
    let f = Fixture::new();
    let (vault, _) = rejecting_vault(&f);
    let caller = vault.session("quiet-window".into(), caller());
    for _ in 0..4 {
        assert_eq!(
            vault.unlock(&caller.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    f.clock.0.store(30, Ordering::SeqCst);
    for _ in 0..5 {
        assert_eq!(
            vault.unlock(&caller.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    f.clock.0.store(59, Ordering::SeqCst);
    assert_eq!(
        vault.unlock(&caller.lease(), password()),
        Err(Failure::Cooldown)
    );
    f.clock.0.store(60, Ordering::SeqCst);
    assert_eq!(
        vault.unlock(&caller.lease(), password()),
        Err(Failure::WrongPassphrase)
    );
}

#[test]
fn saturated_caller_cooldown_deadline_cannot_wrap_or_appear_elapsed() {
    let f = Fixture::new();
    let (vault, _) = rejecting_vault(&f);
    let session = vault.session("saturated-clock".into(), caller());
    f.clock.0.store(u64::MAX - 10, Ordering::SeqCst);
    for _ in 0..5 {
        assert_eq!(
            vault.unlock(&session.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    f.clock.0.store(u64::MAX, Ordering::SeqCst);
    assert_eq!(
        vault.unlock(&session.lease(), password()),
        Err(Failure::Cooldown)
    );
}

struct RejectFailureEvidence;
impl Audit for RejectFailureEvidence {
    fn record(&self, record: &AuditRecord) -> Result<(), Failure> {
        if record.result == "wrong_passphrase_or_ciphertext" {
            Err(Failure::AuditUnavailable)
        } else {
            Ok(())
        }
    }
}
#[test]
fn failed_guess_evidence_failure_cannot_reset_a_reconnecting_callers_budget() {
    let f = Fixture::new();
    let repository = Arc::new(RejectGuesses {
        inner: f.repository.clone(),
        reject: AtomicBool::new(true),
    });
    let vault = Vault::new(repository, Arc::new(RejectFailureEvidence), f.clock.clone());
    for index in 0..5 {
        let session = vault.session(format!("evidence-failure-{index}"), caller());
        assert_eq!(
            vault.unlock(&session.lease(), password()),
            Err(Failure::AuditUnavailable)
        );
    }
    let reconnected = vault.session("after-evidence-failures".into(), caller());
    assert_eq!(
        vault.unlock(&reconnected.lease(), password()),
        Err(Failure::Cooldown)
    );
    let independent = vault.session(
        "other-evidence-caller".into(),
        controller("other", "credential"),
    );
    assert_eq!(
        vault.unlock(&independent.lease(), password()),
        Err(Failure::AuditUnavailable)
    );
}

#[test]
fn successful_controller_budget_survives_quiet_expiry_and_new_caller_pressure() {
    let f = Fixture::new();
    let (vault, _) = rejecting_vault(&f);
    let owner = vault.session("known-controller".into(), caller());
    vault.create(&owner.lease(), password(), &[]).unwrap();
    vault.lock(&owner.lease()).unwrap();
    f.clock.0.store(30, Ordering::SeqCst);
    for index in 0..nessa_vault::contract::MAXIMUM_COOLDOWN_CALLERS - 1 {
        let caller = vault.session(
            format!("new-{index}"),
            controller("other", &format!("new-credential-{index}")),
        );
        assert_eq!(
            vault.unlock(&caller.lease(), password()),
            Err(Failure::WrongPassphrase)
        );
    }
    let overflow = vault.session(
        "untracked-after-quiet".into(),
        controller("other", "overflow-after-quiet"),
    );
    assert_eq!(
        vault.unlock(&overflow.lease(), password()),
        Err(Failure::Capacity)
    );
    assert_eq!(
        vault.unlock(&owner.lease(), password()),
        Err(Failure::WrongPassphrase)
    );
}
