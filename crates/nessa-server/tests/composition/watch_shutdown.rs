//! Normal host consumes the original watch and tracked SDK read owners.
use super::*;
use crate::conversation::application::{RecordReadLease, RecordReadOperation, RecordReadSource};
use crate::conversation::infrastructure::{NessaRecordReadSource, TestReadGate};
use crate::product::HostWatchFixture;
use futures_util::poll;
use nessa_sdk::application::agent_execution::sessions::SessionStorage;
use nessa_sync::replication::domain::Id;
use std::sync::atomic::{AtomicBool, Ordering};
use std::task::Poll;
use tokio::runtime::Handle;
use tokio::sync::{oneshot, Notify};

struct ReleaseRead(Arc<TestReadGate>);
impl Drop for ReleaseRead {
    fn drop(&mut self) {
        self.0.release();
    }
}
async fn observed(report: &Mutex<ShutdownReport>, ready: impl Fn(&ShutdownReport) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if ready(&report.lock().unwrap()) {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_host_closes_admission_before_cleanup_and_reaps_both_original_release_orders() {
    for watch_first in [false, true] {
        let mut fixture = HostWatchFixture::held_authority(false).await;
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
            tokio::spawn(
                async move { reading.read(scope, RecordReadOperation::Head, lease).await },
            );
        tokio::time::timeout(Duration::from_secs(5), read_work.entered())
            .await
            .unwrap();
        let report = Arc::new(Mutex::new(ShutdownReport::Unreported));
        let output = report.clone();
        let closed_before_read = Arc::new(AtomicBool::new(false));
        let checked = closed_before_read.clone();
        let read_state = state.clone();
        let conversations = Arc::new(AtomicBool::new(false));
        let cleaned = conversations.clone();
        let servers = Arc::new(AtomicBool::new(false));
        let stopped = servers.clone();
        let cleanup = tokio::spawn(async move {
            cleanup_product(
                &output,
                &state,
                async {
                    checked.store(
                        HostWatchFixture::admission_closed(&read_state),
                        Ordering::SeqCst,
                    );
                    source.shutdown().await
                },
                async { Ok(()) },
                Some(async {
                    cleaned.store(true, Ordering::SeqCst);
                    storage.shutdown().await.map_err(ConversationError::Storage)
                }),
                async {
                    stopped.store(true, Ordering::SeqCst);
                },
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            )
            .await;
        });
        observed(&report, |r| matches!(r, ShutdownReport::DrainsPending {readers, watches} if watches.result().is_none() && readers.catalogue() == Some(&Ok(())))).await;
        assert!(closed_before_read.load(Ordering::SeqCst));
        assert!(HostWatchFixture::admission_closed(&fixture.state()));
        fixture.connection_stopped().await; // Peer is still retained; host close caused this exit.
        assert!(fixture.resource_held());
        assert_eq!(fixture.completed_authority(), 0);
        assert!(!conversations.load(Ordering::SeqCst));
        assert!(!servers.load(Ordering::SeqCst));
        if watch_first {
            fixture.release();
            observed(&report, |r| matches!(r, ShutdownReport::DrainsPending {readers, watches} if watches.result() == Some(&Ok(())) && readers.record().is_none())).await;
            assert!(!conversations.load(Ordering::SeqCst));
            assert!(!servers.load(Ordering::SeqCst));
            read_work.release();
        } else {
            read_work.release();
            observed(&report, |r| matches!(r, ShutdownReport::DrainsPending {readers, watches} if watches.result().is_none() && readers.record() == Some(&Ok(())))).await;
            assert!(!conversations.load(Ordering::SeqCst));
            assert!(!servers.load(Ordering::SeqCst));
            assert!(fixture.resource_held());
            fixture.release();
        }
        drop(read.await.unwrap().unwrap());
        tokio::time::timeout(Duration::from_secs(5), cleanup)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(fixture.completed_authority(), 1);
        assert!(conversations.load(Ordering::SeqCst));
        assert!(servers.load(Ordering::SeqCst));
        assert!(shutdown_result(&report).is_ok());
        assert_eq!(fixture.state().record_reads.available_permits(), 4);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ordinary_host_retains_watch_and_reader_faults_after_loss_of_both_observers() {
    for fault in [
        WatchTaskFault::Panic,
        WatchTaskFault::UnexpectedCancellation,
    ] {
        let mut fixture = match fault {
            WatchTaskFault::Panic => HostWatchFixture::held_authority(true).await,
            WatchTaskFault::UnexpectedCancellation => HostWatchFixture::cancelled_task().await,
        };
        fixture.lose_socket_observer().await;
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
            tokio::spawn(
                async move { reading.read(scope, RecordReadOperation::Head, lease).await },
            );
        tokio::time::timeout(Duration::from_secs(5), read_work.entered())
            .await
            .unwrap();
        let report = Arc::new(Mutex::new(ShutdownReport::Unreported));
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
                },
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            )
            .await;
            let _ = returned.send(());
        });
        observed(&report, |r| matches!(r, ShutdownReport::DrainsPending {readers, watches} if watches.result().is_none() && readers.catalogue() == Some(&Ok(())))).await;
        assert!(fixture.resource_held());
        drop(cleanup); // Lost host observer detaches, preserving the original cleanup task.
        fixture.release();
        observed(&report, |r| matches!(r, ShutdownReport::DrainsPending {readers, watches} if watches.result() == Some(&Err(fault)) && readers.record().is_none())).await;
        assert!(!conversations.load(Ordering::SeqCst));
        read_work.release();
        assert!(matches!(
            read.await.unwrap(),
            Err(RecordReadError::WorkerPanicked)
        ));
        tokio::time::timeout(Duration::from_secs(5), servers.notified())
            .await
            .unwrap();
        assert!(
            matches!(&*report.lock().unwrap(), ShutdownReport::ServersPending {readers: Err(readers), watches: Err(actual), conversations: Err(ConversationError::Audit)} if actual.fault() == Some(fault) && readers.outcomes().record() == Some(&Err(RecordReadError::WorkerPanicked)))
        );
        release_mcp.send(()).unwrap();
        tokio::time::timeout(Duration::from_secs(5), terminal)
            .await
            .unwrap()
            .unwrap();
        let Err(RunError::Shutdown(Some(ShutdownFailure::Watches {
            watches,
            readers: Err(readers),
            conversations: Err(ConversationError::Audit),
        }))) = shutdown_result(&report)
        else {
            panic!("retain all original independent causes");
        };
        assert_eq!(watches.fault(), Some(fault));
        assert!(!watches.outcome().deadline_exceeded());
        assert_eq!(
            readers.outcomes().record(),
            Some(&Err(RecordReadError::WorkerPanicked))
        );
        assert_eq!(readers.outcomes().catalogue(), Some(&Ok(())));
        assert_eq!(fixture.state().drain_watches().await, Err(fault));
    }
}

