use super::*;
use crate::application::agent_execution::subagents::OwnershipStore;
use crate::domain::agent_execution::sessions::SessionId;
use crate::domain::agent_execution::subagents::{
    AgentLifetimeId, Initiator, LifetimeState, OwnershipGraph,
};
use std::{future::Future, sync::atomic::Ordering, time::Instant};

fn private_directory() -> tempfile::TempDir {
    let directory = tempfile::tempdir().unwrap();
    nessa_local_storage::create_directory(&directory.path().join("private")).unwrap();
    directory
}

#[test]
fn ownership_write_keeps_current_thread_heartbeat_running() {
    let directory = private_directory();
    let path = directory.path().join("private/ownership.sqlite3");
    let mut store = SqliteOwnershipStore::open(&path).unwrap();
    physical_tests::heartbeat(|gate| async move {
        Arc::get_mut(&mut store.state).unwrap().gate = Some(gate);
        store.write(&OwnershipSnapshot::default()).await.unwrap();
    });
}

#[test]
fn ownership_read_keeps_current_thread_heartbeat_running() {
    let directory = private_directory();
    let path = directory.path().join("private/ownership.sqlite3");
    let mut store = SqliteOwnershipStore::open(&path).unwrap();
    physical_tests::heartbeat(|gate| async move {
        store.write(&OwnershipSnapshot::default()).await.unwrap();
        Arc::get_mut(&mut store.state).unwrap().gate = Some(gate);
        assert!(store.read().await.unwrap().lifetimes.is_empty());
    });
}

#[tokio::test]
async fn a_snapshot_round_trips_and_a_corrupt_body_is_left_unchanged() {
    let directory = private_directory();
    let path = directory.path().join("private").join("ownership.sqlite3");
    let store = SqliteOwnershipStore::open(&path).unwrap();
    assert!(store.read().await.unwrap().lifetimes.is_empty());
    let mut graph = OwnershipGraph::new();
    let _evidence = graph
        .open_root(
            SessionId::new("root-session").unwrap(),
            AgentLifetimeId::new("root-life").unwrap(),
            Initiator::Runtime,
        )
        .unwrap();
    store.write(&graph.snapshot()).await.unwrap();
    drop(store);
    let reopened = SqliteOwnershipStore::open(&path).unwrap();
    let loaded = reopened.read().await.unwrap();
    assert_eq!(loaded.lifetimes[0].state, LifetimeState::Open);
    assert_eq!(loaded.lifetimes[0].lifetime_id.as_str(), "root-life");
    drop(reopened);

    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE ownership_snapshot SET body = 'not-json' WHERE id = 1",
            [],
        )
        .unwrap();
    drop(connection);
    let refused = SqliteOwnershipStore::open(&path).unwrap();
    assert!(matches!(refused.read().await, Err(PortFailure::Rejected)));
    let connection = rusqlite::Connection::open(&path).unwrap();
    let body: String = connection
        .query_row(
            "SELECT body FROM ownership_snapshot WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, "not-json");
}

