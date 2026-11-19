use super::*;
use crate::vault::application::{Command, Failure, Operation, VaultConnection};
use serde_json::json;
use std::sync::{Arc, Mutex};

/// A vault child that accepts one passphrase and records what it was sent.
struct Vault {
    passphrase: Mutex<String>,
    sent: Mutex<Vec<(String, Option<String>)>>,
}
impl Vault {
    fn new(passphrase: &str) -> Arc<Self> {
        Arc::new(Self {
            passphrase: Mutex::new(passphrase.into()),
            sent: Mutex::new(Vec::new()),
        })
    }
}
impl VaultConnection for Vault {
    fn next_request(&self) -> u64 {
        1
    }
    fn command(&self, request: Command, _: u64) -> Result<serde_json::Value, Failure> {
        let given = request.passphrase.as_deref().map(|value| value.to_string());
        self.sent
            .lock()
            .unwrap()
            .push((request.operation().into(), given.clone()));
        Ok(match request.operation {
            Operation::Unlock if given.as_deref() == Some(&*self.passphrase.lock().unwrap()) => {
                json!({"ok": true, "status": {"state": "unlocked"}})
            }
            Operation::Unlock => json!({"ok": false, "code": "vault_wrong_passphrase"}),
            Operation::ChangePassphrase => {
                *self.passphrase.lock().unwrap() = given.unwrap();
                json!({"ok": true, "status": {"state": "locked"}})
            }
            _ => json!({"ok": true, "status": {"state": "locked"}}),
        })
    }
    fn interrupt(&self, _: u64) {}
    fn close(&self) {}
}

/// A fingerprint reader that answers as told, and a store holding one copy.
struct Store {
    available: bool,
    prompt: Result<(), BiometricFailure>,
    saved: Mutex<Option<String>>,
    prompts: Mutex<u32>,
}
impl Store {
    fn new(
        available: bool,
        prompt: Result<(), BiometricFailure>,
        saved: Option<&str>,
    ) -> Arc<Self> {
        Arc::new(Self {
            available,
            prompt,
            saved: Mutex::new(saved.map(Into::into)),
            prompts: Mutex::new(0),
        })
    }
    fn saved_copy(&self) -> Option<String> {
        self.saved.lock().unwrap().clone()
    }
}
impl BiometricStore for Store {
    fn available(&self) -> bool {
        self.available
    }
    fn saved(&self) -> bool {
        self.saved.lock().unwrap().is_some()
    }
    fn save(&self, passphrase: &str) -> Result<(), BiometricFailure> {
        *self.saved.lock().unwrap() = Some(passphrase.into());
        Ok(())
    }
    fn release(&self) -> Result<Zeroizing<String>, BiometricFailure> {
        *self.prompts.lock().unwrap() += 1;
        self.prompt?;
        self.saved
            .lock()
            .unwrap()
            .clone()
            .map(Zeroizing::new)
            .ok_or(BiometricFailure::Missing)
    }
    fn forget(&self) {
        *self.saved.lock().unwrap() = None;
    }
}

fn run<T>(future: impl std::future::Future<Output = T>) -> T {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(future)
}
fn kept(store: &Arc<Store>) -> Arc<KeptPassphrase> {
    Arc::new(KeptPassphrase::new(store.clone()))
}
fn command(operation: &str) -> BiometricCommand {
    serde_json::from_value(json!({ "operation": operation })).unwrap()
}
const WORDS: &str = "disposable test words for the vault";
const DEADLINE: Duration = Duration::from_secs(5);

#[test]
fn a_fingerprint_hands_the_saved_passphrase_to_the_vault_as_an_unlock() {
    let vault = Vault::new(WORDS);
    let store = Store::new(true, Ok(()), Some(WORDS));
    let reply = run(execute_biometric(
        vault.clone(),
        kept(&store),
        command("unlock"),
        DEADLINE,
    ));
    assert_eq!(
        reply.unwrap(),
        json!({"ok": true, "status": {"state": "unlocked"}})
    );
    assert_eq!(
        *vault.sent.lock().unwrap(),
        vec![("unlock".to_string(), Some(WORDS.to_string()))]
    );
    assert_eq!(store.saved_copy().as_deref(), Some(WORDS));
}

