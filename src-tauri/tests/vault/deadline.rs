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