#[tokio::test]
async fn an_illegal_body_and_a_foreign_schema_version_are_left_unchanged() {
    let directory = private_directory();
    let path = directory.path().join("private").join("ownership.sqlite3");
    let store = SqliteOwnershipStore::open(&path).unwrap();
    let mut graph = OwnershipGraph::new();
    let _evidence = graph
        .open_root(
            SessionId::new("root-session").unwrap(),
            AgentLifetimeId::new("root-life").unwrap(),
            Initiator::Runtime,
        )
        .unwrap();
    store.write(&graph.snapshot()).await.unwrap();
    drop(store);

    let illegal = r#"{"lifetimes":[{"lifetime_id":"root-life","session_id":"root-session","state":"nope","close_operation":null,"cause":null,"initiator":null,"cascaded_from":null}],"spawns":[],"settlements":[],"reports":[]}"#;
    let connection = rusqlite::Connection::open(&path).unwrap();
    connection
        .execute(
            "UPDATE ownership_snapshot SET body = ?1 WHERE id = 1",
            [illegal],
        )
        .unwrap();
    drop(connection);
    let refused = SqliteOwnershipStore::open(&path).unwrap();
    assert!(matches!(refused.read().await, Err(PortFailure::Rejected)));
    let connection = rusqlite::Connection::open(&path).unwrap();
    let body: String = connection
        .query_row(
            "SELECT body FROM ownership_snapshot WHERE id = 1",
            [],
            |row| row.get(0),
        )
        .unwrap();
    assert_eq!(body, illegal);
    connection.pragma_update(None, "user_version", 2).unwrap();
    drop(connection);
    let before = std::fs::read(&path).unwrap();
    let opened = SqliteOwnershipStore::open(&path);
    assert!(matches!(
        opened,
        Err(OpenError::Version {
            found: 2,
            expected: 1
        })
    ));
    assert_eq!(std::fs::read(&path).unwrap(), before);
}
#[tokio::test]
async fn canceled_sqlite_write_excludes_following_io() {
    let directory = private_directory();
    let store = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap(),
    );
    let probe = store.probe.clone();
    let (gate, watchdog) = physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let first = store.clone();
    let caller = tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
    physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let waiting = store.clone();
    let canceled = tokio::spawn(async move { waiting.read().await });
    tokio::task::yield_now().await;
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    let following = store.clone();
    let write = tokio::spawn(async move {
        let mut graph = OwnershipGraph::new();
        let _evidence = graph
            .open_root(
                SessionId::new("next-session").unwrap(),
                AgentLifetimeId::new("next-life").unwrap(),
                Initiator::Runtime,
            )
            .unwrap();
        following.write(&graph.snapshot()).await
    });
    write.await.unwrap().unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    assert_eq!(
        store.read().await.unwrap().lifetimes[0]
            .lifetime_id
            .as_str(),
        "next-life"
    );
    assert_eq!(probe.submitted.load(std::sync::atomic::Ordering::SeqCst), 3);
}

#[tokio::test]
async fn worker_input_drop_keeps_slot() {
    let directory = private_directory();
    let store = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap(),
    );
    let probe = store.probe.clone();
    let (gate, watchdog) = physical_tests::hold(probe.clone());
    *probe.input_drop.lock().unwrap() = Some(gate);
    let first = store.clone();
    let caller = tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
    physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    let _ = caller.await;
    store.read().await.unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
}

#[test]
fn queued_canceled_worker_retains_slot() {
    let directory = private_directory();
    let store = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap(),
    );
    let probe = store.probe.clone();
    let (pool_gate, watchdog) = physical_tests::hold(probe.clone());
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .max_blocking_threads(1)
        .build()
        .unwrap();
    runtime.block_on(async {
        let blocker = tokio::task::spawn_blocking(move || pool_gate.enter());
        let first = store.clone();
        let caller = tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
        physical_tests::reached(&probe.submitted, 1).await;
        caller.abort();
        let _ = caller.await;
        let following = store.clone();
        let next = tokio::spawn(async move { following.read().await });
        next.await.unwrap().unwrap();
        blocker.await.unwrap();
    });
    assert_eq!(watchdog.join().unwrap(), (1, 0, 0));
    assert_eq!(probe.cleaned.load(std::sync::atomic::Ordering::SeqCst), 2);
}

#[tokio::test]
async fn storage_read_decode_and_poison_are_typed() {
    let directory = private_directory();
    let store =
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap();
    let state = store.state.clone();
    assert!(std::thread::spawn(move || {
        let _guard = state.connection.lock().unwrap();
        panic!("poison actual connection");
    })
    .join()
    .is_err());
    assert!(matches!(store.read().await, Err(PortFailure::Uncertain)));
    assert!(matches!(
        store.write(&OwnershipSnapshot::default()).await,
        Err(PortFailure::Uncertain)
    ));
}

