//! Allocation is tested through the application ports with controllable provider
//! admission. No real account or secret is used in these ordering fixtures.
use crate::workspaces::{application::*, domain::*};
use async_trait::async_trait;
use nessa_protocol::conversation::domain::ConversationId;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
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
}
#[async_trait]
impl Assignments for Store {
    async fn read(&self, thread: &str) -> Result<Option<Assignment>, Failure> {
        Ok(self.records.lock().unwrap().get(thread).cloned())
    }
    async fn save(&self, value: Assignment) -> Result<(), Failure> {
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
struct FakeProvider {
    creates: AtomicUsize,
    connects: Mutex<Vec<String>>,
    deletes: AtomicUsize,
    entered: Semaphore,
    proceed: Semaphore,
}
impl FakeProvider {
    fn new() -> Self {
        Self {
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
    async fn create(&self, _: &BoatConnection, thread: &str) -> Result<String, Failure> {
        self.creates.fetch_add(1, Ordering::SeqCst);
        self.entered.add_permits(1);
        self.proceed.acquire().await.unwrap().forget();
        Ok(format!("vm-{thread}"))
    }
    async fn connect(
        &self,
        _: &BoatConnection,
        _: &str,
        vm: &str,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        self.connects.lock().unwrap().push(vm.into());
        Ok(Arc::new(Runtime))
    }
    async fn remove(&self, _: &BoatConnection, _: &str) -> Result<(), Failure> {
        self.deletes.fetch_add(1, Ordering::SeqCst);
        Err(Failure::CleanupUncertain)
    }
}
fn id(value: u128) -> ConversationId {
    ConversationId::new(&uuid::Uuid::from_u128(value).to_string()).unwrap()
}
fn fixture() -> (Arc<Workspaces>, Arc<Store>, Arc<FakeProvider>) {
    let store = Arc::new(Store::default());
    let provider = Arc::new(FakeProvider::new());
    (
        Arc::new(Workspaces::new(
            Arc::new(Config),
            provider.clone(),
            store.clone(),
        )),
        store,
        provider,
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
    let (service, store, provider) = fixture();
    store.fail_receipt.store(true, Ordering::SeqCst);
    provider.proceed.add_permits(1);
    assert!(matches!(
        service.open(id(1), true).await,
        Err(Failure::CleanupUncertain)
    ));
    assert_eq!(provider.deletes.load(Ordering::SeqCst), 1);
    assert!(matches!(
        service.open(id(1), true).await,
        Err(Failure::ProvisioningUncertain)
    ));
    assert_eq!(provider.creates.load(Ordering::SeqCst), 1);
}

#[test]
fn connection_cache_refuses_capacity_without_evicting_borrowed_environments() {
    use crate::conversation::{
        application::{ConversationError, ThreadEnvironments},
        infrastructure::{
            ssh_environment::DurableEnvironmentAudit, ThreadWorkspaces, MAX_RETAINED_ENVIRONMENTS,
        },
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
