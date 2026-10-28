//! Allocation is tested through the application ports with controllable provider
//! admission. No real account or secret is used in these ordering fixtures.
use crate::workspaces::{application::*, domain::*};
use async_trait::async_trait;
use nessa_protocol::conversation::domain::ConversationId;
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Arc, Mutex,
};
use tokio::sync::Semaphore;
fn connection() -> BoatConnection {
    BoatConnection::new(
        "fixture-token".into(),
        "nessa".into(),
        "fixture-image".into(),
    )
    .unwrap()
}
#[derive(Default)]
struct Store {
    records: Mutex<std::collections::BTreeMap<String, Assignment>>,
    fail_receipt: std::sync::atomic::AtomicBool,
    fail_refusal: AtomicBool,
    panic_receipt: AtomicBool,
}
#[async_trait]
impl Assignments for Store {
    async fn read(&self, thread: &str) -> Result<Option<Assignment>, Failure> {
        Ok(self.records.lock().unwrap().get(thread).cloned())
    }
    async fn save(&self, value: Assignment) -> Result<(), Failure> {
        assert!(
            !(matches!(value, Assignment::Assigned { .. })
                && self.panic_receipt.load(Ordering::SeqCst)),
            "injected receipt publication panic"
        );
        if matches!(value, Assignment::Assigned { .. }) && self.fail_receipt.load(Ordering::SeqCst)
        {
            return Err(Failure::Storage);
        }
        if matches!(value, Assignment::Refused { .. }) && self.fail_refusal.load(Ordering::SeqCst) {
            return Err(Failure::Storage);
        }
        self.records
            .lock()
            .unwrap()
            .insert(value.thread().into(), value);
        Ok(())
    }
}
struct Config;
#[async_trait]
impl Settings for Config {
    fn generation(&self) -> Result<u64, Failure> {
        Ok(1)
    }
    async fn read(&self) -> Result<BoatConnection, Failure> {
        Ok(connection())
    }
    async fn save(&self, _: u64, _: BoatConnection) -> Result<(), Failure> {
        Ok(())
    }
    async fn clear(&self) -> Result<(), Failure> {
        Ok(())
    }
}
struct Runtime;
#[async_trait]
impl WorkspaceRuntime for Runtime {
    async fn serve(&self) -> Result<WorkspaceStream, Failure> {
        Err(Failure::Provider)
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum RecoveryStep {
    Audit(RecoveryEvent),
    Remove(String),
}
#[derive(Default)]
struct Audit {
    events: Mutex<Vec<RecoveryEvent>>,
    steps: Arc<Mutex<Vec<RecoveryStep>>>,
    fail_receipt: AtomicBool,
    fail_cleanup: AtomicBool,
    panic_receipt: AtomicBool,
}
impl RecoveryAudit for Audit {
    fn record(&self, event: &RecoveryEvent) -> Result<(), Failure> {
        self.steps
            .lock()
            .unwrap()
            .push(RecoveryStep::Audit(event.clone()));
        assert!(
            !(matches!(event, RecoveryEvent::ReceiptSaveFailed { .. })
                && self.panic_receipt.load(Ordering::SeqCst)),
            "injected receipt audit panic"
        );
        let fail = match event {
            RecoveryEvent::ReceiptSaveFailed { .. } | RecoveryEvent::AdmissionRefused { .. } => {
                &self.fail_receipt
            }
            RecoveryEvent::RollbackObserved { .. } => &self.fail_cleanup,
        };
        if fail.load(Ordering::SeqCst) {
            return Err(Failure::Storage);
        }
        self.events.lock().unwrap().push(event.clone());
        Ok(())
    }
}
fn recovery_events(thread: &ConversationId, confirmed: bool) -> Vec<RecoveryEvent> {
    let thread = thread.to_string();
    let vm = format!("vm-{thread}");
    vec![
        RecoveryEvent::ReceiptSaveFailed {
            thread: thread.clone(),
            vm: vm.clone(),
            disposition: RefusalDisposition::RollbackNewVm,
        },
        RecoveryEvent::RollbackObserved {
            thread,
            vm,
            confirmed,
        },
    ]
}
struct FakeProvider {
    refusal_on_create: Mutex<Option<AdmissionRejection>>,
    created_disposition: Mutex<RefusalDisposition>,
    refusal_on_connect: Mutex<Option<AdmissionRejection>>,
    creates: AtomicUsize,
    connects: Mutex<Vec<String>>,
    deletes: AtomicUsize,
    entered: Semaphore,
    proceed: Semaphore,
    confirm_remove: AtomicBool,
    panic_remove: AtomicBool,
    recovery_steps: Arc<Mutex<Vec<RecoveryStep>>>,
}
impl FakeProvider {
    fn new(recovery_steps: Arc<Mutex<Vec<RecoveryStep>>>) -> Self {
        Self {
            refusal_on_create: Mutex::new(None),
            created_disposition: Mutex::new(RefusalDisposition::RollbackNewVm),
            refusal_on_connect: Mutex::new(None),
            confirm_remove: AtomicBool::new(false),
            panic_remove: AtomicBool::new(false),
            recovery_steps,
            creates: AtomicUsize::new(0),
            connects: Mutex::new(vec![]),
            deletes: AtomicUsize::new(0),
            entered: Semaphore::new(0),
            proceed: Semaphore::new(0),
        }
    }
}
#[async_trait]
impl Provider for FakeProvider {
    async fn validate(&self, _: &BoatConnection) -> Result<(), Failure> {
        Ok(())
    }
    async fn create(&self, _: &BoatConnection, thread: &str) -> Result<WorkspaceReceipt, Failure> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        self.proceed.acquire().await.unwrap().forget();
        Ok(WorkspaceReceipt {
            vm: format!("vm-{thread}"),
            admission: admission(),
            refusal: self.refusal_on_create.lock().unwrap().clone(),
            disposition: *self.created_disposition.lock().unwrap(),
        })
    }
    async fn connect(
        &self,
        _: &BoatConnection,
        _: &str,
        vm: &str,
        _: &EnvironmentIdentity,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        if let Some(rejection) = self.refusal_on_connect.lock().unwrap().clone() {
            return Err(Failure::AdmissionRejected(rejection));
        }
        self.connects.lock().unwrap().push(vm.into());
        Ok(Arc::new(Runtime))
    }
    async fn remove(&self, _: &BoatConnection, vm: &str) -> Result<(), Failure> {
        self.recovery_steps
            .lock()
            .unwrap()
            .push(RecoveryStep::Remove(vm.into()));
        self.deletes.fetch_add(1, Ordering::SeqCst);
        assert!(
            !self.panic_remove.load(Ordering::SeqCst),
            "injected cleanup panic"
        );
        if self.confirm_remove.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err(Failure::CleanupUncertain)
        }
    }
}
fn id(value: u128) -> ConversationId {
    ConversationId::new(&uuid::Uuid::from_u128(value).to_string()).unwrap()
}
fn fixture() -> (Arc<Workspaces>, Arc<Store>, Arc<FakeProvider>) {
    let (service, store, provider, _) = fixture_with_audit();
    (service, store, provider)
}
fn fixture_with_audit() -> (Arc<Workspaces>, Arc<Store>, Arc<FakeProvider>, Arc<Audit>) {
    let store = Arc::new(Store::default());
    let audit = Arc::new(Audit::default());
    let provider = Arc::new(FakeProvider::new(audit.steps.clone()));
    (
        Arc::new(Workspaces::new(
            Arc::new(Config),
            provider.clone(),
            store.clone(),
            audit.clone(),
        )),
        store,
        provider,
        audit,
    )
}
#[tokio::test]
async fn simultaneous_open_and_reopen_create_one_vm_for_the_thread() {
    let (service, _, provider) = fixture();
    let one = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(1), true).await })
    };
    provider.entered.acquire().await.unwrap().forget();
    let two = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(1), true).await })
    };
    provider.proceed.add_permits(1);
    assert!(one.await.unwrap().is_ok());
    assert!(two.await.unwrap().is_ok());
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
    assert!(service.open(id(1), true).await.is_ok());
    assert!(provider
        .connects
        .lock()
        .unwrap()
        .iter()
        .all(|vm| vm == &format!("vm-{}", id(1))));
}
#[tokio::test]
async fn two_threads_receive_distinct_vm_identities() {
    let (service, _, provider) = fixture();
    provider.proceed.add_permits(2);
    assert!(service.open(id(1), true).await.is_ok());
    assert!(service.open(id(2), true).await.is_ok());
    let vms = provider.connects.lock().unwrap();
    assert_ne!(vms[0], vms[1]);
    assert_eq!(provider.creates.load(Ordering::SeqCst), 2);
}
#[tokio::test]
async fn an_uncertain_intent_and_accounting_never_create_a_replacement() {
    let (service, store, provider) = fixture();
    store.records.lock().unwrap().insert(
        id(1).to_string(),
        Assignment::Provisioning {
            thread: id(1).to_string(),
        },
    );
    assert!(matches!(
        service.open(id(1), true).await,
        Err(Failure::ProvisioningUncertain)
    ));
    assert!(matches!(
        service.open(id(2), false).await,
        Err(Failure::Missing)
    ));
    assert_eq!(provider.creates.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn lost_waiter_does_not_abandon_the_creation_receipt() {
    let (service, store, provider) = fixture();
    let task = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(1), true).await })
    };
    provider.entered.acquire().await.unwrap().forget();
    task.abort();
    provider.proceed.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        loop {
            if matches!(
                store.records.lock().unwrap().get(&id(1).to_string()),
                Some(Assignment::Assigned { .. })
            ) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(service.open(id(1), true).await.is_ok());
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failed_receipt_storage_attempts_cleanup_and_preserves_uncertainty() {
    let (service, store, provider, audit) = fixture_with_audit();
    store.fail_receipt.store(true, Ordering::SeqCst);
    provider.proceed.add_permits(1);
    assert_eq!(
        service.open(id(1), true).await.err(),
        Some(Failure::ReceiptRecovery {
            vm: format!("vm-{}", id(1)),
            receipt_audit_failed: false,
            cleanup: CleanupOutcome::Unconfirmed,
            cleanup_audit_failed: false,
        })
    );
    let events = recovery_events(&id(1), false);
    assert_eq!(*audit.events.lock().unwrap(), events);
    assert_eq!(
        *audit.steps.lock().unwrap(),
        vec![
            RecoveryStep::Audit(events[0].clone()),
            RecoveryStep::Remove(format!("vm-{}", id(1))),
            RecoveryStep::Audit(events[1].clone()),
        ]
    );
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    assert!(matches!(
        service.open(id(1), true).await,
        Err(Failure::ProvisioningUncertain)
    ));
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn receipt_recovery_keeps_target_and_attempts_cleanup_across_all_audit_failures() {
    for receipt_audit_failed in [false, true] {
        for cleanup_confirmed in [false, true] {
            for cleanup_audit_failed in [false, true] {
                let (service, store, provider, audit) = fixture_with_audit();
                store.fail_receipt.store(true, Ordering::SeqCst);
                audit
                    .fail_receipt
                    .store(receipt_audit_failed, Ordering::SeqCst);
                audit
                    .fail_cleanup
                    .store(cleanup_audit_failed, Ordering::SeqCst);
                provider
                    .confirm_remove
                    .store(cleanup_confirmed, Ordering::SeqCst);
                provider.proceed.add_permits(1);
                assert_eq!(
                    service.open(id(2), true).await.err(),
                    Some(Failure::ReceiptRecovery {
                        vm: format!("vm-{}", id(2)),
                        receipt_audit_failed,
                        cleanup: if cleanup_confirmed {
                            CleanupOutcome::Confirmed
                        } else {
                            CleanupOutcome::Unconfirmed
                        },
                        cleanup_audit_failed,
                    })
                );
                let events = recovery_events(&id(2), cleanup_confirmed);
                assert_eq!(
                    *audit.steps.lock().unwrap(),
                    vec![
                        RecoveryStep::Audit(events[0].clone()),
                        RecoveryStep::Remove(format!("vm-{}", id(2))),
                        RecoveryStep::Audit(events[1].clone()),
                    ],
                    "audit refusal must not skip rollback or its observation"
                );
                let persisted = events
                    .into_iter()
                    .enumerate()
                    .filter_map(|(index, event)| {
                        let failed = if index == 0 {
                            receipt_audit_failed
                        } else {
                            cleanup_audit_failed
                        };
                        (!failed).then_some(event)
                    })
                    .collect::<Vec<_>>();
                assert_eq!(*audit.events.lock().unwrap(), persisted);
                assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
                assert!(provider.connects.lock().unwrap().is_empty());
                assert_eq!(
                    store.records.lock().unwrap().get(&id(2).to_string()),
                    Some(&Assignment::Provisioning {
                        thread: id(2).to_string()
                    })
                );
                assert_eq!(
                    service.open(id(2), true).await.err(),
                    Some(Failure::ProvisioningUncertain)
                );
                assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
            }
        }
    }
}

#[tokio::test]
async fn lost_waiter_retains_failed_receipt_target_and_rollback_evidence() {
    let (service, store, provider, audit) = fixture_with_audit();
    store.fail_receipt.store(true, Ordering::SeqCst);
    let task = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(3), true).await })
    };
    provider.entered.acquire().await.unwrap().forget();
    task.abort();
    match task.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("allocation waiter must be cancelled before the provider receipt"),
    }
    provider.proceed.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(2), async {
        while audit.events.lock().unwrap().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let events = recovery_events(&id(3), false);
    assert_eq!(*audit.events.lock().unwrap(), events);
    assert_eq!(
        *audit.steps.lock().unwrap(),
        vec![
            RecoveryStep::Audit(events[0].clone()),
            RecoveryStep::Remove(format!("vm-{}", id(3))),
            RecoveryStep::Audit(events[1].clone()),
        ]
    );
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    assert!(provider.connects.lock().unwrap().is_empty());
    assert_eq!(
        service.open(id(3), true).await.err(),
        Some(Failure::ProvisioningUncertain)
    );
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}

