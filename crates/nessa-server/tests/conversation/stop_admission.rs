//! Independent commands share receipt writes without sharing effect ownership.
use super::*;
use crate::conversation::application::{
    ConversationCreationAudit, ConversationCreationAuditRecord, ConversationCreationCause,
    ConversationFuture,
};
use nessa_sdk::application::agent_execution::commands::{
    CommandAccess, CreationStage, CreationStorageError, MAX_COMMAND_OWNERS,
};
use nessa_sdk::application::agent_execution::sessions::StorageError;
use tokio::sync::Notify;

fn creation_rows(root: &Path) -> i64 {
    let db = nessa_local_database::rusqlite::Connection::open(root.join("records/records.sqlite3"))
        .unwrap();
    db.busy_timeout(Duration::from_secs(5)).unwrap();
    db.query_row(
        "SELECT COUNT(*) FROM event_records WHERE schema_id = 'nessa.creation'",
        [],
        |row| row.get(0),
    )
    .unwrap()
}

#[derive(Default)]
struct ReopenAuditGate {
    entered: Notify,
    release: Notify,
}

impl ConversationCreationAudit for ReopenAuditGate {
    fn record(&self, record: ConversationCreationAuditRecord) -> ConversationFuture<'_, ()> {
        Box::pin(async move {
            if record.cause == ConversationCreationCause::IdempotentReopen {
                self.entered.notify_one();
                self.release.notified().await;
            }
            Ok(())
        })
    }
}

