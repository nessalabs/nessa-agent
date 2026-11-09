//! Recovery joins the original Closing operation after cleanup ownership binds.

use super::*;
use crate::conversation::application::environment::{LeaseHold, LeaseRelease};
use crate::conversation::infrastructure::DurableOwnershipAudit;
use nessa_sdk::application::agent_execution::commands::{
    CreationBinding, CreationFuture, CreationReceipt, CreationStage, CreationStorage,
    CreationStorageLease, MutationReceipt,
};
use nessa_sdk::application::agent_execution::permissions::ActionContext;
use nessa_sdk::application::agent_execution::subagents::{
    CloseCommand, LiveRoom, OwnershipFailure, OwnershipStore,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseEndCause, LeasePhase};
use nessa_sdk::domain::agent_execution::subagents::{
    HostActor, Initiator, LifetimeCause, LifetimeState,
};
use std::collections::HashSet;
use std::process::{Child, Command, Stdio};
use std::sync::atomic::AtomicUsize;
use std::sync::Mutex;

struct BeforeChildInitialization {
    storage: Arc<RecordStorage>,
    marker: std::path::PathBuf,
    attempted: bool,
}
struct BeforeChildInitializationLease {
    original: Arc<dyn CreationStorageLease>,
    marker: std::path::PathBuf,
    attempted: bool,
}
impl CreationStorage for BeforeChildInitialization {
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        Box::pin(async move {
            Ok(self
                .storage
                .open_creation(principal, request, create)
                .await?
                .map(|original| {
                    Arc::new(BeforeChildInitializationLease {
                        original,
                        marker: self.marker.clone(),
                        attempted: self.attempted,
                    }) as Arc<dyn CreationStorageLease>
                }))
        })
    }
    fn mutations(&self, principal: String) -> CreationFuture<'_, Vec<MutationReceipt>> {
        self.storage.mutations(principal)
    }
    fn creations(&self, principal: String) -> CreationFuture<'_, Vec<CreationReceipt>> {
        self.storage.creations(principal)
    }
}
impl CreationStorageLease for BeforeChildInitializationLease {
    fn load(&self, request: &str) -> CreationFuture<'_, Option<CreationReceipt>> {
        self.original.load(request)
    }
    fn save(&self, receipt: CreationReceipt) -> CreationFuture<'_, ()> {
        Box::pin(async move {
            if receipt.binding().actor().request_id() == "accepted-child"
                && receipt.stage() == CreationStage::Attempted
            {
                assert_eq!(
                    self.original.load("accepted-child").await?.unwrap().stage(),
                    CreationStage::Accepted
                );
                if self.attempted {
                    self.original.save(receipt.clone()).await?;
                }
                let pending = self.marker.with_extension("pending");
                std::fs::write(&pending, b"creation-before-initialization").unwrap();
                std::fs::rename(pending, &self.marker).unwrap();
                std::future::pending::<()>().await;
            }
            self.original.save(receipt).await
        })
    }
    fn load_mutation(&self, request: &str) -> CreationFuture<'_, Option<MutationReceipt>> {
        self.original.load_mutation(request)
    }
    fn save_mutation(&self, receipt: MutationReceipt) -> CreationFuture<'_, ()> {
        self.original.save_mutation(receipt)
    }
}

struct AcceptedCrashChild(Child);
impl Drop for AcceptedCrashChild {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

#[test]
#[ignore = "invoked only by the Accepted recovery crash-window fixture"]
fn accepted_child_crash_probe() {
    let root = std::path::PathBuf::from(
        std::env::var_os("NESSA_ACCEPTED_CHILD_CRASH_ROOT").expect("child fixture root"),
    );
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let provider = Arc::new(ProviderFactory::default());
            let (service, storage, metadata) = fixture(&root, provider.clone());
            service.bind_commands(Arc::new(BeforeChildInitialization {
                storage: storage.clone(),
                marker: root.join("creation-before-initialization"),
                attempted: std::env::var("NESSA_CHILD_CRASH_PREFIX").unwrap() == "attempted",
            }));
            bind_with_capacity(&service, &storage, &root, true, 1).await;
            let parent = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
            opened(&service, &provider, &parent).await;
            let _hold = hold_turn(&service, &provider, &parent, "parent").await;
            assert!(metadata
                .load(&ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap())
                .await
                .unwrap()
                .is_none());
            service
                .create_owned_thread(
                    source(&parent, "parent", "accepted-child"),
                    ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap(),
                    RequestedConversation::default(),
                )
                .await
                .unwrap();
            panic!("crash fixture escaped its pre-initialization gate");
        });
}

