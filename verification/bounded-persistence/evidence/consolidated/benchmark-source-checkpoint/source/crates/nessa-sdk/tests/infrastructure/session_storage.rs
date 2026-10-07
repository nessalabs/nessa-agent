//! Public record-storage contract exercised through the SDK port.
//! session_storage -> memory public bindings / receipts / reset
//!                 -> record public writer / watch / physical interruption / retry
//!                 -> record_source public publication / restored extension
//!                 -> discovery public bounded query ordering / physical faults
//!                 -> save_group public leases / emitted records / checkpoints
//!                 -> fixtures public immutable data / actual emitted donor output

mod caller_wakes;
mod discovery;
mod fixtures;
mod memory;
mod record;
mod record_source;
mod save_group;

use nessa_sdk::{
    application::agent_execution::{
        providers::ProviderIdentity,
        sessions::{
            ProviderContext, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
            StorageError,
        },
    },
    domain::agent_execution::sessions::{ExecutionSessionId, SessionId},
    infrastructure::session_storage::{physical_record_schema, RecordStorage},
};
use nessa_sync::replication::{
    application::{begin_pass, finish_pass, RecordSource, StoreError, SyncError},
    domain::{Id, Limits, Scope},
    infrastructure::{
        LoopbackClient, LoopbackReadConfig, LoopbackRecordServer, MemoryAuthorizer, MemoryStore,
    },
};
use std::{
    io::{BufRead, BufReader, Write},
    net::{Ipv4Addr, SocketAddrV4},
    process::{Child, Command, Stdio},
    sync::Arc,
};

fn opened(id: &SessionId) -> (SessionChange, SessionSnapshot) {
    let provider = ProviderIdentity::new("fixture", "model", "workspace").unwrap();
    (
        SessionChange::Opened {
            id: id.clone(),
            provider: provider.clone(),
            context: ProviderContext::Absent,
        },
        SessionSnapshot {
            id: id.clone(),
            provider,
            provider_context: ProviderContext::Absent,
            invocations: Vec::new(),
            queue_history: Vec::new(),
        },
    )
}

#[tokio::test]
async fn record_storage_reopens_the_same_semantic_history() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    assert!(storage.open_existing(id.clone()).await.unwrap().is_none());
    let lease = storage.open(id.clone()).await.unwrap();
    let (change, snapshot) = opened(&id);
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot.clone(),
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    drop(lease);
    let restored = storage.open_existing(id).await.unwrap().unwrap();
    assert_eq!(restored.load().await.unwrap().snapshot(), Some(&snapshot));
    drop(restored);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn record_storage_lease_excludes_second_manager_and_reset_erases_history() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("conversation").unwrap();
    let first = storage.open(id.clone()).await.unwrap();
    assert!(matches!(
        storage.open(id.clone()).await,
        Err(StorageError::Busy)
    ));
    let (change, snapshot) = opened(&id);
    first
        .save_changes(
            first.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    first.erase().await.unwrap();
    assert!(first.load().await.unwrap().snapshot().is_none());
    drop(first);
    assert!(storage
        .open_existing(id)
        .await
        .unwrap()
        .unwrap()
        .load()
        .await
        .unwrap()
        .snapshot()
        .is_none());
}

#[tokio::test]
async fn record_source_reads_committed_frames_while_writer_lease_is_held() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let source = storage
        .record_source(&id, Id::new("origin").unwrap())
        .await
        .unwrap()
        .unwrap();
    let (change, snapshot) = opened(&id);
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    let scope = source.scope(Id::new("receiver").unwrap(), Id::new("epoch").unwrap());
    let (source, head, page) = tokio::task::spawn_blocking(move || {
        let mut source = source;
        let head = source.head(&scope).unwrap();
        let page = source
            .page(&nessa_sync::replication::domain::PageRequest {
                scope,
                after: 0,
                target: head,
                max_records: 2,
                max_payload_bytes: 128 * 1024,
                max_record_bytes: 128 * 1024,
            })
            .unwrap();
        (source, head, page)
    })
    .await
    .unwrap();
    assert_eq!(head, 2);
    assert_eq!(page.records.len(), 2);
    assert_eq!(page.records[0].position, 1);
    assert_eq!(page.records[0].payload[0], 1);
    drop(source);
    drop(lease);
    storage.shutdown().await.unwrap();
}