#[tokio::test]
async fn disconnected_reopen_retains_its_request_through_the_reopen_audit() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    retire(service, storage, metadata).await;

    let provider = Arc::new(ProviderFactory::default());
    let audit = Arc::new(ReopenAuditGate::default());
    let (service, storage, metadata) =
        fixture_with_creation_audit(&root, provider.clone(), audit.clone());
    let reopen = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .create_command(
                    storage,
                    target,
                    caller("reopen"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), audit.entered.notified())
        .await
        .unwrap();
    reopen.abort();
    assert!(reopen.await.unwrap_err().is_cancelled());
    assert!(matches!(
        storage
            .open_command("person".into(), "reopen".into(), CommandAccess::Stop)
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(creation_rows(&root), 0);
    assert_eq!(command_rows(&root), 0);
    audit.release.notify_one();
    let next_owner = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match storage
                .open_command("person".into(), "reopen".into(), CommandAccess::Stop)
                .await
            {
                Ok(Some(lease)) => break lease,
                Err(CreationStorageError::Storage(StorageError::Busy)) => {
                    tokio::task::yield_now().await;
                }
                outcome => panic!(
                    "unexpected reopen ownership outcome: {:?}",
                    outcome.map(|lease| lease.is_some())
                ),
            }
        }
    })
    .await
    .unwrap();
    drop(next_owner);
    assert!(service
        .lookup_creation(
            storage.clone(),
            target,
            caller("reopen"),
            RequestedConversation::default(),
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(creation_rows(&root), 0);
    assert_eq!(command_rows(&root), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn existing_target_reopen_refuses_a_live_stop_request_before_acceptance() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let claim = storage
        .open_command("person".into(), "claimed".into(), CommandAccess::Stop)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(command_rows(&root), 0);
    assert_eq!(creation_rows(&root), 0);
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target.clone(),
                caller("claimed"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Storage(CreationStorageError::Storage(
            StorageError::Busy
        )))
    ));
    assert_eq!(
        service
            .create_command(
                storage.clone(),
                target,
                caller("independent-reopen"),
                RequestedConversation::default(),
            )
            .await
            .unwrap()
            .stage(),
        CreationStage::Ready
    );
    assert_eq!(command_rows(&root), 0);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    assert_eq!(creation_rows(&root), 0);
    assert!(claim.load_mutation("claimed").await.unwrap().is_none());
    drop(claim);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn competing_creation_requests_for_one_target_do_not_add_a_second_receipt() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    let (release_open, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    let creation = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .create_command(
                    storage,
                    target,
                    caller("original"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
        .await
        .unwrap();
    assert_eq!(creation_rows(&root), 2);
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                target.clone(),
                caller("second"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Storage(CreationStorageError::Storage(
            StorageError::Busy
        )))
    ));
    assert_eq!(creation_rows(&root), 2);
    release_open.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), creation)
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .stage(),
        CreationStage::Ready
    );
    assert_eq!(creation_rows(&root), 3);
    assert_eq!(
        service
            .create_command(
                storage.clone(),
                target.clone(),
                caller("second"),
                RequestedConversation::default(),
            )
            .await
            .unwrap()
            .stage(),
        CreationStage::Ready
    );
    assert!(service
        .lookup_creation(
            storage.clone(),
            target,
            caller("second"),
            RequestedConversation::default(),
        )
        .await
        .unwrap()
        .is_none());
    assert_eq!(creation_rows(&root), 3);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 1);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn stop_completes_while_unrelated_creation_is_opening() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let active = id();
    opened(&service, &provider, &active).await;
    let execution = hold_turn(&service, &provider, &active, "active").await;
    // Consume the first attachment's notification before arming the next one.
    provider.opening.notified().await;
    let (release_open, gate) = oneshot::channel();
    *provider.open_gate.lock().unwrap() = Some(gate);
    let opening = id();
    let creation = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let opening = opening.clone();
        async move {
            service
                .create_command(
                    storage,
                    opening,
                    caller("open-other"),
                    RequestedConversation::default(),
                )
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
        .await
        .unwrap();
    // The provider gate remains closed throughout Stop, its retry, and lookups.
    let stopped = tokio::time::timeout(
        Duration::from_secs(5),
        service.stop_command(
            storage.clone(),
            active.clone(),
            caller("stop-active"),
            "active".into(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stopped.stage(), MutationStage::Settled);
    assert_eq!(stopped.outcome(), Some(MutationOutcome::Cancelled));
    assert!(!creation.is_finished());
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.cancel_turns.lock().unwrap().as_slice(), ["active"]);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    let pending = tokio::time::timeout(
        Duration::from_secs(5),
        service.lookup_creation(
            storage.clone(),
            opening.clone(),
            caller("open-other"),
            RequestedConversation::default(),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(pending.stage(), CreationStage::Attempted);
    assert_eq!(
        service
            .stop_command(
                storage.clone(),
                active.clone(),
                caller("stop-active"),
                "active".into(),
            )
            .await
            .unwrap(),
        stopped
    );
    release_open.send(()).unwrap();
    let created = tokio::time::timeout(Duration::from_secs(5), creation)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert_eq!(created.stage(), CreationStage::Ready);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 2);
    drop(execution);
    retire(service, storage, metadata).await;

    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert_eq!(
        service
            .stop_command(
                storage.clone(),
                active,
                caller("stop-active"),
                "active".into(),
            )
            .await
            .unwrap(),
        stopped
    );
    assert_eq!(
        service
            .create_command(
                storage.clone(),
                opening,
                caller("open-other"),
                RequestedConversation::default(),
            )
            .await
            .unwrap(),
        created
    );
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn disconnected_stop_keeps_its_request_while_other_commands_and_reads_progress() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let execution = hold_turn(&service, &provider, &target, "active").await;
    let (release_cancel, gate) = oneshot::channel();
    *provider.cancel_gate.lock().unwrap() = Some(gate);
    let stop = tokio::spawn({
        let service = service.clone();
        let storage = storage.clone();
        let target = target.clone();
        async move {
            service
                .stop_command(storage, target, caller("stop"), "active".into())
                .await
        }
    });
    tokio::time::timeout(Duration::from_secs(5), provider.cancel_started.notified())
        .await
        .unwrap();
    stop.abort();
    assert!(stop.await.unwrap_err().is_cancelled());
    assert!(service.inner.admission.try_write().is_err());
    assert!(matches!(
        service
            .stop_command(
                storage.clone(),
                target.clone(),
                caller("stop"),
                "active".into(),
            )
            .await,
        Err(MutationFailure::Storage(CreationStorageError::Storage(
            StorageError::Busy
        )))
    ));
    let binding = stop_lookup_binding(&target, &caller("stop"), "active").unwrap();
    let attempted = tokio::time::timeout(
        Duration::from_secs(5),
        service.lookup_command(
            storage.clone(),
            target.clone(),
            caller("stop"),
            binding.clone(),
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .unwrap();
    assert_eq!(attempted.stage(), MutationStage::Attempted);
    let unrelated = tokio::time::timeout(
        Duration::from_secs(5),
        service.create_command(
            storage.clone(),
            id(),
            caller("unrelated"),
            RequestedConversation::default(),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(unrelated.stage(), CreationStage::Ready);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    release_cancel.send(()).unwrap();
    let settled = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let receipt = service
                .lookup_command(
                    storage.clone(),
                    target.clone(),
                    caller("stop"),
                    binding.clone(),
                )
                .await
                .unwrap()
                .unwrap();
            if receipt.stage() == MutationStage::Settled {
                break receipt;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(settled.outcome(), Some(MutationOutcome::Cancelled));
    let retry = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            match service
                .stop_command(
                    storage.clone(),
                    target.clone(),
                    caller("stop"),
                    "active".into(),
                )
                .await
            {
                Ok(receipt) => break receipt,
                Err(MutationFailure::Storage(CreationStorageError::Storage(
                    StorageError::Busy,
                ))) => tokio::task::yield_now().await,
                outcome => panic!("unexpected Stop retry outcome: {outcome:?}"),
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(retry, settled);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    drop(execution);
    retire(service, storage, metadata).await;
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert_eq!(
        service
            .stop_command(storage.clone(), target, caller("stop"), "active".into())
            .await
            .unwrap(),
        settled
    );
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    retire(service, storage, metadata).await;
}

#[tokio::test]
async fn stop_reaches_the_provider_with_every_ordinary_command_owner_held() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    let target = id();
    opened(&service, &provider, &target).await;
    let execution = hold_turn(&service, &provider, &target, "active").await;
    let mut owners = Vec::new();
    for index in 0..MAX_COMMAND_OWNERS {
        owners.push(
            storage
                .open_command(
                    format!("occupied-principal-{index}"),
                    format!("occupied-request-{index}"),
                    CommandAccess::Creation,
                )
                .await
                .unwrap()
                .unwrap(),
        );
    }
    assert!(matches!(
        storage
            .open_command(
                "another-principal".into(),
                "overflow".into(),
                CommandAccess::Submit
            )
            .await,
        Err(CreationStorageError::Storage(StorageError::Busy))
    ));
    let stopped = tokio::time::timeout(
        Duration::from_secs(5),
        service.stop_command(storage.clone(), target, caller("stop"), "active".into()),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(stopped.outcome(), Some(MutationOutcome::Cancelled));
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 1);
    assert_eq!(provider.cancel_turns.lock().unwrap().as_slice(), ["active"]);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    // Stop did not make room by releasing or replacing any ordinary owner.
    for index in 0..MAX_COMMAND_OWNERS {
        assert!(matches!(
            storage
                .open_command(
                    format!("occupied-principal-{index}"),
                    format!("occupied-request-{index}"),
                    CommandAccess::Creation,
                )
                .await,
            Err(CreationStorageError::Storage(StorageError::Busy))
        ));
    }
    assert_eq!(owners.len(), MAX_COMMAND_OWNERS);
    drop(owners);
    drop(execution);
    retire(service, storage, metadata).await;
}

#[test]
#[ignore = "invoked only by the concurrent command crash parent"]
fn concurrent_creation_stop_crash_child() {
    let root = PathBuf::from(std::env::var_os("NESSA_MUTATION_ROOT").unwrap());
    let active = std::env::var("NESSA_MUTATION_TARGET").unwrap();
    let opening = std::env::var("NESSA_OPENING_TARGET").unwrap();
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let provider = Arc::new(ProviderFactory::default());
        provider.turn_cancel.store(true, Ordering::SeqCst);
        let (service, storage, _metadata) = fixture(&root, provider.clone());
        let active = ConversationId::new(&active).unwrap();
        let opening = ConversationId::new(&opening).unwrap();
        opened(&service, &provider, &active).await;
        let _held_execution = hold_turn(&service, &provider, &active, "active").await;
        provider.opening.notified().await;
        let (_held_open, gate) = oneshot::channel();
        *provider.open_gate.lock().unwrap() = Some(gate);
        let _creation = tokio::spawn({
            let service = service.clone();
            let storage = storage.clone();
            let opening = opening.clone();
            async move {
                service
                    .create_command(
                        storage,
                        opening,
                        caller("open-other"),
                        RequestedConversation::default(),
                    )
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), provider.opening.notified())
            .await
            .unwrap();
        let (_held_cancel, gate) = oneshot::channel();
        *provider.cancel_gate.lock().unwrap() = Some(gate);
        let _stop = tokio::spawn({
            let service = service.clone();
            let storage = storage.clone();
            let active = active.clone();
            async move {
                service
                    .stop_command(storage, active, caller("stop-active"), "active".into())
                    .await
            }
        });
        tokio::time::timeout(Duration::from_secs(5), provider.cancel_started.notified())
            .await
            .unwrap();
        assert_eq!(
            service
                .lookup_creation(
                    storage.clone(),
                    opening,
                    caller("open-other"),
                    RequestedConversation::default(),
                )
                .await
                .unwrap()
                .unwrap()
                .stage(),
            CreationStage::Attempted
        );
        let binding = stop_lookup_binding(&active, &caller("stop-active"), "active").unwrap();
        assert_eq!(
            service
                .lookup_command(storage, active, caller("stop-active"), binding)
                .await
                .unwrap()
                .unwrap()
                .stage(),
            MutationStage::Attempted
        );
        assert_eq!(command_rows(&root), 2);
        assert_eq!(creation_rows(&root), 2);
        let marker = root.join("concurrent-attempted.pending");
        std::fs::write(&marker, b"1").unwrap();
        std::fs::rename(marker, root.join("concurrent-attempted")).unwrap();
        // The process is killed with both provider calls pending. Their senders
        // stay in this frame so neither call can settle before the crash.
        std::future::pending::<()>().await;
    });
}

#[tokio::test]
async fn concurrent_creation_and_stop_attempts_survive_a_process_crash_without_repeating_effects() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("data");
    let active = id();
    let opening = id();
    let log = directory.path().join("concurrent-command-crash.log");
    let output = std::fs::File::create(&log).unwrap();
    let mut child = ChildOwner(
        Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "conversation::application::service::mutation::tests::stop_admission::concurrent_creation_stop_crash_child",
                "--ignored",
                "--nocapture",
            ])
            .env("NESSA_MUTATION_ROOT", &root)
            .env("NESSA_MUTATION_TARGET", active.to_string())
            .env("NESSA_OPENING_TARGET", opening.to_string())
            .stdout(Stdio::from(output.try_clone().unwrap()))
            .stderr(Stdio::from(output))
            .spawn()
            .unwrap(),
    );
    wait_marker(&root, "concurrent-attempted", &mut child, &log).await;
    child.0.kill().unwrap();
    assert!(!child.0.wait().unwrap().success());
    drop(child);
    let provider = Arc::new(ProviderFactory::default());
    provider.turn_cancel.store(true, Ordering::SeqCst);
    let (service, storage, metadata) = fixture(&root, provider.clone());
    assert!(matches!(
        service
            .create_command(
                storage.clone(),
                opening,
                caller("open-other"),
                RequestedConversation::default(),
            )
            .await,
        Err(CreationFailure::Interrupted(_))
    ));
    assert!(matches!(
        service
            .stop_command(
                storage.clone(),
                active,
                caller("stop-active"),
                "active".into()
            )
            .await,
        Err(MutationFailure::Interrupted(_))
    ));
    assert_eq!(provider.open_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.cancel_calls.load(Ordering::SeqCst), 0);
    assert_eq!(provider.close_calls.load(Ordering::SeqCst), 0);
    assert_eq!(command_rows(&root), 2);
    assert_eq!(creation_rows(&root), 2);
    retire(service, storage, metadata).await;
}