#[tokio::test]
async fn accepted_child_without_metadata_releases_its_original_reservation_after_restart() {
    child_crash_recovery(false, RecoveryObservation::Current).await;
}

#[derive(Clone, Copy, Debug)]
enum RecoveryObservation {
    Current,
    StaleAccepted,
    ForeignPrincipal,
    WrongTarget,
    WrongFingerprint,
    WrongRequest,
}
struct PreloadedCreationObservation {
    storage: Arc<RecordStorage>,
    commands: Arc<RecordStorage>,
    receipt: CreationReceipt,
}
struct CreationLookupOnly(Arc<RecordStorage>);
impl CreationStorage for CreationLookupOnly {
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        self.0.open_creation(principal, request, create)
    }
    fn mutations(&self, principal: String) -> CreationFuture<'_, Vec<MutationReceipt>> {
        self.0.mutations(principal)
    }
    // No discovery capability: a completed graph proof needs no new observation.
}
impl CreationStorage for PreloadedCreationObservation {
    fn open_creation(
        &self,
        principal: String,
        request: String,
        create: bool,
    ) -> CreationFuture<'_, Option<Arc<dyn CreationStorageLease>>> {
        self.commands.open_creation(principal, request, create)
    }
    fn mutations(&self, principal: String) -> CreationFuture<'_, Vec<MutationReceipt>> {
        self.storage.mutations(principal)
    }
    fn creations(&self, _: String) -> CreationFuture<'_, Vec<CreationReceipt>> {
        Box::pin(async { Ok(vec![self.receipt.clone()]) })
    }
}

#[tokio::test]
async fn child_recovery_does_not_promote_uncertain_or_mismatched_creation_evidence() {
    for (attempted, observation) in [
        (true, RecoveryObservation::Current),
        (true, RecoveryObservation::StaleAccepted),
        (false, RecoveryObservation::ForeignPrincipal),
        (false, RecoveryObservation::WrongTarget),
        (false, RecoveryObservation::WrongFingerprint),
        (false, RecoveryObservation::WrongRequest),
    ] {
        child_crash_recovery(attempted, observation).await;
    }
}