#[test]
fn connection_cache_refuses_capacity_without_evicting_borrowed_environments() {
    use crate::conversation::{
        application::{ConversationError, ThreadEnvironments, MAX_RETAINED_ENVIRONMENTS},
        infrastructure::{ssh_environment::DurableEnvironmentAudit, ThreadWorkspaces},
    };
    struct Clock;
    impl nessa_auth::application::ports::Clock for Clock {
        fn unix_milliseconds(&self) -> u64 {
            42
        }
    }
    let directory = tempfile::tempdir().unwrap();
    let (service, _, provider) = fixture();
    let factory = ThreadWorkspaces {
        service,
        audit: DurableEnvironmentAudit::new(directory.path().join("audit"), Arc::new(Clock))
            .unwrap(),
        environments: Default::default(),
    };
    let identity =
        |n: usize| ConversationId::new(&uuid::Uuid::from_u128(n as u128 + 1).to_string()).unwrap();
    let mut retained = (0..MAX_RETAINED_ENVIRONMENTS)
        .map(|n| factory.environment(&identity(n)).unwrap())
        .collect::<Vec<_>>();
    assert!(Arc::ptr_eq(
        &retained[0],
        &factory.environment(&identity(0)).unwrap()
    ));
    assert!(matches!(
        factory.environment(&identity(MAX_RETAINED_ENVIRONMENTS)),
        Err(ConversationError::Unavailable)
    ));
    let released = Arc::downgrade(&retained.pop().unwrap());
    assert!(factory
        .environment(&identity(MAX_RETAINED_ENVIRONMENTS))
        .is_ok());
    assert!(released.upgrade().is_none());
    assert!(Arc::ptr_eq(
        &retained[0],
        &factory.environment(&identity(0)).unwrap()
    ));
    assert_eq!(
        provider.creates.load(Ordering::SeqCst),
        0,
        "cache admission must precede provisioning"
    );
}