#[test]
fn a_dismissed_prompt_sends_nothing_and_keeps_the_copy() {
    let vault = Vault::new(WORDS);
    let store = Store::new(true, Err(BiometricFailure::Cancelled), Some(WORDS));
    let reply = run(execute_biometric(
        vault.clone(),
        kept(&store),
        command("unlock"),
        DEADLINE,
    ));
    assert_eq!(
        reply.unwrap_err(),
        BiometricRefusal::Biometric(BiometricFailure::Cancelled)
    );
    assert!(vault.sent.lock().unwrap().is_empty());
    assert_eq!(store.saved_copy().as_deref(), Some(WORDS));
    assert_eq!(
        serde_json::to_value(BiometricRefusal::Biometric(BiometricFailure::Cancelled)).unwrap(),
        "biometric_cancelled"
    );
}

#[test]
fn a_device_without_a_reader_never_prompts() {
    let vault = Vault::new(WORDS);
    let store = Store::new(false, Ok(()), Some(WORDS));
    let reply = run(execute_biometric(
        vault.clone(),
        kept(&store),
        command("unlock"),
        DEADLINE,
    ));
    assert_eq!(
        reply.unwrap_err(),
        BiometricRefusal::Biometric(BiometricFailure::Unavailable)
    );
    assert_eq!(*store.prompts.lock().unwrap(), 0);
    assert_eq!(
        run(execute_biometric(
            vault,
            kept(&store),
            command("status"),
            DEADLINE
        ))
        .unwrap(),
        json!({"available": false, "saved": false})
    );
}

#[test]
fn a_copy_the_vault_refuses_is_dropped() {
    let vault = Vault::new("the passphrase changed elsewhere");
    let store = Store::new(true, Ok(()), Some(WORDS));
    let reply = run(execute_biometric(
        vault,
        kept(&store),
        command("unlock"),
        DEADLINE,
    ));
    assert_eq!(
        reply.unwrap(),
        json!({"ok": false, "code": "vault_wrong_passphrase"})
    );
    assert_eq!(store.saved_copy(), None);
}

#[test]
fn forget_drops_the_copy_and_reports_it() {
    let vault = Vault::new(WORDS);
    let store = Store::new(true, Ok(()), Some(WORDS));
    assert_eq!(
        run(execute_biometric(
            vault.clone(),
            kept(&store),
            command("status"),
            DEADLINE
        ))
        .unwrap(),
        json!({"available": true, "saved": true})
    );
    assert_eq!(
        run(execute_biometric(
            vault,
            kept(&store),
            command("forget"),
            DEADLINE
        ))
        .unwrap(),
        json!({"available": true, "saved": false})
    );
    assert_eq!(store.saved_copy(), None);
}

#[test]
fn only_an_accepted_unlock_that_asked_to_remember_saves_the_passphrase() {
    let unlocked = json!({"ok": true, "status": {"state": "unlocked"}});
    let wrong = json!({"ok": false, "code": "vault_wrong_passphrase"});
    let store = Store::new(true, Ok(()), None);
    let words = || Some(Zeroizing::new(WORDS.to_string()));
    run(settle_biometric(
        kept(&store),
        &Operation::Unlock,
        false,
        words(),
        &unlocked,
    ));
    assert_eq!(store.saved_copy(), None, "not asked to remember");
    run(settle_biometric(
        kept(&store),
        &Operation::Unlock,
        true,
        words(),
        &wrong,
    ));
    assert_eq!(store.saved_copy(), None, "refused by the vault");
    run(settle_biometric(
        kept(&store),
        &Operation::Unlock,
        true,
        words(),
        &unlocked,
    ));
    assert_eq!(store.saved_copy().as_deref(), Some(WORDS));

    let unavailable = Store::new(false, Ok(()), None);
    run(settle_biometric(
        kept(&unavailable),
        &Operation::Setup,
        true,
        words(),
        &unlocked,
    ));
    assert_eq!(unavailable.saved_copy(), None, "no reader to guard it");
}