#[tokio::test]
async fn pre_effect_worker_panic_is_typed_and_landed_effect_is_uncertain() {
    let directory = private_directory();
    let store =
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap();
    store.probe.panic_before.store(true, Ordering::SeqCst);
    assert!(matches!(store.read().await, Err(PortFailure::Uncertain)));
    store.probe.panic_after.store(true, Ordering::SeqCst);
    assert!(matches!(
        store.write(&OwnershipSnapshot::default()).await,
        Err(PortFailure::Uncertain)
    ));
    assert!(store.read().await.unwrap().lifetimes.is_empty());
    let connection = store.state.connection.lock().unwrap();
    let count: i64 = connection
        .query_row("SELECT count(*) FROM ownership_snapshot", [], |row| {
            row.get(0)
        })
        .unwrap();
    assert_eq!(count, 1, "uncertain publication physically landed");
}

#[test]
fn stopped_executor_capture_cleanup_keeps_slot() {
    let directory = private_directory();
    let store = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap(),
    );
    let probe = store.probe.clone();
    let (admission_gate, admission_watchdog) = physical_tests::hold(probe.clone());
    let (input_gate, input_watchdog) = physical_tests::hold(probe.clone());
    *probe.input_drop.lock().unwrap() = Some(input_gate);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap();
    let held = runtime.block_on(store.worker.admit()).unwrap();
    let first = store.clone();
    let mut future = Box::pin(async move { first.write(&OwnershipSnapshot::default()).await });
    runtime.block_on(std::future::poll_fn(|cx| {
        assert!(future.as_mut().poll(cx).is_pending());
        std::task::Poll::Ready(())
    }));
    drop(runtime);
    let release = std::thread::spawn(move || {
        admission_gate.enter();
        drop(held);
    });
    let joined = std::thread::spawn(move || physical_tests::plain_thread(future));
    tokio::runtime::Builder::new_current_thread()
        .build()
        .unwrap()
        .block_on(async {
            assert!(store.read().await.unwrap().lifetimes.is_empty());
        });
    assert!(matches!(
        joined.join().unwrap(),
        Err(PortFailure::Uncertain)
    ));
    release.join().unwrap();
    assert_eq!(admission_watchdog.join().unwrap(), (0, 0, 0));
    assert_eq!(input_watchdog.join().unwrap(), (1, 0, 0));
    assert_eq!(probe.started.load(Ordering::SeqCst), 1);
    assert_eq!(probe.cleaned.load(Ordering::SeqCst), 2);
}

#[test]
fn panicking_payload_destructor_is_not_run_by_join_mapping() {
    const CHILD: &str = "NESSA_627_PAYLOAD_CHILD";
    if std::env::var_os(CHILD).is_some() {
        let directory = private_directory();
        let store = SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3"))
            .unwrap();
        store
            .probe
            .payload_before
            .store(true, std::sync::atomic::Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                assert!(matches!(store.read().await, Err(PortFailure::Uncertain)));
                assert!(store.read().await.unwrap().lifetimes.is_empty());
            });
        return;
    }
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", "infrastructure::session_storage::ownership::tests::panicking_payload_destructor_is_not_run_by_join_mapping"])
        .env(CHILD, "1").stdout(std::process::Stdio::null()).stderr(std::process::Stdio::null()).spawn().unwrap();
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(5) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("payload subprocess watchdog expired");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn no_runtime_first_poll_is_typed() {
    let directory = private_directory();
    let store =
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap();
    assert!(matches!(
        physical_tests::plain_thread(store.read()),
        Err(PortFailure::Rejected)
    ));
    assert!(matches!(
        physical_tests::plain_thread(store.write(&OwnershipSnapshot::default())),
        Err(PortFailure::Rejected)
    ));
    assert_eq!(
        store
            .probe
            .submitted
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

#[test]
fn queued_send_future_resumes_on_plain_thread() {
    let directory = private_directory();
    let store = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap(),
    );
    let probe = store.probe.clone();
    let (gate, watchdog) = physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let first = store.clone();
    let entered_runtime = runtime.enter();
    let held = tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
    drop(entered_runtime);
    runtime.block_on(physical_tests::reached(&probe.started, 1));
    let later = store.clone();
    let mut future = Box::pin(async move { later.read().await });
    runtime.block_on(std::future::poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    }));
    let result = std::thread::spawn(move || physical_tests::plain_thread(future))
        .join()
        .unwrap();
    assert!(result.unwrap().lifetimes.is_empty());
    runtime.block_on(held).unwrap().unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
}