async fn child_crash_recovery(attempted: bool, observation: RecoveryObservation) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let log = directory.path().join("accepted-crash.log");
    let output = std::fs::File::create(&log).unwrap();
    let error = output.try_clone().unwrap();
    let mut process = AcceptedCrashChild(Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "conversation::application::service::mutation::tests::parent_stop::owned_recovery::accepted_child_crash_probe", "--ignored", "--nocapture"])
        .env("NESSA_ACCEPTED_CHILD_CRASH_ROOT", &root)
        .env("NESSA_CHILD_CRASH_PREFIX", if attempted { "attempted" } else { "accepted" })
        .stdout(Stdio::from(output)).stderr(Stdio::from(error)).spawn().unwrap());
    tokio::time::timeout(TURN_HOLD_HANG, async {
        loop {
            match std::fs::read(root.join("creation-before-initialization")) {
                Ok(bytes) => {
                    assert_eq!(bytes, b"creation-before-initialization");
                    break;
                }
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => panic!("cannot observe Accepted prefix: {error}"),
            }
            assert!(
                process.0.try_wait().unwrap().is_none(),
                "fixture exited: {}",
                std::fs::read_to_string(&log).unwrap_or_default()
            );
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    process.0.kill().unwrap();
    assert!(!process.0.wait().unwrap().success());
    drop(process);

    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let prefix = owned.read().await.unwrap();
    let original = prefix
        .spawns
        .iter()
        .find(|row| row.child_session.as_str() == "00000000-0000-4000-8000-000000000002")
        .unwrap();
    assert_eq!(original.progress, SpawnProgress::Reserved);
    let lifetime = original.child_lifetime.clone();
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap();
    assert!(metadata.load(&target).await.unwrap().is_none());
    let receipt = storage
        .open_creation("person".into(), "accepted-child".into(), false)
        .await
        .unwrap()
        .unwrap();
    let original = receipt.load("accepted-child").await.unwrap().unwrap();
    assert_eq!(
        original.stage(),
        if attempted {
            CreationStage::Attempted
        } else {
            CreationStage::Accepted
        }
    );
    drop(receipt);
    let mut alternate_commands = None;
    if !matches!(observation, RecoveryObservation::Current) {
        let actual = original.binding();
        let actor = actual.actor();
        let observed = CreationBinding::new(
            ActionContext::new(
                if matches!(observation, RecoveryObservation::ForeignPrincipal) {
                    "foreign"
                } else {
                    actor.principal_id()
                },
                actor.surface_id(),
                if matches!(observation, RecoveryObservation::WrongRequest) {
                    "different-request"
                } else {
                    actor.request_id()
                },
            )
            .unwrap(),
            if matches!(observation, RecoveryObservation::WrongTarget) {
                conversation_session(&id())
            } else {
                actual.target().clone()
            },
            if matches!(observation, RecoveryObservation::WrongFingerprint) {
                [9; 32]
            } else {
                *actual.fingerprint()
            },
        );
        let observed = CreationReceipt::accepted(observed);
        let commands = if matches!(observation, RecoveryObservation::StaleAccepted) {
            storage.clone()
        } else {
            // These are valid stored command records from a mismatched owner
            // or binding, not merely a stale observation rejected by reload.
            // Recovery must correlate the graph before trusting that lease.
            let commands = Arc::new(RecordStorage::new(root.join("mismatched-commands")).unwrap());
            let actor = observed.binding().actor();
            let lease = commands
                .open_creation(actor.principal_id().into(), actor.request_id().into(), true)
                .await
                .unwrap()
                .unwrap();
            lease.save(observed.clone()).await.unwrap();
            drop(lease);
            alternate_commands = Some(commands.clone());
            commands
        };
        service.bind_commands(Arc::new(PreloadedCreationObservation {
            storage: storage.clone(),
            commands,
            receipt: observed,
        }));
    }
    // The injected child quota is one; ordinary live-map admission keeps its
    // normal fixture limit. This isolates the retained child reservation.
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let recovered = coordinator.lifetime_state(&lifetime);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    let parent = ConversationId::new("00000000-0000-4000-8000-000000000003").unwrap();
    opened(&service, &provider, &parent).await;
    let hold = hold_turn(&service, &provider, &parent, "after-recovery").await;
    let fresh = service
        .create_owned_thread(
            source(&parent, "after-recovery", "fresh-child"),
            id(),
            RequestedConversation::default(),
        )
        .await;
    drop(hold);
    let opens = provider.open_calls.load(Ordering::SeqCst);
    retire(service, storage, metadata).await;
    if let Some(commands) = alternate_commands {
        commands.shutdown().await.unwrap();
    }
    if !attempted && matches!(observation, RecoveryObservation::Current) {
        assert_eq!(
            recovered,
            Some(LifetimeState::Closed),
            "Accepted proves no initializer was authorized"
        );
        assert!(
            fresh.is_ok(),
            "the original child reservation was not released: {fresh:?}"
        );
        assert_eq!(
            opens, 2,
            "only the new parent and legitimate new child open"
        );
        let provider = Arc::new(ProviderFactory::default());
        let (service, storage, metadata) = fixture(&root, provider.clone());
        service.bind_commands(Arc::new(CreationLookupOnly(storage.clone())));
        let restored = bind_with_capacity(&service, &storage, &root, true, 1).await;
        assert_eq!(
            restored.lifetime_state(&lifetime),
            Some(LifetimeState::Closed)
        );
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
        // The failed reservation never acquired this target. Ordinary opening
        // gets a new root rather than reopening its historical child lifetime.
        opened(&service, &provider, &target).await;
        let root_lifetime = restored
            .active_root_for_session(&conversation_session(&target))
            .unwrap();
        assert_ne!(root_lifetime, lifetime);
        assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
        retire(service, storage, metadata).await;
    } else {
        assert_eq!(
            recovered,
            Some(LifetimeState::Open),
            "{attempted}/{observation:?}"
        );
        assert!(
            matches!(
                &fresh,
                Err(commands::CreationFailure::Target(
                    ConversationError::Unavailable
                ))
            ),
            "uncertain or mismatched evidence cannot free quota: {fresh:?}"
        );
        assert_eq!(opens, 1, "no child initializer dispatched");
    }
}
#[tokio::test]
async fn startup_retries_a_completed_closing_generation_after_rebinding_its_owner() {
    closing_recovery_after_rebind(false).await;
}

#[tokio::test]
async fn startup_keeps_uncertain_cleanup_owned_and_retries_the_same_close_after_confirmation() {
    closing_recovery_after_rebind(true).await;
}

/// Physical provider cleanup happens normally, but this environment withholds
/// its acknowledgement. Later account answers refer only to leases actually
/// ended by this hold; no graph rows or lease records are fabricated.
struct WithheldCleanup {
    inner: Arc<dyn Environment>,
    ended: Arc<Mutex<HashSet<LeaseId>>>,
    answer: AtomicUsize,
    accounts: AtomicUsize,
}
struct WithheldHold {
    inner: Arc<dyn LeaseHold>,
    lease: LeaseId,
    ended: Arc<Mutex<HashSet<LeaseId>>>,
}
impl LeaseHold for WithheldHold {
    fn model_provider(
        &self,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        self.inner.model_provider(binding)
    }

    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        self.inner.ends()
    }
    fn end(&self, cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async move {
            let result = self.inner.end(cause).await;
            if matches!(
                result,
                LeaseRelease::ByAgentClose | LeaseRelease::Released(LeaseCleanup::Confirmed { .. })
            ) {
                self.ended.lock().unwrap().insert(self.lease.clone());
            }
            LeaseRelease::Unanswered
        })
    }
}
impl Environment for WithheldCleanup {
    fn declaration(&self) -> EnvironmentDeclaration {
        self.inner.declaration()
    }
    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            let mut opened = self.inner.open(lease, grant, binding).await?;
            opened.hold = Arc::new(WithheldHold {
                inner: opened.hold,
                lease: lease.clone(),
                ended: self.ended.clone(),
            });
            Ok(opened)
        })
    }
    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(async move {
            self.accounts.fetch_add(1, Ordering::SeqCst);
            match self.answer.load(Ordering::SeqCst) {
                0 => None,
                1 => Some(LeaseCleanup::NotHeld),
                _ if self.ended.lock().unwrap().contains(lease) => {
                    Some(LeaseCleanup::Confirmed { forced: false })
                }
                _ => None,
            }
        })
    }
}