#[tokio::test]
async fn completed_reader_drain_is_not_relabelled_as_timeout_while_original_watch_is_held() {
    let mut fixture = HostWatchFixture::held_authority(false).await;
    fixture.lose_socket_observer().await;
    let storage = fixture.storage();
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("host-origin").unwrap(),
        Handle::current(),
    );
    let lease = RecordReadLease::new(
        fixture
            .state()
            .record_reads
            .clone()
            .try_acquire_owned()
            .unwrap(),
    );
    drop(
        source
            .read(fixture.scope(), RecordReadOperation::Head, lease)
            .await
            .unwrap(),
    );
    let actual_reader_outcome = source.shutdown().await;
    let state = fixture.state();
    let report = Mutex::new(ShutdownReport::Unreported);
    let cleanup = cleanup_product(
        &report,
        &state,
        std::future::ready(actual_reader_outcome),
        async { Ok(()) },
        Some(async { storage.shutdown().await.map_err(ConversationError::Storage) }),
        async {},
        std::future::ready(Ok(())),
        Duration::ZERO,
    );
    tokio::pin!(cleanup);
    assert!(matches!(poll!(&mut cleanup), Poll::Pending));
    tokio::time::pause();
    tokio::time::advance(Duration::from_secs(1)).await;
    assert!(matches!(poll!(&mut cleanup), Poll::Pending));
    // Row H5: the deadline stays armed for the held watch drain after both
    // readers finished. It is recorded against the watch only; cleanup still
    // waits for the original watch task.
    assert!(
        matches!(&*report.lock().unwrap(), ShutdownReport::DrainsPending {readers, watches} if readers.complete() && !readers.deadline_exceeded() && watches.result().is_none() && watches.deadline_exceeded())
    );
    tokio::time::resume();
    fixture.release();
    tokio::time::timeout(Duration::from_secs(5), cleanup)
        .await
        .unwrap();
    let Err(RunError::Shutdown(Some(ShutdownFailure::Watches {
        watches,
        readers: Ok(()),
        conversations: Ok(()),
    }))) = shutdown_result(&report)
    else {
        panic!("a watch drain held past the deadline is not a confirmed shutdown");
    };
    assert!(watches.outcome().deadline_exceeded());
    assert_eq!(watches.fault(), None);
}