#[tokio::test]
async fn sync_pass_reloads_checkpoint_after_lost_commit_reply() {
    let directory = tempfile::tempdir().unwrap();
    let storage = Arc::new(RecordStorage::new(directory.path().join("sessions")).unwrap());
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let (change, snapshot) = opened(&id);
    lease
        .save_changes(
            lease.load().await.unwrap().binding().clone(),
            snapshot,
            vec![SessionSaveUnit::new(vec![change]).unwrap()],
        )
        .await
        .unwrap();
    let source = storage
        .record_source(&id, Id::new("origin").unwrap())
        .await
        .unwrap()
        .unwrap();
    let result = tokio::task::spawn_blocking(move || {
        let mut source = source;
        let scope = source.scope(Id::new("receiver").unwrap(), Id::new("epoch").unwrap());
        let limits = Limits::new(2, 128 * 1024, 128 * 1024).unwrap();
        let mut access = MemoryAuthorizer::allowed(scope.clone());
        let mut replica = MemoryStore::new();
        replica.fail_next_apply(StoreError::Uncertain);
        let mut pass = begin_pass(&scope, &mut access, &mut source, &mut replica).unwrap();
        assert_eq!(
            finish_pass(&mut pass, limits, &mut access, &mut source, &mut replica),
            Err(SyncError::Store(StoreError::Uncertain))
        );
        let mut resumed = begin_pass(&scope, &mut access, &mut source, &mut replica).unwrap();
        assert_eq!(resumed.position(), 2);
        finish_pass(&mut resumed, limits, &mut access, &mut source, &mut replica).unwrap();
        (source, replica.records(&scope).unwrap(), access.checks)
    })
    .await
    .unwrap();
    assert_eq!(result.1.len(), 2);
    assert_eq!(result.1[0].position, 1);
    assert_eq!(result.2, 3);
    drop(result.0);
    drop(lease);
    storage.shutdown().await.unwrap();
}

struct ChildGuard(Child);

impl Drop for ChildGuard {
    fn drop(&mut self) {
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}

struct ServerChild {
    child: ChildGuard,
    output: BufReader<std::process::ChildStdout>,
    address: SocketAddrV4,
    scope: Scope,
}

impl ServerChild {
    fn start(root: &std::path::Path) -> Self {
        let mut child = ChildGuard(
            Command::new(std::env::current_exe().unwrap())
                .arg("--exact")
                .arg("infrastructure::session_storage::child_record_source_server_probe")
                .arg("--nocapture")
                .env("NESSA_RECORD_SOURCE_SERVER_ROOT", root)
                .stdin(Stdio::piped())
                .stdout(Stdio::piped())
                .stderr(Stdio::inherit())
                .spawn()
                .unwrap(),
        );
        let mut output = BufReader::new(child.0.stdout.take().unwrap());
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(
                output.read_line(&mut line).unwrap(),
                0,
                "server exited before ready"
            );
            if let Some(value) = line.trim().strip_prefix("NESSA_RECORD_SOURCE_READY ") {
                let (port, incarnation) = value.split_once(' ').unwrap();
                let address = SocketAddrV4::new(Ipv4Addr::LOCALHOST, port.parse().unwrap());
                let scope = Scope::new(
                    Id::new("receiver").unwrap(),
                    Id::new("origin").unwrap(),
                    Id::new("conversation").unwrap(),
                    Id::new(incarnation).unwrap(),
                    physical_record_schema(),
                    Id::new("epoch").unwrap(),
                );
                return Self {
                    child,
                    output,
                    address,
                    scope,
                };
            }
        }
    }

    fn append_context(&mut self) {
        self.child
            .0
            .stdin
            .as_mut()
            .unwrap()
            .write_all(b"append\n")
            .unwrap();
        self.child.0.stdin.as_mut().unwrap().flush().unwrap();
        let mut line = String::new();
        loop {
            line.clear();
            assert_ne!(self.output.read_line(&mut line).unwrap(), 0);
            if line.contains("NESSA_RECORD_SOURCE_APPENDED") {
                return;
            }
        }
    }
}