async fn closing_recovery_after_rebind(withheld: bool) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let environment = Arc::new(WithheldCleanup {
        inner: crate::conversation::infrastructure::in_process_environment(),
        ended: Arc::default(),
        answer: AtomicUsize::new(0),
        accounts: AtomicUsize::new(0),
    });
    let environment_port: Arc<dyn Environment> = if withheld {
        environment.clone()
    } else {
        environment.inner.clone()
    };
    let (service, storage, metadata) =
        fixture_in(&root, provider.clone(), environment_port.clone().into());
    let coordinator = bind_with_capacity(&service, &storage, &root, true, 1).await;
    let parent = id();
    opened(&service, &provider, &parent).await;
    let p = hold_turn(&service, &provider, &parent, "parent").await;
    let child_id = child(&service, &parent, "parent", "child").await;
    let lifetime = coordinator
        .active_lifetime_for_session(&conversation_session(&child_id))
        .unwrap();
    drop(p);
    if withheld {
        let Err(ConversationError::Retirement(failures)) = service.shutdown().await else {
            panic!("shutdown must retain and report unanswered environment cleanup");
        };
        assert_eq!(failures.len(), 2);
        for id in [&parent, &child_id] {
            assert!(failures
                .iter()
                .any(|(target, error)| target == &id.to_string()
                    && matches!(error, AgentError::CleanupUncertain)));
        }
        drop(service);
        storage.shutdown().await.unwrap();
        drop(storage);
        drop(metadata);
    } else {
        retire(service, storage, metadata).await;
    }
    let command = || CloseCommand {
        lifetime: lifetime.clone(),
        cause: LifetimeCause::HostClose,
        initiator: Initiator::Host(HostActor::new("person", "panel", "host-close").unwrap()),
        external_attachment: false,
        timeout: Some(TURN_HOLD_HANG),
    };
    // The real cleanup owner has gone; the supported SDK close persists its
    // original operation but cannot claim physical cleanup through a dead Weak.
    assert!(matches!(
        coordinator.end_lifetime(command()).await,
        Err(OwnershipFailure::Incomplete)
    ));
    let owned = SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap();
    let closing = owned.read().await.unwrap();
    let original = closing
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == lifetime)
        .unwrap()
        .clone();
    assert_eq!(original.state, LifetimeState::Closing);
    assert_eq!(original.cause, Some(LifetimeCause::HostClose));
    drop(coordinator);
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture_in(&root, provider.clone(), environment_port.into());
    if withheld {
        let (committed, _) = service.committed_with_order(&child_id).await.unwrap();
        assert!(
            committed
                .snapshot()
                .unwrap()
                .lease
                .as_ref()
                .unwrap()
                .held()
                .is_some_and(|lease| matches!(
                    lease.phase(),
                    LeasePhase::Interrupted {
                        late_cleanup: None,
                        ..
                    }
                )),
            "the actual lease history retains lost cleanup acknowledgement"
        );
    }
    service.bind_commands(storage.clone());
    let room = Arc::new(LiveCapacity::new(1));
    let coordinator = OwnershipCoordinator::new(OwnershipDependencies {
        store: Arc::new(SqliteOwnershipStore::open(&root.join("ownership.sqlite3")).unwrap()),
        audit: Arc::new(
            DurableOwnershipAudit::new(root.join("ownership-audit"), Arc::new(TestClock)).unwrap(),
        ),
        factory: service.owned_thread_factory(),
        room: room.clone(),
    });
    coordinator.resume().await.unwrap();
    // A public close retry joins/completes the empty-adapter generation. This
    // barrier makes the recovery ordering deterministic without a timing sleep.
    assert!(matches!(
        coordinator.end_lifetime(command()).await,
        Err(OwnershipFailure::Incomplete)
    ));
    service
        .bind_owned_threads(coordinator.clone(), true)
        .unwrap();
    let snapshot = owned.read().await.unwrap();
    assert!(metadata
        .load(&child_id)
        .await
        .unwrap()
        .unwrap()
        .deletion()
        .is_none());
    service.recover_owned_threads(&snapshot).await.unwrap();
    if withheld {
        for answer in [0, 1] {
            environment.answer.store(answer, Ordering::SeqCst);
            service
                .recover_owned_threads(&owned.read().await.unwrap())
                .await
                .unwrap();
            assert_eq!(
                coordinator.lifetime_state(&lifetime),
                Some(LifetimeState::Closing)
            );
            let pending = owned.read().await.unwrap();
            let row = pending
                .lifetimes
                .iter()
                .find(|row| row.lifetime_id == lifetime)
                .unwrap();
            assert_eq!(row.close_operation, original.close_operation);
            assert_eq!(row.cause, original.cause);
            assert_eq!(row.initiator, original.initiator);
            assert!(
                !LiveRoom::try_reserve(room.as_ref()),
                "uncertain cleanup retains its capacity"
            );
        }
        environment.answer.store(2, Ordering::SeqCst);
        service
            .recover_owned_threads(&owned.read().await.unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        coordinator.lifetime_state(&lifetime),
        Some(LifetimeState::Closed)
    );
    let closed = owned.read().await.unwrap();
    let row = closed
        .lifetimes
        .iter()
        .find(|row| row.lifetime_id == lifetime)
        .unwrap();
    assert_eq!(row.close_operation, original.close_operation);
    assert_eq!(row.cause, original.cause);
    assert_eq!(row.initiator, original.initiator);
    assert!(
        LiveRoom::try_reserve(room.as_ref()),
        "confirmed cleanup releases the single child slot"
    );
    LiveRoom::release(room.as_ref());
    let accounts = environment.accounts.load(Ordering::SeqCst);
    let executions = provider.executions.lock().unwrap().len();
    service.recover_owned_threads(&closed).await.unwrap();
    assert_eq!(provider.executions.lock().unwrap().len(), executions);
    assert_eq!(
        environment.accounts.load(Ordering::SeqCst),
        accounts,
        "settled ownership does not replay physical cleanup"
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}
