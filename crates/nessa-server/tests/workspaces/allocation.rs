//! Allocation is tested through the application ports with controllable provider
//! admission. No real account or secret is used in these ordering fixtures.
use crate::conversation::domain::{ChannelId, VmLifetime, VmLifetimeScope, VmLifetimeSettings};
use crate::mcp_servers::infrastructure::{ConfigCheck, ConfigJson, OsConfigFiles};
use crate::workspaces::infrastructure::{ConfigBoatRuntimeSettings, RuntimeSettingsAudit};
use crate::workspaces::{application::*, domain::*};
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
/// The Claude sign-in kept for Boat VMs, and the connection cleared with it.
#[derive(Default)]
struct SignIns {
    saved: std::sync::Mutex<Option<ClaudeSignIn>>,
    cleared: std::sync::atomic::AtomicBool,
}
#[async_trait]
impl ClaudeSignIns for SignIns {
    async fn read(&self) -> Result<Option<ClaudeSignIn>, Failure> {
        Ok(self.saved.lock().unwrap().clone())
    }
    async fn save(&self, sign_in: ClaudeSignIn) -> Result<(), Failure> {
        *self.saved.lock().unwrap() = Some(sign_in);
        Ok(())
    }
    async fn clear(&self) -> Result<(), Failure> {
        *self.saved.lock().unwrap() = None;
        self.cleared
            .store(true, std::sync::atomic::Ordering::SeqCst);
        Ok(())
    }
}
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
#[async_trait]
impl RuntimeSettings for Config {
    async fn read(&self, scope: &VmLifetimeScope) -> Result<VmLifetimeSettings, Failure> {
        Ok(match scope {
            VmLifetimeScope::Global => VmLifetimeSettings::global(VmLifetime::Unlimited),
            VmLifetimeScope::Channel(channel) => {
                VmLifetimeSettings::for_channel(channel.clone(), VmLifetime::Unlimited, None)
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
struct Runtime;
#[async_trait]
impl WorkspaceRuntime for Runtime {
    async fn serve(&self, _: ServedCopy) -> Result<WorkspaceStream, Failure> {
        Err(Failure::Provider)
    }
    async fn image_protocol(&self) -> Option<String> {
        None
    }
    async fn run(&self, _: String, _: Option<std::fs::File>) -> std::io::Result<String> {
        Err(std::io::Error::other("no VM"))
    }
}
#[derive(Clone, Debug, PartialEq, Eq)]
enum RecoveryStep {
    Audit(RecoveryEvent),
    Remove(String),
    Stop(String),
}
#[derive(Default)]
struct Audit {
    events: Mutex<Vec<RecoveryEvent>>,
    steps: Arc<Mutex<Vec<RecoveryStep>>>,
    fail_receipt: AtomicBool,
    fail_cleanup: AtomicBool,
    panic_receipt: AtomicBool,
    fail_stop: AtomicBool,
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
            RecoveryEvent::StopRequested { .. } | RecoveryEvent::StopAnswered { .. } => {
                &self.fail_stop
            }
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
    created_lifetimes: Mutex<Vec<VmLifetime>>,
    created_images: Mutex<Vec<String>>,
    connected_lifetimes: Mutex<Vec<VmLifetime>>,
    connects: Mutex<Vec<String>>,
    deletes: AtomicUsize,
    stops: Mutex<Vec<String>>,
    entered: Semaphore,
    proceed: Semaphore,
    confirm_remove: AtomicBool,
    panic_remove: AtomicBool,
    refuse_stop: AtomicBool,
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
            refuse_stop: AtomicBool::new(false),
            recovery_steps,
            creates: AtomicUsize::new(0),
            created_lifetimes: Mutex::new(vec![]),
            created_images: Mutex::new(vec![]),
            connected_lifetimes: Mutex::new(vec![]),
            connects: Mutex::new(vec![]),
            deletes: AtomicUsize::new(0),
            stops: Mutex::new(vec![]),
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
    async fn create(
        &self,
        connection: &BoatConnection,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
        self.created_lifetimes
            .lock()
            .unwrap()
            .push(connection.auto_stop);
        self.created_images
            .lock()
            .unwrap()
            .push(connection.image.clone());
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
        connection: &BoatConnection,
        _: &str,
        vm: &str,
        _: &EnvironmentIdentity,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        self.connected_lifetimes
            .lock()
            .unwrap()
            .push(connection.auto_stop);
        if let Some(rejection) = self.refusal_on_connect.lock().unwrap().clone() {
            return Err(Failure::AdmissionRejected(rejection));
        }
        self.connects.lock().unwrap().push(vm.into());
        Ok(Arc::new(Runtime))
    }
    async fn stop(&self, _: &BoatConnection, vm: &str) -> Result<(), Failure> {
        self.recovery_steps
            .lock()
            .unwrap()
            .push(RecoveryStep::Stop(vm.into()));
        if self.refuse_stop.load(Ordering::SeqCst) {
            return Err(Failure::Provider);
        }
        self.stops.lock().unwrap().push(vm.into());
        Ok(())
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
            Arc::new(SignIns::default()),
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
async fn config_default_and_channel_override_reach_admitted_creation_and_existing_connections() {
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let settings = Arc::new(ConfigBoatRuntimeSettings {
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
        Arc::new(SignIns::default()),
    );
    provider.proceed.add_permits(2);
    service.open(id(101), true, None).await.unwrap();
    service
        .open(id(102), true, Some(channel.clone()))
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
    service.open(id(101), true, None).await.unwrap();
    service.open(id(102), true, Some(channel)).await.unwrap();
    assert_eq!(provider.creates.load(Ordering::SeqCst), 2);
    assert_eq!(
        *provider.connected_lifetimes.lock().unwrap(),
        vec![finite, VmLifetime::Unlimited, VmLifetime::Unlimited, finite]
    );
}
#[tokio::test]
async fn wrong_scope_lifetime_snapshot_refuses_before_provider_create() {
    let (_, store, provider, audit) = fixture_with_audit();
    let service = Workspaces::new(
        Arc::new(Config),
        provider.clone(),
        store,
        audit,
        Arc::new(WrongRuntimeScope),
        Arc::new(SignIns::default()),
    );
    assert!(matches!(
        service.open(id(103), true, None).await,
        Err(Failure::Storage)
    ));
    assert_eq!(provider.creates.load(Ordering::SeqCst), 0);
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
async fn a_rest_stops_the_vm_only_once_nothing_uses_it_and_never_deletes_it() {
    let (service, _, provider) = fixture();
    // No VM yet: nothing to stop.
    assert_eq!(
        service.rest(id(1), RestCause::Idle).await.stopped,
        Ok(false)
    );
    provider.proceed.add_permits(1);
    let runtime = service.open(id(1), true, None).await.unwrap();
    // The runtime an opening handed out holds the VM in use.
    assert_eq!(
        service.rest(id(1), RestCause::Idle).await.stopped,
        Ok(false)
    );
    assert!(provider.stops.lock().unwrap().is_empty());
    drop(runtime);
    assert_eq!(service.rest(id(1), RestCause::Idle).await.stopped, Ok(true));
    assert_eq!(*provider.stops.lock().unwrap(), [format!("vm-{}", id(1))]);
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 0);
    // The next opening reaches the same VM again, which resumes it.
    assert!(service.open(id(1), true, None).await.is_ok());
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
    assert_eq!(provider.connects.lock().unwrap().len(), 2);
}
#[tokio::test]
async fn each_stop_is_recorded_with_its_cause_before_and_after_and_a_failed_record_never_holds_it_back(
) {
    let (service, _, provider, audit) = fixture_with_audit();
    provider.proceed.add_permits(1);
    drop(service.open(id(1), true, None).await.unwrap());
    let thread = id(1).to_string();
    let vm = format!("vm-{thread}");
    let rested = service.rest(id(1), RestCause::Closed).await;
    assert_eq!(rested.stopped, Ok(true));
    assert!(!rested.audit_failed);
    // Recorded before Boat is asked, and after it answered.
    assert_eq!(
        audit
            .steps
            .lock()
            .unwrap()
            .iter()
            .filter(|step| !matches!(
                step,
                RecoveryStep::Audit(
                    RecoveryEvent::ReceiptSaveFailed { .. }
                        | RecoveryEvent::AdmissionRefused { .. }
                        | RecoveryEvent::RollbackObserved { .. }
                )
            ))
            .cloned()
            .collect::<Vec<_>>(),
        [
            RecoveryStep::Audit(RecoveryEvent::StopRequested {
                thread: thread.clone(),
                vm: vm.clone(),
                cause: RestCause::Closed,
            }),
            RecoveryStep::Stop(vm.clone()),
            RecoveryStep::Audit(RecoveryEvent::StopAnswered {
                thread: thread.clone(),
                vm: vm.clone(),
                cause: RestCause::Closed,
                failure: None,
            }),
        ]
    );
    let stops = |audit: &Audit| -> Vec<RecoveryEvent> {
        audit
            .events
            .lock()
            .unwrap()
            .iter()
            .filter(|event| {
                matches!(
                    event,
                    RecoveryEvent::StopRequested { .. } | RecoveryEvent::StopAnswered { .. }
                )
            })
            .cloned()
            .collect()
    };
    assert_eq!(
        stops(&audit),
        [
            RecoveryEvent::StopRequested {
                thread: thread.clone(),
                vm: vm.clone(),
                cause: RestCause::Closed,
            },
            RecoveryEvent::StopAnswered {
                thread: thread.clone(),
                vm: vm.clone(),
                cause: RestCause::Closed,
                failure: None,
            },
        ]
    );
    // A store that refuses the record still sees the VM stopped, and says so.
    audit.fail_stop.store(true, Ordering::SeqCst);
    let rested = service.rest(id(1), RestCause::Idle).await;
    assert_eq!(rested.stopped, Ok(true));
    assert!(rested.audit_failed);
    assert_eq!(*provider.stops.lock().unwrap(), [vm.clone(), vm.clone()]);
    // A stop Boat refuses is recorded as not stopped.
    audit.fail_stop.store(false, Ordering::SeqCst);
    provider.refuse_stop.store(true, Ordering::SeqCst);
    let rested = service.rest(id(1), RestCause::Idle).await;
    assert_eq!(rested.stopped, Err(Failure::Provider));
    assert!(!rested.audit_failed);
    assert_eq!(
        stops(&audit).last(),
        Some(&RecoveryEvent::StopAnswered {
            thread,
            vm,
            cause: RestCause::Idle,
            failure: Some("provider"),
        })
    );
}
#[tokio::test]
async fn a_new_vm_is_made_from_the_pinned_image_not_the_one_saved_with_the_account() {
    let store = Arc::new(Store::default());
    let audit = Arc::new(Audit::default());
    let provider = Arc::new(FakeProvider::new(audit.steps.clone()));
    let service = Workspaces::new(
        Arc::new(Config),
        provider.clone(),
        store.clone(),
        audit,
        Arc::new(Config),
        Arc::new(SignIns::default()),
    )
    .with_image(Some("pinned-image"));
    // The account was connected while another build pinned "fixture-image".
    assert_eq!(connection().image, "fixture-image");
    provider.proceed.add_permits(1);
    assert!(service.open(id(1), true, None).await.is_ok());
    assert_eq!(*provider.created_images.lock().unwrap(), ["pinned-image"]);
    assert!(matches!(
        store.read(&id(1).to_string()).await,
        Ok(Some(Assignment::Assigned { image, .. })) if image == "pinned-image"
    ));
}
#[tokio::test]
async fn a_rest_waiting_behind_an_opening_keeps_the_vm_it_opens() {
    let (service, _, provider) = fixture();
    // The opening holds the thread's lock while it creates the VM; a rest
    // asked meanwhile runs after it, and finds the VM in use.
    let opening = {
        let service = service.clone();
        tokio::spawn(async move { service.open(id(1), true, None).await })
    };
    provider.entered.acquire().await.unwrap().forget();
    let rest = {
        let service = service.clone();
        tokio::spawn(async move { service.rest(id(1), RestCause::Idle).await.stopped })
    };
    provider.proceed.add_permits(1);
    let runtime = opening.await.unwrap().unwrap();
    assert_eq!(rest.await.unwrap(), Ok(false));
    assert!(provider.stops.lock().unwrap().is_empty());
    drop(runtime);
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
        infrastructure::{
            ssh_environment::DurableEnvironmentAudit, LocalChannelStore, ThreadWorkspaces,
        },
    };
    struct Clock;
    impl nessa_auth::application::ports::Clock for Clock {
        fn unix_milliseconds(&self) -> u64 {
            42
        }
    }
    let directory = tempfile::tempdir().unwrap();
    std::fs::set_permissions(directory.path(), std::fs::Permissions::from_mode(0o700)).unwrap();
    let (service, _, provider) = fixture();
    let factory = ThreadWorkspaces {
        service,
        channels: Arc::new(
            LocalChannelStore::open(&directory.path().join("channels.sqlite3")).unwrap(),
        ),
        audit: DurableEnvironmentAudit::new(directory.path().join("audit"), Arc::new(Clock))
            .unwrap(),
        installer: Arc::new(
            crate::conversation::infrastructure::ssh_environment::HostInstaller::this_build(
                Arc::new(
                    crate::conversation::infrastructure::ssh_environment::OpenSshConnector::new(),
                ),
                directory.path().join("host-builds"),
            ),
        ),
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
        service.open(id(4), true, None).await.err(),
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
                        service.open(id(4), true, None).await.err(),
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
                    let retry = service.open(id(4), true, None).await.err();
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
    assert!(service.open(id(5), true, None).await.is_ok());
    *provider.refusal_on_connect.lock().unwrap() = Some(rejection());
    assert!(matches!(
        service.open(id(5), false, None).await,
        Err(Failure::AdmissionRecovery { .. })
    ));
    assert!(matches!(
        store.read(&id(5).to_string()).await.unwrap(),
        Some(Assignment::Refused { .. })
    ));
    *provider.refusal_on_connect.lock().unwrap() = None;
    assert_eq!(
        service.open(id(5), true, None).await.err(),
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
        tokio::spawn(async move { service.open(id(6), true, None).await })
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
        service.open(id(6), true, None).await.err(),
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
        service.open(id(7), true, None).await.err(),
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
        service.open(id(7), true, None).await.err(),
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
        service.open(id(8), true, None).await,
        Err(Failure::AdmissionRecovery {
            cleanup: CleanupOutcome::Retained,
            ..
        })
    ));
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 0);
    assert_eq!(
        service.open(id(8), true, None).await.err(),
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
            assert!(service.open(id(9), true, None).await.is_ok());
            store.fail_refusal.store(receipt_failed, Ordering::SeqCst);
            audit.fail_receipt.store(audit_failed, Ordering::SeqCst);
            *provider.refusal_on_connect.lock().unwrap() = Some(rejection());
            assert_eq!(
                service.open(id(9), false, None).await.err(),
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

#[tokio::test]
async fn a_claude_sign_in_is_kept_only_in_the_token_alphabet() {
    let sign_ins = Arc::new(SignIns::default());
    let service = Workspaces::new(
        Arc::new(Config),
        Arc::new(FakeProvider::new(Default::default())),
        Arc::new(Store::default()),
        Arc::new(Audit::default()),
        Arc::new(Config),
        sign_ins.clone(),
    );
    for refused in [
        "",
        "sk-ant-api03-abc",
        "sk-ant-oat01-abc\nexport X=1",
        "sk-ant-oat01-a b",
        "sk-ant-oat01-'x'",
        &format!("sk-ant-oat01-{}", "a".repeat(512)),
    ] {
        assert_eq!(
            service.save_claude_sign_in(refused.into()).await,
            Err(Failure::InvalidInput),
            "{refused:?}"
        );
    }
    assert!(sign_ins.saved.lock().unwrap().is_none());
    service
        .save_claude_sign_in("sk-ant-oat01-first_A-1".into())
        .await
        .unwrap();
    service
        .save_claude_sign_in("sk-ant-oat01-second".into())
        .await
        .unwrap();
    assert_eq!(
        sign_ins.saved.lock().unwrap().as_ref().unwrap().as_bytes(),
        b"sk-ant-oat01-second"
    );
}

#[tokio::test]
async fn disconnecting_boat_forgets_its_claude_sign_in() {
    let sign_ins = Arc::new(SignIns::default());
    let service = Workspaces::new(
        Arc::new(Config),
        Arc::new(FakeProvider::new(Default::default())),
        Arc::new(Store::default()),
        Arc::new(Audit::default()),
        Arc::new(Config),
        sign_ins.clone(),
    );
    service
        .save_claude_sign_in("sk-ant-oat01-kept".into())
        .await
        .unwrap();
    service.disconnect().await.unwrap();
    assert!(sign_ins.saved.lock().unwrap().is_none());
    assert!(sign_ins.cleared.load(std::sync::atomic::Ordering::SeqCst));
}