#[test]
fn separate_process_receiver_catches_write_between_head_and_recheck() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let mut server = ServerChild::start(&root);
    let mut client = LoopbackClient::new(server.address, Id::new("read-token").unwrap()).unwrap();
    let scope = server.scope.clone();
    let limits = Limits::new(2, 128 * 1024, 128 * 1024).unwrap();
    let mut access = MemoryAuthorizer::allowed(scope.clone());
    let mut replica = MemoryStore::new();

    let mut first = begin_pass(&scope, &mut access, &mut client, &mut replica).unwrap();
    assert_eq!(first.target(), 2);
    server.append_context();
    replica.fail_next_apply(StoreError::Uncertain);
    assert_eq!(
        finish_pass(&mut first, limits, &mut access, &mut client, &mut replica),
        Err(SyncError::Store(StoreError::Uncertain))
    );
    let mut resumed = begin_pass(&scope, &mut access, &mut client, &mut replica).unwrap();
    assert_eq!(resumed.position(), 2);
    assert_eq!(resumed.target(), 4);
    finish_pass(&mut resumed, limits, &mut access, &mut client, &mut replica).unwrap();
    assert_eq!(replica.records(&scope).unwrap().len(), 4);
    let first_counters = client.counters();
    assert!(first_counters.payload_bytes > 0);
    assert!(first_counters.protocol_bytes > 0);
    assert_eq!(first_counters.duplicate_bytes, 0);
    println!(
        "NESSA_RECORD_SOURCE_TRAFFIC payload={} protocol={} duplicate={}",
        first_counters.payload_bytes, first_counters.protocol_bytes, first_counters.duplicate_bytes
    );

    drop(server);
    let restarted = ServerChild::start(&root);
    assert_eq!(restarted.scope, scope);
    let mut client =
        LoopbackClient::new(restarted.address, Id::new("read-token").unwrap()).unwrap();
    let rechecked = begin_pass(&scope, &mut access, &mut client, &mut replica).unwrap();
    assert!(rechecked.is_complete());
    assert_eq!(replica.records(&scope).unwrap().len(), 4);
    let counters = client.counters();
    assert!(counters.protocol_bytes > 0);
}

#[test]
fn child_record_source_server_probe() {
    let Ok(root) = std::env::var("NESSA_RECORD_SOURCE_SERVER_ROOT") else {
        return;
    };
    let runtime = tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()
        .unwrap();
    let storage = Arc::new(RecordStorage::new(root).unwrap());
    let session = SessionId::new("conversation").unwrap();
    let lease = runtime.block_on(storage.open(session.clone())).unwrap();
    let loaded = runtime.block_on(lease.load()).unwrap();
    let (saved, binding) = loaded.into_published(&session).unwrap();
    let (snapshot, next_generation) = match saved {
        Some(snapshot) => (snapshot, binding),
        None => {
            let (change, snapshot) = opened(&session);
            let receipt = runtime
                .block_on(lease.save_changes(
                    binding.clone(),
                    snapshot.clone(),
                    vec![SessionSaveUnit::new(vec![change]).unwrap()],
                ))
                .unwrap();
            (snapshot, receipt.next_for(&binding, 1).unwrap())
        }
    };
    let source = runtime
        .block_on(storage.record_source(&session, Id::new("origin").unwrap()))
        .unwrap()
        .unwrap();
    let scope = source.scope(Id::new("receiver").unwrap(), Id::new("epoch").unwrap());
    let config = LoopbackReadConfig {
        origin: scope.origin().clone(),
        stream: scope.stream().clone(),
        incarnation: scope.incarnation().clone(),
        schema: scope.schema().clone(),
        access_epoch: scope.access_epoch().clone(),
        read_token: Id::new("read-token").unwrap(),
        allowed_receivers: vec![scope.receiver().clone()],
    };
    let server = LoopbackRecordServer::bind(0, config, move || Ok(source.clone())).unwrap();
    let port = server.local_addr().unwrap().port();
    // Libtest's single-threaded runner leaves the child test name on the
    // current line, so begin the protocol marker on a fresh line.
    println!(
        "\nNESSA_RECORD_SOURCE_READY {port} {}",
        scope.incarnation().as_str()
    );
    std::io::stdout().flush().unwrap();
    let control_handle = runtime.handle().clone();
    let control = std::thread::spawn(move || {
        let mut input = String::new();
        std::io::stdin().read_line(&mut input).unwrap();
        if input.trim() == "append" {
            let context = ProviderContext::Recorded(ExecutionSessionId::new("remote").unwrap());
            let change = SessionChange::ProviderContext {
                before: ProviderContext::Absent,
                after: context.clone(),
            };
            let next = SessionSnapshot {
                provider_context: context,
                ..snapshot
            };
            control_handle
                .block_on(lease.save_changes(
                    next_generation,
                    next,
                    vec![SessionSaveUnit::new(vec![change]).unwrap()],
                ))
                .unwrap();
            println!("NESSA_RECORD_SOURCE_APPENDED");
            std::io::stdout().flush().unwrap();
        }
    });
    server.serve().unwrap();
    control.join().unwrap();
}

#[path = "session_storage/bounded_persistence_benchmark.rs"]
mod bounded_persistence_benchmark;