#[tokio::test]
async fn ended_mcp_cleanup_preserves_returned_original_watch_fault_and_reader_outcomes() {
    let mut fixture = HostWatchFixture::cancelled_task().await;
    let state = fixture.state();
    let storage = fixture.storage();
    let source = NessaRecordReadSource::new(
        storage.clone(),
        Id::new("host-origin").unwrap(),
        Handle::current(),
    );
    let lease = RecordReadLease::new(state.record_reads.clone().try_acquire_owned().unwrap());
    drop(
        source
            .read(fixture.scope(), RecordReadOperation::Head, lease)
            .await
            .unwrap(),
    );
    let reader_result = source.shutdown().await;
    fixture.release();
    assert_eq!(
        state.drain_watches().await,
        Err(WatchTaskFault::UnexpectedCancellation)
    );
    let report = Mutex::new(ShutdownReport::Unreported);
    {
        let cleanup = cleanup_product(
            &report,
            &state,
            std::future::ready(reader_result),
            async { Ok(()) },
            Some(async { storage.shutdown().await.map_err(ConversationError::Storage) }),
            std::future::pending(),
            std::future::ready(Ok(())),
            Duration::from_secs(30),
        );
        tokio::pin!(cleanup);
        // Poll actual cleanup while waiting for its same retained MCP phase.
        tokio::select! {
            () = &mut cleanup => panic!("MCP remains unreported"),
            () = observed(&report, |r| matches!(r, ShutdownReport::ServersPending {..})) => {},
        }
    }
    assert!(matches!(
        shutdown_result(&report),
        Err(RunError::Shutdown(Some(
            ShutdownFailure::ServersUnreported {
                watches: Err(watches),
                readers: Ok(()),
                conversations: Ok(())
            }
        ))) if watches.fault() == Some(WatchTaskFault::UnexpectedCancellation)
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ended_original_cleanup_retains_returned_watch_fault_at_each_earlier_stage() {
    for during_conversations in [false, true] {
        let mut fixture = HostWatchFixture::held_authority(true).await;
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
        let mut read = Some(tokio::spawn(async move {
            reading.read(scope, RecordReadOperation::Head, lease).await
        }));
        tokio::time::timeout(Duration::from_secs(5), read_work.entered())
            .await
            .unwrap();
        let report = Mutex::new(ShutdownReport::Unreported);
        let conversations_started = AtomicBool::new(false);
        let servers_started = AtomicBool::new(false);
        let (_release_conversations, held_conversations) = oneshot::channel::<()>();
        {
            let cleanup = cleanup_product(
                &report,
                &state,
                async { source.shutdown().await },
                async { Ok(()) },
                Some(async {
                    conversations_started.store(true, Ordering::SeqCst);
                    held_conversations.await.unwrap();
                    storage.shutdown().await.map_err(ConversationError::Storage)
                }),
                async { servers_started.store(true, Ordering::SeqCst) },
                std::future::ready(Ok(())),
                Duration::from_secs(30),
            );
            tokio::pin!(cleanup);
            assert!(matches!(poll!(&mut cleanup), Poll::Pending));
            fixture.connection_stopped().await;
            assert!(fixture.resource_held());
            fixture.release();
            tokio::select! {
                () = &mut cleanup => panic!("original reader work remains held"),
                () = observed(&report, |r| matches!(r, ShutdownReport::DrainsPending {
                    readers, watches
                } if watches.result() == Some(&Err(WatchTaskFault::Panic)) && readers.record().is_none() && readers.catalogue() == Some(&Ok(())))) => {},
            }
            assert_eq!(fixture.completed_authority(), 1);
            assert!(!conversations_started.load(Ordering::SeqCst));
            assert_eq!(state.record_reads.available_permits(), 3);
            if during_conversations {
                read_work.release();
                tokio::select! {
                    () = &mut cleanup => panic!("original conversation cleanup remains unknown"),
                    () = observed(&report, |r| matches!(r, ShutdownReport::ConversationsPending {
                        readers: Ok(()), watches: Err(watches)
                    } if watches.fault() == Some(WatchTaskFault::Panic))) => {},
                }
                drop(read.take().unwrap().await.unwrap().unwrap());
                assert!(conversations_started.load(Ordering::SeqCst));
            }
            // Inject the original cleanup future ending, rather than losing its
            // JoinHandle observer (the separate host-owner test covers that case).
        }
        assert!(!servers_started.load(Ordering::SeqCst));
        if during_conversations {
            assert!(matches!(
                shutdown_result(&report),
                Err(RunError::Shutdown(Some(
                    ShutdownFailure::ConversationsUnreported {
                        readers: Ok(()),
                        watches: Err(watches),
                    }
                ))) if watches.fault() == Some(WatchTaskFault::Panic)
            ));
        } else {
            let Err(RunError::Shutdown(Some(ShutdownFailure::DrainsUnreported {
                outcomes,
                watches,
            }))) = shutdown_result(&report)
            else {
                panic!("retain returned watch fault with original unknown reader outcome");
            };
            assert_eq!(watches.result(), Some(&Err(WatchTaskFault::Panic)));
            assert_eq!(outcomes.record(), None);
            assert_eq!(outcomes.catalogue(), Some(&Ok(())));
            assert!(!outcomes.deadline_exceeded());
            assert!(!conversations_started.load(Ordering::SeqCst));
            assert!(!read.as_ref().unwrap().is_finished());
            assert_eq!(state.record_reads.available_permits(), 3);
        }
        // Ending the cleanup wait did not join or release the physical reader.
        // Finish that same owner and its storage explicitly after inspecting evidence.
        read_work.release();
        if let Some(read) = read {
            drop(read.await.unwrap().unwrap());
        }
        source.shutdown().await.unwrap();
        storage.shutdown().await.unwrap();
        assert_eq!(state.record_reads.available_permits(), 4);
        assert_eq!(state.drain_watches().await, Err(WatchTaskFault::Panic));
    }
}
