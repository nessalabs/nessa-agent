//! Allocation is tested through the application ports with controllable provider
//! admission. No real account or secret is used in these ordering fixtures.
use crate::conversation::domain::{ChannelId, VmLifetime, VmLifetimeScope, VmLifetimeSettings};
use crate::mcp_servers::infrastructure::{ConfigCheck, ConfigJson, OsConfigFiles};
use crate::workspaces::{
    application::*,
    domain::*,
    infrastructure::{ConfigRuntimeSettings, RuntimeSettingsAudit},
};
use async_trait::async_trait;
use nessa_auth::{
    application::ports::Clock,
    domain::{OrganizationId, PrincipalId},
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::infrastructure::clock::RuntimeClock;
use std::os::unix::fs::PermissionsExt;
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
            RecoveryEvent::ReceiptSaveFailed { .. } => &self.fail_receipt,
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
        },
        RecoveryEvent::RollbackObserved {
            thread,
            vm,
            confirmed,
        },
    ]
}
struct FakeProvider {
    creates: AtomicUsize,
    created_lifetimes: Mutex<Vec<VmLifetime>>,
    connected_lifetimes: Mutex<Vec<VmLifetime>>,
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
            confirm_remove: AtomicBool::new(false),
            panic_remove: AtomicBool::new(false),
            recovery_steps,
            creates: AtomicUsize::new(0),
            created_lifetimes: Mutex::new(vec![]),
            connected_lifetimes: Mutex::new(vec![]),
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
    async fn create(&self, connection: &BoatConnection, thread: &str) -> Result<String, Failure> {
        self.created_lifetimes
            .lock()
            .unwrap()
            .push(connection.auto_stop);
        self.creates.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        self.proceed.acquire().await.unwrap().forget();
        Ok(format!("vm-{thread}"))
    }
    async fn connect(
        &self,
        connection: &BoatConnection,
        _: &str,
        vm: &str,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        self.connected_lifetimes
            .lock()
            .unwrap()
            .push(connection.auto_stop);
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
            Arc::new(Config),
        )),
        store,
        provider,
        audit,
    )
}
struct Time;
impl Clock for Time {
    fn unix_milliseconds(&self) -> u64 {
        42
    }
}
#[tokio::test]
async fn config_default_and_channel_override_reach_creation_and_existing_vm_connections() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let settings = Arc::new(ConfigRuntimeSettings {
        config: Arc::new(ConfigJson::new(
            Arc::new(OsConfigFiles::new(directory.path().join("config.json"))),
            ConfigCheck {
                limit: 4096,
                parses: Box::new(|bytes| {
                    serde_json::from_slice::<serde_json::Value>(bytes).is_ok()
                }),
            },
            Arc::new(RuntimeClock::new()),
        )),
        audit: Arc::new(
            RuntimeSettingsAudit::new(directory.path().join("audit"), Arc::new(Time)).unwrap(),
        ),
        next_id: Arc::new(uuid::Uuid::new_v4),
    });
    let caller = || RuntimeCaller {
        organization: OrganizationId::new("fixture-org").unwrap(),
        principal: PrincipalId::new("fixture-owner").unwrap(),
        credential: "fixture-credential".into(),
        operation: "fixture-request".into(),
    };
    let finite = VmLifetime::from_seconds(Some(14_400)).unwrap();
    let channel = ChannelId::from_random([1; 10]);
    settings
        .save(VmLifetimeScope::Global, Some(finite), caller())
        .await
        .unwrap();
    settings
        .save(
            VmLifetimeScope::Channel(channel.clone()),
            Some(VmLifetime::Unlimited),
            caller(),
        )
        .await
        .unwrap();
    let (_, store, provider, audit) = fixture_with_audit();
    let service = Workspaces::new(
        Arc::new(Config),
        provider.clone(),
        store,
        audit,
        settings.clone(),
    );
    provider.proceed.add_permits(2);
    service.open(id(1), true, None).await.unwrap();
    service
        .open(id(2), true, Some(channel.clone()))
        .await
        .unwrap();
    assert_eq!(
        *provider.created_lifetimes.lock().unwrap(),
        vec![finite, VmLifetime::Unlimited]
    );
    settings
        .save(
            VmLifetimeScope::Global,
            Some(VmLifetime::Unlimited),
            caller(),
        )
        .await
        .unwrap();
    settings
        .save(
            VmLifetimeScope::Channel(channel.clone()),
            Some(finite),
            caller(),
        )
        .await
        .unwrap();
    service.open(id(1), true, None).await.unwrap();
    service.open(id(2), true, Some(channel)).await.unwrap();
    assert_eq!(provider.creates.load(Ordering::SeqCst), 2);
    assert_eq!(
        *provider.connected_lifetimes.lock().unwrap(),
        vec![finite, VmLifetime::Unlimited, VmLifetime::Unlimited, finite]
    );
}
#[tokio::test]
async fn simultaneous_open_and_reopen_create_one_vm_for_the_thread() {
    let (service, _, provider) = fixture();
    let one = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(1), true, None).await })
    };
    provider.entered.acquire().await.unwrap().forget();
    let two = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(1), true, None).await })
    };
    provider.proceed.add_permits(1);
    assert!(one.await.unwrap().is_ok());
    assert!(two.await.unwrap().is_ok());
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
    assert!(service.open(id(1), true, None).await.is_ok());
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
    assert!(service.open(id(1), true, None).await.is_ok());
    assert!(service.open(id(2), true, None).await.is_ok());
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
        service.open(id(1), true, None).await,
        Err(Failure::ProvisioningUncertain)
    ));
    assert!(matches!(
        service.open(id(2), false, None).await,
        Err(Failure::Missing)
    ));
    assert_eq!(provider.creates.load(Ordering::SeqCst), 0);
}
#[tokio::test]
async fn lost_waiter_does_not_abandon_the_creation_receipt() {
    let (service, store, provider) = fixture();
    let task = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(1), true, None).await })
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
    assert!(service.open(id(1), true, None).await.is_ok());
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}
#[tokio::test]
async fn failed_receipt_storage_attempts_cleanup_and_preserves_uncertainty() {
    let (service, store, provider, audit) = fixture_with_audit();
    store.fail_receipt.store(true, Ordering::SeqCst);
    provider.proceed.add_permits(1);
    assert_eq!(
        service.open(id(1), true, None).await.err(),
        Some(Failure::ReceiptRecovery {
            vm: format!("vm-{}", id(1)),
            receipt_audit_failed: false,
            cleanup_unconfirmed: true,
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
        service.open(id(1), true, None).await,
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
                    service.open(id(2), true, None).await.err(),
                    Some(Failure::ReceiptRecovery {
                        vm: format!("vm-{}", id(2)),
                        receipt_audit_failed,
                        cleanup_unconfirmed: !cleanup_confirmed,
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
                    service.open(id(2), true, None).await.err(),
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
        tokio::spawn(async move { service.open(id(3), true, None).await })
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
        service.open(id(3), true, None).await.err(),
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
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let factory = ThreadWorkspaces {
        service,
        channels: Arc::new(
            crate::conversation::infrastructure::LocalChannelStore::open(
                &directory.path().join("channels.sqlite3"),
            )
            .unwrap(),
        ),
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
        service.open(id(3), true, None).await.err(),
        Some(Failure::ReceiptRecovery {
            vm: format!("vm-{}", id(3)),
            receipt_audit_failed: true,
            cleanup_unconfirmed: true,
            cleanup_audit_failed: false,
        })
    );
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(
        *audit.events.lock().unwrap(),
        vec![recovery_events(&id(3), false)[1].clone()]
    );
    assert_eq!(
        service.open(id(3), true, None).await.err(),
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
        service.open(id(4), true, None).await.err(),
        Some(Failure::ReceiptRecovery {
            vm: format!("vm-{}", id(4)),
            receipt_audit_failed: false,
            cleanup_unconfirmed: true,
            cleanup_audit_failed: false,
        })
    );
    assert_eq!(
        *audit.events.lock().unwrap(),
        recovery_events(&id(4), false)
    );
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    assert_eq!(
        service.open(id(4), true, None).await.err(),
        Some(Failure::ProvisioningUncertain)
    );
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}

#[async_trait]
impl RuntimeSettings for Config {
    async fn read(&self, scope: &VmLifetimeScope) -> Result<VmLifetimeSettings, Failure> {
        Ok(match scope {
            VmLifetimeScope::Global => VmLifetimeSettings::global(Default::default()),
            VmLifetimeScope::Channel(channel) => {
                VmLifetimeSettings::for_channel(channel.clone(), Default::default(), None)
            }
        })
    }
    async fn save(
        &self,
        _: VmLifetimeScope,
        _: Option<VmLifetime>,
        _: RuntimeCaller,
    ) -> Result<VmLifetimeSettings, Failure> {
        Err(Failure::Storage)
    }
}

struct WrongRuntimeScope;
#[async_trait]
impl RuntimeSettings for WrongRuntimeScope {
    async fn read(&self, _: &VmLifetimeScope) -> Result<VmLifetimeSettings, Failure> {
        Ok(VmLifetimeSettings::for_channel(
            ChannelId::from_random([2; 10]),
            VmLifetime::from_seconds(Some(14_400)).unwrap(),
            None,
        ))
    }
    async fn save(
        &self,
        _: VmLifetimeScope,
        _: Option<VmLifetime>,
        _: RuntimeCaller,
    ) -> Result<VmLifetimeSettings, Failure> {
        Err(Failure::Storage)
    }
}
#[tokio::test]
async fn another_scopes_valid_lifetime_snapshot_cannot_authorize_vm_allocation() {
    let (_, store, provider, audit) = fixture_with_audit();
    provider.proceed.add_permits(1);
    let service = Workspaces::new(
        Arc::new(Config),
        provider.clone(),
        store.clone(),
        audit,
        Arc::new(WrongRuntimeScope),
    );
    assert_eq!(
        service.open(id(1), true, None).await.err(),
        Some(Failure::Storage)
    );
    assert_eq!(provider.creates.load(Ordering::SeqCst), 0);
    assert!(store.records.lock().unwrap().is_empty());
}
