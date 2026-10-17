//! Normal host cleanup joins the tracked SDK record read owner.
use super::tests::failure;
use super::*;
use crate::conversation::application::{RecordReadLease, RecordReadOperation, RecordReadSource};
use crate::conversation::infrastructure::{NessaRecordReadSource, TestReadGate};
use crate::core::{Outcome, ShutdownStage};
use crate::product::HostProductFixture;
use nessa_sdk::application::agent_execution::sessions::SessionStorage;
use nessa_sync::replication::domain::Id;
use std::sync::atomic::{AtomicBool, Ordering};
use tokio::runtime::Handle;
use tokio::sync::{oneshot, Notify};

struct ReleaseRead(Arc<TestReadGate>);
impl Drop for ReleaseRead {
    fn drop(&mut self) {
        self.0.release();
    }
}

async fn observed(report: &ReportSlot, ready: impl Fn(&ShutdownReport) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if report.lock().unwrap().as_ref().is_some_and(&ready) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

/// A record read held inside its physical work keeps host cleanup at the
/// drains: conversations and MCP stop do not begin until the read is joined,
/// and once it is, the report confirms and the read's capacity is back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_host_joins_a_held_record_read_before_later_cleanup() {
    let fixture = HostProductFixture::new().await;
    let state = fixture.state();
    let storage = fixture.storage();
    let (source, read_work) = NessaRecordReadSource::held_for_host_shutdown(
        storage.clone(),
        Id::new("host-origin").unwrap(),
        Handle::current(),
        false,
    );
    let source = Arc::new(source);
    let _release = ReleaseRead(read_work.clone());
    let lease = RecordReadLease::new(state.record_reads.clone().try_acquire_owned().unwrap());
    let scope = fixture.scope();
    let reading = source.clone();
    let read =
        tokio::spawn(async move { reading.read(scope, RecordReadOperation::Head, lease).await });
    tokio::time::timeout(Duration::from_secs(5), read_work.entered())
        .await
        .unwrap();
    let report = Arc::new(Mutex::new(None));
    let output = report.clone();
    let conversations = Arc::new(AtomicBool::new(false));
    let cleaned = conversations.clone();
    let servers = Arc::new(AtomicBool::new(false));
    let stopped = servers.clone();
    let cleanup = tokio::spawn(async move {
        cleanup_product(
            &output,
            &state,
            async { source.shutdown().await },
            async { Ok(()) },
            Some(async {
                cleaned.store(true, Ordering::SeqCst);
                storage.shutdown().await.map_err(ConversationError::Storage)
            }),
            async {
                stopped.store(true, Ordering::SeqCst);
                Ok(())
            },
            std::future::ready(Ok(())),
            Duration::from_secs(30),
        )
        .await;
    });
    observed(&report, |r| {
        r.stage() == ShutdownStage::Drains
            && !r.readers().record().is_known()
            && r.readers().catalogue().is_ok()
    })
    .await;
    assert!(!conversations.load(Ordering::SeqCst));
    assert!(!servers.load(Ordering::SeqCst));
    read_work.release();
    drop(read.await.unwrap().unwrap());
    tokio::time::timeout(Duration::from_secs(5), cleanup)
        .await
        .unwrap()
        .unwrap();
    assert!(conversations.load(Ordering::SeqCst));
    assert!(servers.load(Ordering::SeqCst));
    assert!(shutdown_result(&report).is_ok());
    assert_eq!(fixture.state().record_reads.available_permits(), 4);
}

/// The host loses its observer of cleanup while a record read is held: the
/// cleanup task goes on, and the read's worker panic, the conversations'
/// failure and the later stages all land in the one report.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_host_retains_a_reader_fault_after_loss_of_its_observer() {
    let fixture = HostProductFixture::new().await;
    let state = fixture.state();
    let storage = fixture.storage();
    let (source, read_work) = NessaRecordReadSource::held_for_host_shutdown(
        storage.clone(),
        Id::new("host-origin").unwrap(),
        Handle::current(),
        true,
    );
    let source = Arc::new(source);
    let _release = ReleaseRead(read_work.clone());
    let scope = fixture.scope();
    let lease = RecordReadLease::new(state.record_reads.clone().try_acquire_owned().unwrap());
    let reading = source.clone();
    let read =
        tokio::spawn(async move { reading.read(scope, RecordReadOperation::Head, lease).await });
    tokio::time::timeout(Duration::from_secs(5), read_work.entered())
        .await
        .unwrap();
    let report = Arc::new(Mutex::new(None));
    let output = report.clone();
    let conversations = Arc::new(AtomicBool::new(false));
    let cleaned = conversations.clone();
    let servers = Arc::new(Notify::new());
    let entered_mcp = servers.clone();
    let (release_mcp, mcp) = oneshot::channel();
    let (returned, terminal) = oneshot::channel();
    let cleanup = tokio::spawn(async move {
        cleanup_product(
            &output,
            &state,
            async { source.shutdown().await },
            async { Ok(()) },
            Some(async {
                cleaned.store(true, Ordering::SeqCst);
                storage.shutdown().await.unwrap();
                Err(ConversationError::Audit)
            }),
            async {
                entered_mcp.notify_one();
                mcp.await.unwrap();
                Ok(())
            },
            std::future::ready(Ok(())),
            Duration::from_secs(30),
        )
        .await;
        let _ = returned.send(());
    });
    observed(&report, |r| {
        r.stage() == ShutdownStage::Drains
            && !r.readers().record().is_known()
            && r.readers().catalogue().is_ok()
    })
    .await;
    drop(cleanup); // Lost host observer detaches, preserving the original cleanup task.
    assert!(!conversations.load(Ordering::SeqCst));
    read_work.release();
    assert!(matches!(
        read.await.unwrap(),
        Err(RecordReadError::WorkerPanicked)
    ));
    tokio::time::timeout(Duration::from_secs(5), servers.notified())
        .await
        .unwrap();
    assert!(report.lock().unwrap().as_ref().is_some_and(|r| {
        r.stage() == ShutdownStage::Servers
            && matches!(r.conversations(), Outcome::Failed(ConversationError::Audit))
            && r.readers().record() == &Outcome::Failed(RecordReadError::WorkerPanicked)
            && r.readers().catalogue().is_ok()
    }));
    release_mcp.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), terminal)
        .await
        .unwrap()
        .unwrap();
    let failure = failure(shutdown_result(&report));
    let report = failure.report();
    assert_eq!(report.stage(), ShutdownStage::Complete);
    assert_eq!(
        report.readers().record(),
        &Outcome::Failed(RecordReadError::WorkerPanicked)
    );
    assert_eq!(report.readers().catalogue(), &Outcome::Ok);
    assert!(!report.readers().deadline_exceeded());
    assert!(matches!(
        report.conversations(),
        Outcome::Failed(ConversationError::Audit)
    ));
    assert!(report.servers().is_ok() && report.native().is_ok());
}