#[test]
fn originating_runtime_shutdown_is_typed() {
    let directory = private_directory();
    let store = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap(),
    );
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let held = runtime
        .block_on(store.worker.admit())
        .expect("test held physical admission");
    let later = store.clone();
    let mut future = Box::pin(async move { later.read().await });
    runtime.block_on(std::future::poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    }));
    drop(runtime);
    drop(held);
    assert!(matches!(
        physical_tests::plain_thread(future),
        Err(PortFailure::Uncertain)
    ));
    assert_eq!(
        store
            .probe
            .started
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
}

fn isolated_fault_case(name: &str, body: impl FnOnce()) {
    const CHILD: &str = "NESSA_627_FAULT_CASE";
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        body();
        return;
    }
    let exact = format!("infrastructure::session_storage::ownership::tests::{name}");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &exact])
        .env(CHILD, name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "isolated physical fault failed: {status}");
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(5) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("physical fault subprocess watchdog expired");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn detached_caller_faulting_payload_is_contained() {
    isolated_fault_case("detached_caller_faulting_payload_is_contained", || {
        let directory = private_directory();
        let store = Arc::new(
            SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3"))
                .unwrap(),
        );
        let probe = store.probe.clone();
        let (gate, watchdog) = physical_tests::hold(probe.clone());
        *probe.before.lock().unwrap() = Some(gate);
        probe.payload_before.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let first = store.clone();
                let caller =
                    tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
                physical_tests::reached(&probe.started, 1).await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                assert!(store.read().await.unwrap().lifetimes.is_empty());
            });
        assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
        assert_eq!(
            probe.payload_dropped.load(Ordering::SeqCst),
            0,
            "faulting physical payload was disposed after caller disappeared"
        );
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 2);
    });
}

#[test]
fn stopped_executor_input_drop_fault_keeps_slot() {
    isolated_fault_case("stopped_executor_input_drop_fault_keeps_slot", || {
        let directory = private_directory();
        let adapter = Arc::new(
            SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3"))
                .unwrap(),
        );
        let probe = adapter.probe.clone();
        let (admission_gate, admission_watchdog) = physical_tests::hold(probe.clone());
        let (input_gate, input_watchdog) = physical_tests::hold(probe.clone());
        *probe.input_drop.lock().unwrap() = Some(input_gate);
        probe.panic_input_drop.store(true, Ordering::SeqCst);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap();
        let held = runtime.block_on(adapter.worker.admit()).unwrap();
        let first = adapter.clone();
        let mut future = Box::pin(async move { first.write(&OwnershipSnapshot::default()).await });
        runtime.block_on(std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        }));
        drop(runtime); // Before submission; already queued blocking jobs would run.
        let release = std::thread::spawn(move || {
            admission_gate.enter();
            drop(held);
        });
        let joined = std::thread::spawn(move || physical_tests::plain_thread(future));
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                assert!(adapter.read().await.unwrap().lifetimes.is_empty());
            });
        assert!(matches!(
            joined.join().unwrap(),
            Err(PortFailure::Uncertain)
        ));
        release.join().unwrap();
        assert_eq!(admission_watchdog.join().unwrap(), (0, 0, 0));
        assert_eq!(input_watchdog.join().unwrap(), (1, 0, 0));
        assert_eq!(probe.started.load(Ordering::SeqCst), 1);
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 2);
        assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
    });
}

#[test]
fn ready_effect_input_drop_fault_is_uncertain() {
    isolated_fault_case("ready_effect_input_drop_fault_is_uncertain", || {
        let directory = private_directory();
        let store = Arc::new(
            SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3"))
                .unwrap(),
        );
        let probe = store.probe.clone();
        let (gate, watchdog) = physical_tests::hold(probe.clone());
        *probe.input_drop.lock().unwrap() = Some(gate);
        probe.panic_input_drop.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let first = store.clone();
                let caller =
                    tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
                physical_tests::reached(&probe.started, 1).await;
                let later = store.clone();
                let next = tokio::spawn(async move { later.read().await });
                assert!(matches!(caller.await.unwrap(), Err(PortFailure::Uncertain)));
                assert!(next.await.unwrap().unwrap().lifetimes.is_empty());
                probe.panic_input_drop.store(true, Ordering::SeqCst);
                assert!(matches!(store.read().await, Err(PortFailure::Uncertain)));
            });
        assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
        let connection = store.state.connection.lock().unwrap();
        let count: i64 = connection
            .query_row("SELECT count(*) FROM ownership_snapshot", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(count, 1);
    });
}