#[tokio::test]
async fn recovery_port_panics_keep_cleanup_and_target_evidence_owned() {
    let (service, store, provider, audit) = fixture_with_audit();
    store.fail_receipt.store(true, Ordering::SeqCst);
    audit.panic_receipt.store(true, Ordering::SeqCst);
    provider.panic_remove.store(true, Ordering::SeqCst);
    provider.proceed.add_permits(1);
    assert_eq!(
        service.open(id(3), true).await.err(),
        Some(Failure::ReceiptRecovery {
            vm: format!("vm-{}", id(3)),
            receipt_audit_failed: true,
            cleanup: CleanupOutcome::Unconfirmed,
            cleanup_audit_failed: false,
        })
    );
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(
        *audit.events.lock().unwrap(),
        vec![recovery_events(&id(3), false)[1].clone()]
    );
    assert_eq!(
        service.open(id(3), true).await.err(),
        Some(Failure::ProvisioningUncertain)
    );
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn receipt_publication_panic_keeps_external_target_in_the_recovery_owner() {
    let (service, store, provider, audit) = fixture_with_audit();
    store.panic_receipt.store(true, Ordering::SeqCst);
    provider.proceed.add_permits(1);
    assert_eq!(
        service.open(id(4), true).await.err(),
        Some(Failure::ReceiptRecovery {
            vm: format!("vm-{}", id(4)),
            receipt_audit_failed: false,
            cleanup: CleanupOutcome::Unconfirmed,
            cleanup_audit_failed: false,
        })
    );
    assert_eq!(
        *audit.events.lock().unwrap(),
        recovery_events(&id(4), false)
    );
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.open(id(4), true).await.err(),
        Some(Failure::ProvisioningUncertain)
    );
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}

