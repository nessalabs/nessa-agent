use nessa_local_storage::PrivateTempFile;
use nessa_vault::{
    application::{
        Audit, AuditRecord, Caller, Clock, Entropy, ObservationClock, Repository, Vault,
    },
    domain::{ChangePublication, EntryName, Failure, Passphrase, Secret, Status, VaultKey},
    infrastructure::{
        EncryptedFileVault, FileAudit, FilePublication, NativeFilePublication, OsEntropy,
    },
};
use std::{
    io::{Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc, Arc, Mutex,
    },
};
const OLD: &str = "original disposable vault verification words";
const NEW: &str = "replacement disposable vault verification words";
fn phrase(value: &str) -> Passphrase {
    Passphrase::new(value.into()).unwrap()
}
fn entry(name: &str) -> EntryName {
    EntryName::new(name).unwrap()
}
fn caller() -> Caller {
    Caller {
        principal: "synthetic-owner".into(),
        credential: "synthetic-desktop".into(),
    }
}
struct Time;
impl Clock for Time {
    fn seconds(&self) -> u64 {
        0
    }
}
impl ObservationClock for Time {
    fn unix_millis(&self) -> Result<u64, Failure> {
        Ok(1)
    }
}
#[derive(Default)]
struct Evidence {
    mode: AtomicUsize,
    stored: Mutex<Vec<AuditRecord>>,
}
impl Audit for Evidence {
    fn record(&self, record: &AuditRecord) -> Result<(), Failure> {
        let fail = match self.mode.load(Ordering::SeqCst) {
            1 => record.action == "passphrase_change" && record.result == "requested",
            2 => record.action == "lock",
            3 => record.action == "passphrase_change" && record.result != "requested",
            4 => {
                record.action == "lock"
                    || (record.action == "passphrase_change" && record.result != "requested")
            }
            _ => false,
        };
        if fail {
            Err(Failure::AuditUnavailable)
        } else {
            self.stored.lock().unwrap().push(record.clone());
            Ok(())
        }
    }
}
struct Fixture {
    root: tempfile::TempDir,
    repo: Arc<EncryptedFileVault>,
    audit: Arc<Evidence>,
    vault: Arc<Vault>,
}
impl Fixture {
    fn new() -> Self {
        let root = tempfile::tempdir().unwrap();
        let repo = Arc::new(EncryptedFileVault::new(
            root.path().join("vault"),
            Arc::new(OsEntropy),
        ));
        let audit = Arc::new(Evidence::default());
        let vault = Vault::new(repo.clone(), audit.clone(), Arc::new(Time));
        Self {
            root,
            repo,
            audit,
            vault,
        }
    }
    fn file(&self) -> PathBuf {
        self.root.path().join("vault/entries.enc")
    }
}
#[test]
fn confirmed_change_seals_then_new_phrase_unlocks_exact_entries_and_fences_old_consumers() {
    let f = Fixture::new();
    let owner = f.vault.session("owner".into(), caller());
    let initial = vec![
        (
            entry("first"),
            Secret::new(b"synthetic first value".to_vec()),
        ),
        (entry("second"), Secret::new(vec![0, 255, 12, 0, 6])),
    ];
    f.vault
        .create(&owner.lease(), phrase(OLD), &initial)
        .unwrap();
    let generation = f.vault.unlocked_generation().unwrap();
    let previous = std::fs::read(f.file()).unwrap();
    let receipt = f
        .vault
        .change_passphrase(&owner.lease(), phrase(NEW))
        .unwrap();
    assert_eq!(receipt.publication(), ChangePublication::Committed);
    assert!(!receipt.audit_failed());
    assert_eq!(f.vault.status(), Ok(Status::Locked));
    assert_eq!(f.vault.unlocked_generation(), None);
    assert!(matches!(
        f.vault.read_in(generation, &entry("first")),
        Err(Failure::Locked)
    ));
    let current = std::fs::read(f.file()).unwrap();
    assert_eq!(&current[..12], b"NESSAVAULT01");
    assert_ne!(&current[12..28], &previous[12..28]);
    assert_ne!(&current[28..52], &previous[28..52]);
    assert!(matches!(
        f.repo.unlock(&phrase(OLD)),
        Err(Failure::WrongPassphrase)
    ));
    f.vault.unlock(&owner.lease(), phrase(NEW)).unwrap();
    for (name, expected) in initial {
        assert_eq!(
            f.vault.read(&name).unwrap().unwrap().as_slice(),
            expected.as_slice()
        );
    }
    assert_eq!(
        f.vault.write_in(generation, &entry("first"), b"stale"),
        Err(Failure::Locked)
    );
    let evidence = f.audit.stored.lock().unwrap();
    let change: Vec<_> = evidence
        .iter()
        .filter(|row| row.action == "passphrase_change")
        .collect();
    assert_eq!(change.len(), 2);
    assert_eq!(
        (
            change[0].before,
            change[0].after,
            change[0].cause,
            change[0].result
        ),
        (
            Status::Unlocked,
            Status::Unlocked,
            "explicit_request",
            "requested"
        )
    );
    assert_eq!(
        (
            change[1].before,
            change[1].after,
            change[1].cause,
            change[1].result
        ),
        (
            Status::Locked,
            Status::Locked,
            "encrypted_receipt",
            "committed"
        )
    );
    assert_eq!(change[1].caller.principal, "synthetic-owner");
    assert_eq!(change[1].lease, "owner");
    let debug = format!("{receipt:?}");
    assert!(
        !debug.contains(OLD) && !debug.contains(NEW) && !debug.contains("synthetic first value")
    );
}
#[test]
fn preadmission_refusals_preserve_the_live_owner_and_ciphertext() {
    let f = Fixture::new();
    let owner = f.vault.session("owner".into(), caller());
    assert_eq!(
        f.vault.change_passphrase(&owner.lease(), phrase(NEW)),
        Err(Failure::Locked)
    );
    f.vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
    let bytes = std::fs::read(f.file()).unwrap();
    let generation = f.vault.unlocked_generation();
    let other = f.vault.session("other".into(), caller());
    assert_eq!(
        f.vault.change_passphrase(&other.lease(), phrase(NEW)),
        Err(Failure::SessionOwned)
    );
    assert_eq!(
        f.vault
            .change_passphrase(&owner.lease(), phrase("aaaaaaaaaaaaaaa")),
        Err(Failure::PassphrasePolicy)
    );
    f.audit.mode.store(1, Ordering::SeqCst);
    assert_eq!(
        f.vault.change_passphrase(&owner.lease(), phrase(NEW)),
        Err(Failure::AuditUnavailable)
    );
    assert_eq!(f.vault.unlocked_generation(), generation);
    assert_eq!(std::fs::read(f.file()).unwrap(), bytes);
    f.audit.mode.store(0, Ordering::SeqCst);
    drop(owner);
    let mut closed = f.vault.session("closed".into(), caller());
    closed.close().unwrap();
    assert_eq!(
        f.vault.change_passphrase(&closed.lease(), phrase(NEW)),
        Err(Failure::SessionClosed)
    );
}
#[test]
fn sealing_audit_failure_does_not_replace_but_attempts_attributed_terminal_evidence() {
    for mode in [2, 4] {
        let f = Fixture::new();
        let owner = f.vault.session("owner".into(), caller());
        f.vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
        let bytes = std::fs::read(f.file()).unwrap();
        f.audit.mode.store(mode, Ordering::SeqCst);
        let receipt = f
            .vault
            .change_passphrase(&owner.lease(), phrase(NEW))
            .unwrap();
        assert_eq!(
            receipt.publication(),
            ChangePublication::Unchanged(Failure::AuditUnavailable)
        );
        assert!(receipt.audit_failed());
        assert_eq!(f.vault.status(), Ok(Status::Locked));
        assert_eq!(std::fs::read(f.file()).unwrap(), bytes);
        assert!(f.repo.unlock(&phrase(OLD)).is_ok());
        let evidence = f.audit.stored.lock().unwrap();
        let terminal = evidence
            .iter()
            .find(|r| r.action == "passphrase_change" && r.result == "unchanged");
        if mode == 2 {
            let terminal = terminal.expect("selective sink accepts terminal outcome");
            assert_eq!(terminal.cause, "audit_refused");
            assert_eq!(
                (terminal.before, terminal.after),
                (Status::Locked, Status::Locked)
            );
            assert_eq!(terminal.caller.principal, "synthetic-owner");
            assert_eq!(terminal.lease, "owner");
        } else {
            assert!(terminal.is_none());
        }
    }
}
struct ChosenOutcome {
    inner: Arc<EncryptedFileVault>,
    outcome: ChangePublication,
}
impl Repository for ChosenOutcome {
    fn configured(&self) -> Result<bool, Failure> {
        self.inner.configured()
    }
    fn create(&self, p: &Passphrase, e: &[(EntryName, Secret)]) -> Result<VaultKey, Failure> {
        self.inner.create(p, e)
    }
    fn unlock(&self, p: &Passphrase) -> Result<VaultKey, Failure> {
        self.inner.unlock(p)
    }
    fn read(&self, k: &VaultKey, n: &EntryName) -> Result<Option<Secret>, Failure> {
        self.inner.read(k, n)
    }
    fn write(&self, k: &VaultKey, n: &EntryName, v: &[u8]) -> Result<(), Failure> {
        self.inner.write(k, n, v)
    }
    fn delete(&self, k: &VaultKey, n: &EntryName) -> Result<(), Failure> {
        self.inner.delete(k, n)
    }
    fn change_passphrase(&self, k: &VaultKey, p: &Passphrase) -> ChangePublication {
        if self.outcome != ChangePublication::Unchanged(Failure::Unavailable) {
            assert_eq!(
                self.inner.change_passphrase(k, p),
                ChangePublication::Committed
            );
        }
        self.outcome
    }
}
#[test]
fn terminal_audit_failure_preserves_each_independent_publication_fact() {
    for outcome in [
        ChangePublication::Unchanged(Failure::Unavailable),
        ChangePublication::Committed,
        ChangePublication::Uncertain(Failure::Unavailable),
    ] {
        let f = Fixture::new();
        let vault = Vault::new(
            Arc::new(ChosenOutcome {
                inner: f.repo.clone(),
                outcome,
            }),
            f.audit.clone(),
            Arc::new(Time),
        );
        let owner = vault.session("owner".into(), caller());
        vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
        f.audit.mode.store(3, Ordering::SeqCst);
        let receipt = vault
            .change_passphrase(&owner.lease(), phrase(NEW))
            .unwrap();
        assert_eq!(receipt.publication(), outcome);
        assert!(receipt.audit_failed());
        assert_eq!(vault.status(), Ok(Status::Locked));
        let expected = if matches!(outcome, ChangePublication::Unchanged(_)) {
            OLD
        } else {
            NEW
        };
        assert!(f.repo.unlock(&phrase(expected)).is_ok());
    }
}
struct GateAudit {
    inner: Arc<Evidence>,
    started: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl Audit for GateAudit {
    fn record(&self, r: &AuditRecord) -> Result<(), Failure> {
        if r.action == "passphrase_change" && r.result == "requested" {
            self.started.send(()).unwrap();
            self.resume.lock().unwrap().recv().unwrap();
        }
        self.inner.record(r)
    }
}
#[test]
fn connection_close_during_intent_prevents_admission_after_audit_resumes() {
    let f = Fixture::new();
    let (started, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let vault = Vault::new(
        f.repo.clone(),
        Arc::new(GateAudit {
            inner: f.audit.clone(),
            started,
            resume: Mutex::new(released),
        }),
        Arc::new(Time),
    );
    let mut owner = vault.session("owner".into(), caller());
    vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
    let bytes = std::fs::read(f.file()).unwrap();
    let lease = owner.lease();
    let worker = {
        let vault = vault.clone();
        let lease = lease.clone();
        std::thread::spawn(move || vault.change_passphrase(&lease, phrase(NEW)))
    };
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let closing = std::thread::spawn(move || owner.close());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while lease.is_active() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    resume.send(()).unwrap();
    assert_eq!(worker.join().unwrap(), Err(Failure::SessionClosed));
    closing.join().unwrap().unwrap();
    assert_eq!(std::fs::read(f.file()).unwrap(), bytes);
    assert_eq!(vault.status(), Ok(Status::Locked));
}
#[test]
fn explicit_lock_during_intent_prevents_admission_after_audit_resumes() {
    let f = Fixture::new();
    let (started, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let vault = Vault::new(
        f.repo.clone(),
        Arc::new(GateAudit {
            inner: f.audit.clone(),
            started,
            resume: Mutex::new(released),
        }),
        Arc::new(Time),
    );
    let owner = vault.session("owner".into(), caller());
    vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
    let generation = vault.unlocked_generation().unwrap();
    let bytes = std::fs::read(f.file()).unwrap();
    let worker = {
        let vault = vault.clone();
        let lease = owner.lease();
        std::thread::spawn(move || vault.change_passphrase(&lease, phrase(NEW)))
    };
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let (answer, answered) = mpsc::sync_channel(1);
    let locking = {
        let vault = vault.clone();
        let lease = owner.lease();
        std::thread::spawn(move || answer.send(vault.lock(&lease)).unwrap())
    };
    let before_resume = answered.recv_timeout(std::time::Duration::from_secs(5));
    // Always release the audit and join both threads, including in the revert probe.
    resume.send(()).unwrap();
    let change = worker.join().unwrap();
    locking.join().unwrap();
    assert_eq!(before_resume, Ok(Ok(Status::Locked)));
    assert_eq!(change, Err(Failure::Locked));
    assert_eq!(vault.status(), Ok(Status::Locked));
    assert_eq!(std::fs::read(f.file()).unwrap(), bytes);
    assert!(f.repo.unlock(&phrase(OLD)).is_ok());
    assert!(matches!(
        f.repo.unlock(&phrase(NEW)),
        Err(Failure::WrongPassphrase)
    ));
    assert!(matches!(
        vault.read_in(generation, &entry("absent")),
        Err(Failure::Locked)
    ));
    let evidence = f.audit.stored.lock().unwrap();
    assert_eq!(evidence.iter().filter(|r| r.action == "lock").count(), 1);
    assert!(!evidence
        .iter()
        .any(|r| r.action == "passphrase_change" && r.result != "requested"));
}
struct GateRepository {
    inner: Arc<EncryptedFileVault>,
    started: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl Repository for GateRepository {
    fn configured(&self) -> Result<bool, Failure> {
        self.inner.configured()
    }
    fn create(&self, p: &Passphrase, e: &[(EntryName, Secret)]) -> Result<VaultKey, Failure> {
        self.inner.create(p, e)
    }
    fn unlock(&self, p: &Passphrase) -> Result<VaultKey, Failure> {
        self.inner.unlock(p)
    }
    fn read(&self, k: &VaultKey, n: &EntryName) -> Result<Option<Secret>, Failure> {
        self.inner.read(k, n)
    }
    fn write(&self, k: &VaultKey, n: &EntryName, v: &[u8]) -> Result<(), Failure> {
        self.inner.write(k, n, v)
    }
    fn delete(&self, k: &VaultKey, n: &EntryName) -> Result<(), Failure> {
        self.inner.delete(k, n)
    }
    fn change_passphrase(&self, k: &VaultKey, p: &Passphrase) -> ChangePublication {
        self.started.send(()).unwrap();
        self.resume.lock().unwrap().recv().unwrap();
        self.inner.change_passphrase(k, p)
    }
}
#[test]
fn admitted_change_can_settle_after_close_and_does_not_publish_a_key() {
    let f = Fixture::new();
    let (started, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let vault = Vault::new(
        Arc::new(GateRepository {
            inner: f.repo.clone(),
            started,
            resume: Mutex::new(released),
        }),
        f.audit.clone(),
        Arc::new(Time),
    );
    let mut owner = vault.session("owner".into(), caller());
    vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
    let lease = owner.lease();
    let worker = {
        let vault = vault.clone();
        let lease = lease.clone();
        std::thread::spawn(move || vault.change_passphrase(&lease, phrase(NEW)))
    };
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    assert_eq!(vault.unlock(&lease, phrase(OLD)), Err(Failure::Busy));
    let closing = std::thread::spawn(move || owner.close());
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while lease.is_active() {
        assert!(std::time::Instant::now() < deadline);
        std::thread::yield_now();
    }
    resume.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap().unwrap().publication(),
        ChangePublication::Committed
    );
    closing.join().unwrap().unwrap();
    assert_eq!(vault.status(), Ok(Status::Locked));
    assert!(f.repo.unlock(&phrase(NEW)).is_ok());
}
#[derive(Clone, Copy)]
enum Fault {
    Prepare,
    CorruptTemporary,
    BeforeReplace,
    AfterReplace,
    Sync,
}
struct Publication {
    fault: Fault,
    enabled: AtomicBool,
    replaces: AtomicUsize,
}
impl Publication {
    fn fault(&self, fault: Fault) -> bool {
        self.enabled.load(Ordering::SeqCst)
            && std::mem::discriminant(&self.fault) == std::mem::discriminant(&fault)
    }
}
impl FilePublication for Publication {
    fn prepare(&self, d: &Path, b: &[u8]) -> Result<PrivateTempFile, Failure> {
        if self.fault(Fault::Prepare) {
            return Err(Failure::Unavailable);
        }
        let mut temporary = NativeFilePublication.prepare(d, b)?;
        if self.fault(Fault::CorruptTemporary) {
            temporary.as_file_mut().seek(SeekFrom::Start(0)).unwrap();
            temporary.as_file_mut().write_all(b"BROKEN").unwrap();
        }
        Ok(temporary)
    }
    fn replace(&self, t: PrivateTempFile, d: &Path) -> Result<(), Failure> {
        self.replaces.fetch_add(1, Ordering::SeqCst);
        if self.fault(Fault::BeforeReplace) {
            return Err(Failure::Unavailable);
        }
        NativeFilePublication.replace(t, d)?;
        if self.fault(Fault::AfterReplace) {
            return Err(Failure::Unavailable);
        }
        Ok(())
    }
    fn publish_new(&self, t: PrivateTempFile, d: &Path) -> Result<(), Failure> {
        NativeFilePublication.publish_new(t, d)
    }
    fn sync(&self, d: &Path) -> Result<(), Failure> {
        if self.fault(Fault::Sync) {
            Err(Failure::Unavailable)
        } else {
            NativeFilePublication.sync(d)
        }
    }
}
#[test]
fn real_file_faults_preserve_prepublication_bytes_and_classify_postreplace_uncertainty() {
    for fault in [
        Fault::Prepare,
        Fault::CorruptTemporary,
        Fault::BeforeReplace,
        Fault::AfterReplace,
        Fault::Sync,
    ] {
        let root = tempfile::tempdir().unwrap();
        let publication = Arc::new(Publication {
            fault,
            enabled: AtomicBool::new(false),
            replaces: AtomicUsize::new(0),
        });
        let repo = EncryptedFileVault::with_publication(
            root.path().join("vault"),
            Arc::new(OsEntropy),
            publication.clone(),
        );
        let key = repo
            .create(
                &phrase(OLD),
                &[(
                    entry("synthetic"),
                    Secret::new(b"retained synthetic value".to_vec()),
                )],
            )
            .unwrap();
        let file = root.path().join("vault/entries.enc");
        let before = std::fs::read(&file).unwrap();
        publication.enabled.store(true, Ordering::SeqCst);
        let outcome = repo.change_passphrase(&key, &phrase(NEW));
        let expected = if matches!(fault, Fault::Prepare | Fault::CorruptTemporary) {
            ChangePublication::Unchanged(Failure::Unavailable)
        } else {
            ChangePublication::Uncertain(Failure::Unavailable)
        };
        assert_eq!(outcome, expected);
        publication.enabled.store(false, Ordering::SeqCst);
        let recovery = if matches!(fault, Fault::AfterReplace | Fault::Sync) {
            NEW
        } else {
            OLD
        };
        let recovered = repo.unlock(&phrase(recovery)).unwrap();
        assert_eq!(
            repo.read(&recovered, &entry("synthetic"))
                .unwrap()
                .unwrap()
                .as_slice(),
            b"retained synthetic value"
        );
        if matches!(fault, Fault::Prepare | Fault::CorruptTemporary) {
            assert_eq!(publication.replaces.load(Ordering::SeqCst), 0);
            assert_eq!(std::fs::read(&file).unwrap(), before);
        }
    }
}
struct FailedEntropy;
impl Entropy for FailedEntropy {
    fn fill(&self, _: &mut [u8]) -> Result<(), Failure> {
        Err(Failure::Unavailable)
    }
}
#[test]
fn new_entropy_refusal_keeps_the_original_file_and_weak_change_never_dispatches_publication() {
    let f = Fixture::new();
    let key = f.repo.create(&phrase(OLD), &[]).unwrap();
    let bytes = std::fs::read(f.file()).unwrap();
    let repo = EncryptedFileVault::new(f.root.path().join("vault"), Arc::new(FailedEntropy));
    assert_eq!(
        repo.change_passphrase(&key, &phrase(NEW)),
        ChangePublication::Unchanged(Failure::Unavailable)
    );
    assert_eq!(
        repo.change_passphrase(&key, &phrase("aaaaaaaaaaaaaaa")),
        ChangePublication::Unchanged(Failure::PassphrasePolicy)
    );
    assert_eq!(std::fs::read(f.file()).unwrap(), bytes);
}
#[test]
fn original_format_fixtures_can_change_without_a_format_transition() {
    for (fixture, old_phrase, has_entry) in [
        (
            include_bytes!("fixtures/format-one.enc").as_slice(),
            "public vault format verification words",
            true,
        ),
        (
            include_bytes!("fixtures/legacy-weak.enc").as_slice(),
            "aaaaaaaaaaaa",
            false,
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("vault");
        nessa_local_storage::create_directory(&directory).unwrap();
        nessa_local_storage::open(
            &directory.join("entries.enc"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap()
        .write_all(fixture)
        .unwrap();
        let repo = EncryptedFileVault::new(directory, Arc::new(OsEntropy));
        let key = repo.unlock(&phrase(old_phrase)).unwrap();
        assert_eq!(
            repo.change_passphrase(&key, &phrase(NEW)),
            ChangePublication::Committed
        );
        let key = repo.unlock(&phrase(NEW)).unwrap();
        let value = repo.read(&key, &entry("example-secret")).unwrap();
        if has_entry {
            assert_eq!(value.unwrap().as_slice(), b"public fixture value");
        } else {
            assert!(value.is_none());
        }
    }
}

#[test]
fn file_audit_accepts_only_the_change_state_and_outcome_contract() {
    for (before, after, cause, result) in [
        (
            Status::Unlocked,
            Status::Unlocked,
            "explicit_request",
            "requested",
        ),
        (
            Status::Locked,
            Status::Locked,
            "encrypted_receipt",
            "committed",
        ),
        (
            Status::Locked,
            Status::Locked,
            "storage_refused",
            "unchanged",
        ),
        (Status::Locked, Status::Locked, "audit_refused", "unchanged"),
        (
            Status::Locked,
            Status::Locked,
            "storage_unavailable",
            "not_confirmed",
        ),
    ] {
        let root = tempfile::tempdir().unwrap();
        let audit = FileAudit::new(root.path().join("audit"), Arc::new(Time));
        let event = AuditRecord {
            target: None,
            operation: 0,
            lease: "owner".into(),
            caller: caller(),
            action: "passphrase_change",
            before,
            after,
            cause,
            result,
        };
        audit.record(&event).unwrap();
        for invalid in [
            AuditRecord {
                operation: 1,
                before: Status::Unconfigured,
                ..event.clone()
            },
            AuditRecord {
                operation: 1,
                target: Some(entry("unexpected")),
                ..event.clone()
            },
            AuditRecord {
                operation: 1,
                result: "published",
                ..event.clone()
            },
        ] {
            assert_eq!(audit.record(&invalid), Err(Failure::AuditUnavailable));
        }
    }
}

struct PanicRepository {
    inner: Arc<EncryptedFileVault>,
    commit_first: bool,
}
impl Repository for PanicRepository {
    fn configured(&self) -> Result<bool, Failure> {
        self.inner.configured()
    }
    fn create(&self, p: &Passphrase, e: &[(EntryName, Secret)]) -> Result<VaultKey, Failure> {
        self.inner.create(p, e)
    }
    fn unlock(&self, p: &Passphrase) -> Result<VaultKey, Failure> {
        self.inner.unlock(p)
    }
    fn read(&self, k: &VaultKey, n: &EntryName) -> Result<Option<Secret>, Failure> {
        self.inner.read(k, n)
    }
    fn write(&self, k: &VaultKey, n: &EntryName, v: &[u8]) -> Result<(), Failure> {
        self.inner.write(k, n, v)
    }
    fn delete(&self, k: &VaultKey, n: &EntryName) -> Result<(), Failure> {
        self.inner.delete(k, n)
    }
    fn change_passphrase(&self, k: &VaultKey, p: &Passphrase) -> ChangePublication {
        if self.commit_first {
            assert_eq!(
                self.inner.change_passphrase(k, p),
                ChangePublication::Committed
            );
        }
        panic!("synthetic publication interruption")
    }
}
#[test]
fn repository_panic_before_or_after_replacement_settles_unknown_and_remains_recoverable() {
    for commit_first in [false, true] {
        let f = Fixture::new();
        let vault = Vault::new(
            Arc::new(PanicRepository {
                inner: f.repo.clone(),
                commit_first,
            }),
            f.audit.clone(),
            Arc::new(Time),
        );
        let owner = vault.session("owner".into(), caller());
        vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
        let receipt = vault
            .change_passphrase(&owner.lease(), phrase(NEW))
            .unwrap();
        assert_eq!(
            receipt.publication(),
            ChangePublication::Uncertain(Failure::Unavailable)
        );
        assert!(!receipt.audit_failed());
        assert_eq!(vault.status(), Ok(Status::Locked));
        let expected = if commit_first { NEW } else { OLD };
        assert_eq!(
            vault.unlock(&owner.lease(), phrase(expected)),
            Ok(Status::Unlocked)
        );
        let evidence = f.audit.stored.lock().unwrap();
        assert!(evidence
            .iter()
            .any(|r| r.action == "passphrase_change" && r.result == "not_confirmed"));
    }
}
struct PanicAudit {
    inner: Arc<Evidence>,
    phase: usize,
}
impl Audit for PanicAudit {
    fn record(&self, r: &AuditRecord) -> Result<(), Failure> {
        let phase = if r.action == "passphrase_change" && r.result == "requested" {
            1
        } else if r.action == "lock" {
            2
        } else if r.action == "passphrase_change" {
            3
        } else {
            0
        };
        if phase == self.phase {
            panic!("synthetic evidence interruption");
        }
        self.inner.record(r)
    }
}
#[test]
fn audit_panics_are_typed_without_poisoning_state_or_losing_commit_truth() {
    for phase in [1, 2, 3] {
        let f = Fixture::new();
        let vault = Vault::new(
            f.repo.clone(),
            Arc::new(PanicAudit {
                inner: f.audit.clone(),
                phase,
            }),
            Arc::new(Time),
        );
        let owner = vault.session("owner".into(), caller());
        vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
        let result = vault.change_passphrase(&owner.lease(), phrase(NEW));
        if phase == 1 {
            assert_eq!(result, Err(Failure::AuditUnavailable));
            assert_eq!(vault.status(), Ok(Status::Unlocked));
        } else {
            let receipt = result.unwrap();
            assert!(receipt.audit_failed());
            assert_eq!(
                receipt.publication(),
                if phase == 2 {
                    ChangePublication::Unchanged(Failure::AuditUnavailable)
                } else {
                    ChangePublication::Committed
                }
            );
            assert_eq!(vault.status(), Ok(Status::Locked));
        }
        let expected = if phase == 3 { NEW } else { OLD };
        assert!(f.repo.unlock(&phrase(expected)).is_ok());
    }
}
struct TerminalAudit {
    inner: Arc<Evidence>,
    started: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl Audit for TerminalAudit {
    fn record(&self, r: &AuditRecord) -> Result<(), Failure> {
        if r.action == "passphrase_change" && r.result != "requested" {
            self.started.send(()).unwrap();
            self.resume.lock().unwrap().recv().unwrap();
        }
        self.inner.record(r)
    }
}
#[test]
fn global_derivation_permit_remains_owned_until_terminal_audit_settles() {
    let f = Fixture::new();
    let (started, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let vault = Vault::new(
        f.repo.clone(),
        Arc::new(TerminalAudit {
            inner: f.audit.clone(),
            started,
            resume: Mutex::new(released),
        }),
        Arc::new(Time),
    );
    let owner = vault.session("owner".into(), caller());
    vault.create(&owner.lease(), phrase(OLD), &[]).unwrap();
    let lease = owner.lease();
    let worker = {
        let vault = vault.clone();
        let lease = lease.clone();
        std::thread::spawn(move || vault.change_passphrase(&lease, phrase(NEW)))
    };
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let (attempted, attempting) = mpsc::sync_channel(1);
    let (answer, answered) = mpsc::sync_channel(1);
    let contender = {
        let vault = vault.clone();
        let lease = lease.clone();
        std::thread::spawn(move || {
            attempted.send(()).unwrap();
            answer.send(vault.unlock(&lease, phrase(NEW))).unwrap();
        })
    };
    attempting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let before_settlement = answered.recv_timeout(std::time::Duration::from_secs(5));
    // Release and join even when probing a removed derivation permit.
    resume.send(()).unwrap();
    assert_eq!(
        worker.join().unwrap().unwrap().publication(),
        ChangePublication::Committed
    );
    contender.join().unwrap();
    assert_eq!(before_settlement, Ok(Err(Failure::Busy)));
    assert_eq!(vault.unlock(&lease, phrase(NEW)), Ok(Status::Unlocked));
}
struct GatePublication {
    started: mpsc::SyncSender<()>,
    resume: Mutex<mpsc::Receiver<()>>,
}
impl FilePublication for GatePublication {
    fn prepare(&self, d: &Path, b: &[u8]) -> Result<PrivateTempFile, Failure> {
        NativeFilePublication.prepare(d, b)
    }
    fn replace(&self, t: PrivateTempFile, d: &Path) -> Result<(), Failure> {
        self.started.send(()).unwrap();
        self.resume.lock().unwrap().recv().unwrap();
        NativeFilePublication.replace(t, d)
    }
    fn publish_new(&self, t: PrivateTempFile, d: &Path) -> Result<(), Failure> {
        NativeFilePublication.publish_new(t, d)
    }
    fn sync(&self, d: &Path) -> Result<(), Failure> {
        NativeFilePublication.sync(d)
    }
}
#[test]
fn shared_file_lock_prevents_an_old_key_writer_from_overwriting_changed_ciphertext() {
    let root = tempfile::tempdir().unwrap();
    let directory = root.path().join("vault");
    let baseline = EncryptedFileVault::new(directory.clone(), Arc::new(OsEntropy));
    let first = baseline
        .create(
            &phrase(OLD),
            &[(
                entry("retained"),
                Secret::new(b"original synthetic".to_vec()),
            )],
        )
        .unwrap();
    let second = baseline.unlock(&phrase(OLD)).unwrap();
    let (started, waiting) = mpsc::sync_channel(1);
    let (resume, released) = mpsc::sync_channel(1);
    let changing = EncryptedFileVault::with_publication(
        directory.clone(),
        Arc::new(OsEntropy),
        Arc::new(GatePublication {
            started,
            resume: Mutex::new(released),
        }),
    );
    let change = std::thread::spawn(move || changing.change_passphrase(&first, &phrase(NEW)));
    waiting
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    let (writing, begun) = mpsc::sync_channel(1);
    let competitor = std::thread::spawn(move || {
        writing.send(()).unwrap();
        baseline.write(&second, &entry("retained"), b"stale overwrite")
    });
    begun
        .recv_timeout(std::time::Duration::from_secs(10))
        .unwrap();
    resume.send(()).unwrap();
    assert_eq!(change.join().unwrap(), ChangePublication::Committed);
    assert_eq!(competitor.join().unwrap(), Err(Failure::Unavailable));
    let repo = EncryptedFileVault::new(directory, Arc::new(OsEntropy));
    let key = repo.unlock(&phrase(NEW)).unwrap();
    assert_eq!(
        repo.read(&key, &entry("retained"))
            .unwrap()
            .unwrap()
            .as_slice(),
        b"original synthetic"
    );
}

#[test]
fn weak_direct_repository_change_does_not_create_a_vault_directory() {
    let f = Fixture::new();
    let key = f.repo.create(&phrase(OLD), &[]).unwrap();
    let path = f.root.path().join("absent-vault");
    let repo = EncryptedFileVault::new(path.clone(), Arc::new(OsEntropy));
    assert_eq!(
        repo.change_passphrase(&key, &phrase("aaaaaaaaaaaaaaa")),
        ChangePublication::Unchanged(Failure::PassphrasePolicy)
    );
    assert!(!path.exists());
}
