use crate::vault::application::{execute, Command, Failure, VaultConnection};
use std::sync::{
    atomic::{AtomicU64, Ordering},
    Arc, Condvar, Mutex,
};
struct Blocked {
    cancelled: Mutex<bool>,
    wake: Condvar,
    interrupted: AtomicU64,
}
impl VaultConnection for Blocked {
    fn next_request(&self) -> u64 {
        7
    }
    fn command(&self, _: Command, _: u64) -> Result<serde_json::Value, Failure> {
        let mut cancelled = self.cancelled.lock().unwrap();
        while !*cancelled {
            cancelled = self.wake.wait(cancelled).unwrap();
        }
        Err(Failure::VaultUnavailable)
    }
    fn interrupt(&self, id: u64) {
        self.interrupted.store(id, Ordering::SeqCst);
        *self.cancelled.lock().unwrap() = true;
        self.wake.notify_all();
    }
    fn close(&self) {
        self.interrupt(7);
    }
}
#[test]
fn a_deadline_interrupts_the_owned_operation_before_reporting_failure() {
    tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap()
        .block_on(async {
            let vault = Arc::new(Blocked {
                cancelled: Mutex::new(false),
                wake: Condvar::new(),
                interrupted: AtomicU64::new(0),
            });
            let request = serde_json::from_value(
                serde_json::json!({"operation":"unlock","passphrase":"disposable test words"}),
            )
            .unwrap();
            let result = execute(vault.clone(), request, std::time::Duration::ZERO).await;
            let interrupted = vault.interrupted.load(Ordering::SeqCst);
            let cancelled = *vault.cancelled.lock().unwrap();
            vault.close(); // The test also releases the worker if its assertion fails.
            assert!(matches!(result, Err(Failure::VaultUnavailable)));
            assert_eq!(interrupted, 7);
            assert!(cancelled);
        });
}

#[test]
fn a_change_deadline_preserves_mutation_uncertainty_and_never_replays() {
    tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
        let vault = Arc::new(Blocked { cancelled: Mutex::new(false), wake: Condvar::new(), interrupted: AtomicU64::new(0) });
        let request: Command = serde_json::from_value(serde_json::json!({"operation":"changePassphrase","passphrase":"disposable replacement phrase"})).unwrap();
        assert_eq!(request.operation(), "changePassphrase");
        let result = execute(vault.clone(), request, std::time::Duration::ZERO).await;
        vault.close();
        assert!(matches!(result, Err(Failure::VaultChangeNotConfirmed)));
        assert_eq!(vault.interrupted.load(Ordering::SeqCst), 7);
        assert_eq!(serde_json::to_value(Failure::VaultChangeNotConfirmed).unwrap(), "vault_change_not_confirmed");
    });
}

struct Panicked(AtomicU64);
impl VaultConnection for Panicked {
    fn next_request(&self) -> u64 {
        11
    }
    fn command(&self, _: Command, _: u64) -> Result<serde_json::Value, Failure> {
        panic!("synthetic native worker failure")
    }
    fn interrupt(&self, id: u64) {
        self.0.store(id, Ordering::SeqCst);
    }
    fn close(&self) {}
}
#[test]
fn a_change_worker_panic_is_not_an_unchanged_or_unavailable_result() {
    tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
        let request = serde_json::from_value(serde_json::json!({"operation":"changePassphrase","passphrase":"disposable replacement phrase"})).unwrap();
        let vault = Arc::new(Panicked(AtomicU64::new(0)));
        assert!(matches!(execute(vault.clone(), request, std::time::Duration::from_secs(2)).await, Err(Failure::VaultChangeNotConfirmed)));
        assert_eq!(vault.0.load(Ordering::SeqCst), 11);
    });
}

struct Busy;
impl VaultConnection for Busy {
    fn next_request(&self) -> u64 {
        13
    }
    fn command(&self, _: Command, _: u64) -> Result<serde_json::Value, Failure> {
        Err(Failure::VaultBusy)
    }
    fn interrupt(&self, _: u64) {}
    fn close(&self) {}
}
#[test]
fn a_known_pre_dispatch_refusal_is_preserved_for_change() {
    tokio::runtime::Builder::new_current_thread().enable_time().build().unwrap().block_on(async {
        let request = serde_json::from_value(serde_json::json!({"operation":"changePassphrase","passphrase":"disposable replacement phrase"})).unwrap();
        assert!(matches!(execute(Arc::new(Busy), request, std::time::Duration::from_secs(2)).await, Err(Failure::VaultBusy)));
    });
}