#[test]
fn a_change_replaces_a_saved_copy_and_never_creates_one() {
    let changed = json!({"ok": true, "status": {"state": "locked"}});
    let unchanged = json!({"ok": false, "code": "vault_change_unchanged"});
    let next = || Some(Zeroizing::new("the next disposable passphrase".to_string()));

    let empty = Store::new(true, Ok(()), None);
    run(settle_biometric(
        kept(&empty),
        &Operation::ChangePassphrase,
        false,
        next(),
        &changed,
    ));
    assert_eq!(empty.saved_copy(), None);

    let held = Store::new(true, Ok(()), Some(WORDS));
    run(settle_biometric(
        kept(&held),
        &Operation::ChangePassphrase,
        false,
        next(),
        &unchanged,
    ));
    assert_eq!(held.saved_copy().as_deref(), Some(WORDS));
    run(settle_biometric(
        kept(&held),
        &Operation::ChangePassphrase,
        false,
        next(),
        &json!({"ok": false, "code": "vault_changed_audit_unavailable"}),
    ));
    assert_eq!(
        held.saved_copy().as_deref(),
        Some("the next disposable passphrase")
    );
}

#[test]
fn remember_is_accepted_on_the_wire_and_defaults_off() {
    let plain: Command =
        serde_json::from_value(json!({"operation": "unlock", "passphrase": WORDS})).unwrap();
    assert!(!plain.remember);
    let asked: Command = serde_json::from_value(
        json!({"operation": "unlock", "passphrase": WORDS, "remember": true}),
    )
    .unwrap();
    assert!(asked.remember);
}

/// A store whose "is one kept?" check stops until the test lets it go, so a
/// forget can be started between that check and the save that follows it.
struct HeldCheck {
    inner: Arc<Store>,
    checking: std::sync::mpsc::SyncSender<()>,
    go: Mutex<std::sync::mpsc::Receiver<()>>,
    held: std::sync::atomic::AtomicBool,
}
impl BiometricStore for HeldCheck {
    fn available(&self) -> bool {
        self.inner.available()
    }
    fn saved(&self) -> bool {
        let kept = self.inner.saved();
        // Only the first check (the change's) is held; later reads pass.
        if !self.held.swap(true, std::sync::atomic::Ordering::SeqCst) {
            let _ = self.checking.send(());
            let _ = self.go.lock().unwrap().recv();
        }
        kept
    }
    fn save(&self, passphrase: &str) -> Result<(), BiometricFailure> {
        self.inner.save(passphrase)
    }
    fn release(&self) -> Result<Zeroizing<String>, BiometricFailure> {
        self.inner.release()
    }
    fn forget(&self) {
        self.inner.forget()
    }
}

#[test]
fn turning_off_during_a_change_is_never_undone_by_its_replacement() {
    let inner = Store::new(true, Ok(()), Some(WORDS));
    let (checking, checked) = std::sync::mpsc::sync_channel(1);
    let (go, held) = std::sync::mpsc::sync_channel(1);
    let kept = Arc::new(KeptPassphrase::new(Arc::new(HeldCheck {
        inner: inner.clone(),
        checking,
        go: Mutex::new(held),
        held: std::sync::atomic::AtomicBool::new(false),
    })));
    let changing = {
        let kept = kept.clone();
        std::thread::spawn(move || {
            run(settle_biometric(
                kept,
                &Operation::ChangePassphrase,
                false,
                Some(Zeroizing::new("the next disposable passphrase".to_string())),
                &json!({"ok": true, "status": {"state": "locked"}}),
            ))
        })
    };
    // The change has seen a copy kept and has not saved yet.
    checked.recv().unwrap();
    let (forgot, forgotten) = std::sync::mpsc::channel();
    let forgetting = {
        let kept = kept.clone();
        std::thread::spawn(move || {
            run(execute_biometric(
                Vault::new(WORDS),
                kept,
                command("forget"),
                DEADLINE,
            ))
            .unwrap();
            forgot.send(()).unwrap();
        })
    };
    assert!(
        forgotten.recv_timeout(Duration::from_millis(100)).is_err(),
        "turning off finished inside the change's check-and-save"
    );
    go.send(()).unwrap();
    changing.join().unwrap();
    forgetting.join().unwrap();
    assert_eq!(inner.saved_copy(), None, "the person's turn-off was undone");
}