#[test]
fn operation_panic_and_capture_drop_fault_are_separate_boundaries() {
    isolated_fault_case(
        "operation_panic_and_capture_drop_fault_are_separate_boundaries",
        || {
            let directory = private_directory();
            let store =
                SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3"))
                    .unwrap();
            store.probe.panic_after.store(true, Ordering::SeqCst);
            store.probe.panic_input_drop.store(true, Ordering::SeqCst);
            tokio::runtime::Builder::new_current_thread()
                .enable_time()
                .build()
                .unwrap()
                .block_on(async {
                    assert!(matches!(
                        store.write(&OwnershipSnapshot::default()).await,
                        Err(PortFailure::Uncertain)
                    ));
                    assert!(store.read().await.unwrap().lifetimes.is_empty());
                });
        },
    );
}

#[tokio::test]
async fn admission_precedes_owned_input_transfer() {
    let directory = private_directory();
    let store = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3")).unwrap(),
    );
    let probe = store.probe.clone();
    let (gate, watchdog) = physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let first = store.clone();
    let held = tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
    physical_tests::reached(&probe.started, 1).await;
    let mut graph = OwnershipGraph::new();
    for index in 0..32 {
        let _evidence = graph
            .open_root(
                SessionId::new(format!("session-{index}")).unwrap(),
                AgentLifetimeId::new(format!("life-{index}")).unwrap(),
                Initiator::Runtime,
            )
            .unwrap();
    }
    store.write(&graph.snapshot()).await.unwrap();
    held.await.unwrap().unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    assert_eq!(probe.inputs_at_release.load(Ordering::SeqCst), 1);
    assert_eq!(store.read().await.unwrap().lifetimes.len(), 32);
}

#[tokio::test]
async fn independent_instances_progress() {
    let directory = private_directory();
    let first = Arc::new(
        SqliteOwnershipStore::open(&directory.path().join("private/first.sqlite3")).unwrap(),
    );
    let second =
        SqliteOwnershipStore::open(&directory.path().join("private/second.sqlite3")).unwrap();
    let probe = first.probe.clone();
    let (gate, watchdog) = physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let worker = first.clone();
    let held = tokio::spawn(async move { worker.write(&OwnershipSnapshot::default()).await });
    physical_tests::reached(&probe.started, 1).await;
    second.write(&OwnershipSnapshot::default()).await.unwrap();
    assert_eq!(probe.cleaned.load(Ordering::SeqCst), 0);
    held.await.unwrap().unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
}

#[test]
fn detached_ready_input_drop_fault_is_contained() {
    isolated_fault_case("detached_ready_input_drop_fault_is_contained", || {
        let directory = private_directory();
        let store = Arc::new(
            SqliteOwnershipStore::open(&directory.path().join("private/ownership.sqlite3"))
                .unwrap(),
        );
        let probe = store.probe.clone();
        let (gate, watchdog) = physical_tests::hold(probe.clone());
        *probe.input_drop.lock().unwrap() = Some(gate);
        probe.panic_input_drop.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let first = store.clone();
                let caller =
                    tokio::spawn(async move { first.write(&OwnershipSnapshot::default()).await });
                physical_tests::reached(&probe.started, 1).await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                store.read().await.unwrap();
            });
        assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
        let count: i64 = store
            .state
            .connection
            .lock()
            .unwrap()
            .query_row("SELECT count(*) FROM ownership_snapshot", [], |row| {
                row.get(0)
            })
            .unwrap();
        assert_eq!(
            count, 1,
            "ready physical write landed before captured input fault"
        );
        assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 2);
    });
}