pub(super) fn admission() -> EnvironmentIdentity {
    EnvironmentIdentity::new(
        "11111111-1111-4111-8111-111111111111".into(),
        "nessa".into(),
        "22222222-2222-4222-8222-222222222222".into(),
        2,
    )
    .unwrap()
}

fn rejection() -> AdmissionRejection {
    AdmissionRejection::new(
        AdmissionRefusal::WrongVersion,
        Some(ObservedEnvironment {
            name: Some("nessa".into()),
            version: Some(3),
        }),
    )
}
#[tokio::test]
async fn refused_creations_keep_the_target_and_cleanup_outcome_across_storage_and_audit_failure() {
    for receipt_failed in [false, true] {
        for audit_failed in [false, true] {
            for cleanup_unconfirmed in [false, true] {
                for cleanup_audit_failed in [false, true] {
                    let (service, store, provider, audit) = fixture_with_audit();
                    *provider.refusal_on_create.lock().unwrap() = Some(rejection());
                    store.fail_refusal.store(receipt_failed, Ordering::SeqCst);
                    audit.fail_receipt.store(audit_failed, Ordering::SeqCst);
                    audit
                        .fail_cleanup
                        .store(cleanup_audit_failed, Ordering::SeqCst);
                    provider
                        .confirm_remove
                        .store(!cleanup_unconfirmed, Ordering::SeqCst);
                    provider.proceed.add_permits(1);
                    let vm = format!("vm-{}", id(4));
                    assert_eq!(
                        service.open(id(4), true).await.err(),
                        Some(Failure::AdmissionRecovery {
                            vm: vm.clone(),
                            rejection: rejection(),
                            receipt_failed,
                            audit_failed,
                            cleanup: if cleanup_unconfirmed {
                                CleanupOutcome::Unconfirmed
                            } else {
                                CleanupOutcome::Confirmed
                            },
                            cleanup_audit_failed
                        })
                    );
                    let admission_event = RecoveryEvent::AdmissionRefused {
                        thread: id(4).to_string(),
                        vm: vm.clone(),
                        admission: admission(),
                        rejection: rejection(),
                        disposition: RefusalDisposition::RollbackNewVm,
                    };
                    assert_eq!(
                        *audit.steps.lock().unwrap(),
                        vec![
                            RecoveryStep::Audit(admission_event),
                            RecoveryStep::Remove(vm.clone()),
                            RecoveryStep::Audit(RecoveryEvent::RollbackObserved {
                                thread: id(4).to_string(),
                                vm,
                                confirmed: !cleanup_unconfirmed
                            })
                        ]
                    );
                    assert!(provider.connects.lock().unwrap().is_empty());
                    let retry = service.open(id(4), true).await.err();
                    assert_eq!(
                        retry,
                        Some(if receipt_failed {
                            Failure::ProvisioningUncertain
                        } else {
                            Failure::RefusedWorkspace {
                                vm: format!("vm-{}", id(4)),
                                rejection: rejection(),
                                disposition: RefusalDisposition::RollbackNewVm,
                            }
                        })
                    );
                    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
                }
            }
        }
    }
}
#[tokio::test]
async fn a_reconnect_refusal_is_durable_and_never_reactivates_after_provider_repair() {
    let (service, store, provider, _) = fixture_with_audit();
    provider.proceed.add_permits(1);
    assert!(service.open(id(5), true).await.is_ok());
    *provider.refusal_on_connect.lock().unwrap() = Some(rejection());
    assert!(matches!(
        service.open(id(5), false).await,
        Err(Failure::AdmissionRecovery { .. })
    ));
    assert!(matches!(
        store.read(&id(5).to_string()).await.unwrap(),
        Some(Assignment::Refused { .. })
    ));
    *provider.refusal_on_connect.lock().unwrap() = None;
    assert_eq!(
        service.open(id(5), true).await.err(),
        Some(Failure::RefusedWorkspace {
            vm: format!("vm-{}", id(5)),
            rejection: rejection(),
            disposition: RefusalDisposition::RetainTarget
        })
    );
    assert_eq!(
        provider.deletes.load(Ordering::SeqCst),
        0,
        "a reconnect refusal must preserve existing workspace data"
    );
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(provider.connects.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn a_dropped_waiter_keeps_the_durable_refusal_and_cleanup_evidence() {
    let (service, store, provider, audit) = fixture_with_audit();
    *provider.refusal_on_create.lock().unwrap() = Some(rejection());
    let task = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(6), true).await })
    };
    provider.entered.acquire().await.unwrap().forget();
    task.abort();
    match task.await {
        Err(error) => assert!(error.is_cancelled()),
        Ok(_) => panic!("waiter must be cancelled"),
    }
    provider.proceed.add_permits(1);
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        while audit.events.lock().unwrap().len() != 2 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(matches!(
        store.read(&id(6).to_string()).await.unwrap(),
        Some(Assignment::Refused { .. })
    ));
    assert_eq!(
        service.open(id(6), true).await.err(),
        Some(Failure::RefusedWorkspace {
            vm: format!("vm-{}", id(6)),
            rejection: rejection(),
            disposition: RefusalDisposition::RollbackNewVm
        })
    );
    assert!(provider.connects.lock().unwrap().is_empty());
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn replayed_or_unverified_receipt_targets_are_retained_when_publication_fails() {
    let (service, store, provider, audit) = fixture_with_audit();
    *provider.created_disposition.lock().unwrap() = RefusalDisposition::RetainTarget;
    store.fail_receipt.store(true, Ordering::SeqCst);
    provider.proceed.add_permits(1);
    assert_eq!(
        service.open(id(7), true).await.err(),
        Some(Failure::ReceiptRecovery {
            vm: format!("vm-{}", id(7)),
            receipt_audit_failed: false,
            cleanup: CleanupOutcome::Retained,
            cleanup_audit_failed: false
        })
    );
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 0);
    assert_eq!(
        *audit.events.lock().unwrap(),
        vec![RecoveryEvent::ReceiptSaveFailed {
            thread: id(7).to_string(),
            vm: format!("vm-{}", id(7)),
            disposition: RefusalDisposition::RetainTarget
        }]
    );
    assert_eq!(
        service.open(id(7), true).await.err(),
        Some(Failure::ProvisioningUncertain)
    );
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn an_initial_connection_refusal_cannot_delete_a_replayed_workspace() {
    let (service, _, provider, _) = fixture_with_audit();
    *provider.created_disposition.lock().unwrap() = RefusalDisposition::RetainTarget;
    *provider.refusal_on_connect.lock().unwrap() = Some(rejection());
    provider.proceed.add_permits(1);
    assert!(matches!(
        service.open(id(8), true).await,
        Err(Failure::AdmissionRecovery {
            cleanup: CleanupOutcome::Retained,
            ..
        })
    ));
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 0);
    assert_eq!(
        service.open(id(8), true).await.err(),
        Some(Failure::RefusedWorkspace {
            vm: format!("vm-{}", id(8)),
            rejection: rejection(),
            disposition: RefusalDisposition::RetainTarget
        })
    );
}
#[tokio::test]
async fn failed_refusal_storage_or_audit_never_deletes_existing_workspace_data() {
    for receipt_failed in [false, true] {
        for audit_failed in [false, true] {
            let (service, store, provider, audit) = fixture_with_audit();
            provider.proceed.add_permits(1);
            assert!(service.open(id(9), true).await.is_ok());
            store.fail_refusal.store(receipt_failed, Ordering::SeqCst);
            audit.fail_receipt.store(audit_failed, Ordering::SeqCst);
            *provider.refusal_on_connect.lock().unwrap() = Some(rejection());
            assert_eq!(
                service.open(id(9), false).await.err(),
                Some(Failure::AdmissionRecovery {
                    vm: format!("vm-{}", id(9)),
                    rejection: rejection(),
                    receipt_failed,
                    audit_failed,
                    cleanup: CleanupOutcome::Retained,
                    cleanup_audit_failed: false
                })
            );
            assert_eq!(provider.deletes.load(Ordering::SeqCst), 0);
            assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
            assert_eq!(provider.connects.lock().unwrap().len(), 1);
        }
    }
}
