#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
use std::{
    future::Future,
    io::{Read, Seek, SeekFrom, Write},
    sync::{atomic::Ordering, Arc},
};

use uuid::Uuid;

use super::FileRecords;
use crate::mcp_authorization::application::{AuthorizationRecords, RecordFailure, TokenMaterial};
use crate::mcp_authorization::domain::{Deletion, Publication, ServerAuth};

fn secret() -> TokenMaterial {
    TokenMaterial {
        access_token: "sekret-token".into(),
        refresh_token: Some("refresh-sekret".into()),
        generation: 3,
    }
}

fn records() -> (FileRecords, std::path::PathBuf) {
    let directory = std::env::temp_dir().join(format!("nessa-mcp-auth-{}", Uuid::new_v4()));
    (FileRecords::new(&directory), directory)
}

fn contains_token(directory: &std::path::Path, token: &str) -> bool {
    let mut pending = vec![directory.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(entries) = std::fs::read_dir(&path) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                pending.push(path);
                continue;
            }
            let Ok(bytes) = std::fs::read(&path) else {
                continue;
            };
            if bytes
                .windows(token.len())
                .any(|window| window == token.as_bytes())
            {
                return true;
            }
        }
    }
    false
}

#[test]
fn record_store_keeps_current_thread_heartbeat_running() {
    let (mut records, directory) = records();
    super::super::physical_tests::heartbeat(|gate| async move {
        Arc::get_mut(&mut records.state).unwrap().gate = Some(gate);
        let server = Uuid::new_v4();
        records
            .store(&ServerAuth::consent_needed(
                server,
                "docs",
                "https://mcp.example/mcp",
            ))
            .await
            .unwrap();
        Arc::get_mut(&mut records.state).unwrap().gate = None;
        assert!(records.load(server).await.unwrap().is_some());
    });
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn secret_store_keeps_current_thread_heartbeat_running() {
    let (mut records, directory) = records();
    super::super::physical_tests::heartbeat(|gate| async move {
        Arc::get_mut(&mut records.state).unwrap().gate = Some(gate);
        let server = Uuid::new_v4();
        assert_eq!(
            records.store_secret(server, &secret()).await,
            Ok(Publication::Acknowledged)
        );
        Arc::get_mut(&mut records.state).unwrap().gate = None;
        assert!(records.load_secret(server).await.unwrap() == Some(secret()));
    });
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn a_sealed_file_round_trips_and_never_stores_the_token_in_the_clear() {
    let (records, directory) = records();
    assert!(records.writer_available());
    let server = Uuid::new_v4();
    let secret = secret();
    assert_eq!(
        records.store_secret(server, &secret).await,
        Ok(Publication::Acknowledged)
    );
    let loaded = records.load_secret(server).await.unwrap().unwrap();
    assert_eq!(loaded.access_token, secret.access_token);
    assert_eq!(loaded.refresh_token, secret.refresh_token);
    assert_eq!(loaded.generation, secret.generation);
    assert!(!contains_token(&directory, "sekret-token"));
    assert!(!contains_token(&directory, "refresh-sekret"));
    #[cfg(unix)]
    {
        for name in ["key", &format!("{server}.seal")] {
            let mode = std::fs::metadata(directory.join("secrets").join(name))
                .unwrap()
                .permissions()
                .mode();
            assert_eq!(mode & 0o077, 0, "{name} is not user-only: {mode:o}");
        }
    }
    let facts = ServerAuth::consent_needed(server, "docs", "https://mcp.example/mcp");
    records.store(&facts).await.unwrap();
    let record_path = directory.join(format!("{server}.json"));
    let record = std::fs::read(&record_path).unwrap();
    assert!(!record
        .windows(b"sekret-token".len())
        .any(|window| window == b"sekret-token"));
    #[cfg(unix)]
    {
        let mode = std::fs::metadata(&record_path)
            .unwrap()
            .permissions()
            .mode();
        assert_eq!(mode & 0o077, 0, "the record is not user-only: {mode:o}");
    }
    assert_eq!(records.delete_secret(server).await, Ok(Deletion::Deleted));
    assert_eq!(records.load_secret(server).await, Ok(None));
    let _ = std::fs::remove_dir_all(&directory);
}

#[tokio::test]
async fn a_tampered_seal_is_unavailable_and_not_a_token() {
    let (records, directory) = records();
    let server = Uuid::new_v4();
    records.store_secret(server, &secret()).await.unwrap();
    let path = directory.join("secrets").join(format!("{server}.seal"));
    let mut file =
        nessa_local_storage::open(&path, nessa_local_storage::OpenMode::ReadWrite).unwrap();
    let mut byte = [0_u8; 1];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0xff;
    file.seek(SeekFrom::Start(0)).unwrap();
    file.write_all(&byte).unwrap();
    assert_eq!(
        records.load_secret(server).await,
        Err(RecordFailure::Unavailable)
    );
    let _ = std::fs::remove_dir_all(&directory);
}
#[test]
fn record_load_keeps_current_thread_heartbeat_running() {
    let (mut records, directory) = records();
    super::super::physical_tests::heartbeat(|gate| async move {
        let server = Uuid::new_v4();
        records
            .store(&ServerAuth::consent_needed(
                server,
                "docs",
                "https://mcp.example/mcp",
            ))
            .await
            .unwrap();
        Arc::get_mut(&mut records.state).unwrap().gate = Some(gate);
        assert!(records.load(server).await.unwrap().is_some());
    });
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn secret_load_keeps_current_thread_heartbeat_running() {
    let (mut records, directory) = records();
    super::super::physical_tests::heartbeat(|gate| async move {
        let server = Uuid::new_v4();
        records.store_secret(server, &secret()).await.unwrap();
        Arc::get_mut(&mut records.state).unwrap().gate = Some(gate);
        assert!(records.load_secret(server).await.unwrap() == Some(secret()));
    });
    let _ = std::fs::remove_dir_all(directory);
}

#[test]
fn secret_delete_keeps_current_thread_heartbeat_running() {
    let (mut records, directory) = records();
    super::super::physical_tests::heartbeat(|gate| async move {
        let server = Uuid::new_v4();
        records.store_secret(server, &secret()).await.unwrap();
        Arc::get_mut(&mut records.state).unwrap().gate = Some(gate);
        assert_eq!(records.delete_secret(server).await, Ok(Deletion::Deleted));
    });
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn canceled_secret_store_excludes_delete_and_load() {
    let (records, directory) = records();
    let records = Arc::new(records);
    let server = Uuid::new_v4();
    let probe = records.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let first = records.clone();
    let caller = tokio::spawn(async move { first.store_secret(server, &secret()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    assert!(caller.await.unwrap_err().is_cancelled());
    let waiting = records.clone();
    let canceled = tokio::spawn(async move { waiting.load_secret(server).await });
    tokio::task::yield_now().await;
    canceled.abort();
    assert!(canceled.await.unwrap_err().is_cancelled());
    let later = records.clone();
    let deletion = tokio::spawn(async move { later.delete_secret(server).await });
    let recovery = records.clone();
    let load = tokio::spawn(async move { recovery.load_secret(server).await });
    assert_eq!(deletion.await.unwrap(), Ok(Deletion::Deleted));
    assert!(load.await.unwrap().unwrap().is_none());
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    assert!(records.load_secret(server).await.unwrap().is_none());
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn canceled_delete_excludes_replacement() {
    let (records, directory) = records();
    let records = Arc::new(records);
    let server = Uuid::new_v4();
    records.store_secret(server, &secret()).await.unwrap();
    let probe = records.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let first = records.clone();
    let caller = tokio::spawn(async move { first.delete_secret(server).await });
    super::super::physical_tests::reached(&probe.started, 2).await;
    caller.abort();
    let _ = caller.await;
    let mut replacement = secret();
    replacement.access_token = "replacement-token".into();
    assert_eq!(
        records.store_secret(server, &replacement).await,
        Ok(Publication::Acknowledged)
    );
    assert_eq!(watchdog.join().unwrap(), (2, 2, 1));
    assert!(records.load_secret(server).await.unwrap() == Some(replacement));
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn worker_input_drop_keeps_slot() {
    let (records, directory) = records();
    let records = Arc::new(records);
    let server = Uuid::new_v4();
    let probe = records.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.input_drop.lock().unwrap() = Some(gate);
    let first = records.clone();
    let caller = tokio::spawn(async move { first.store_secret(server, &secret()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    let _ = caller.await;
    assert_eq!(records.delete_secret(server).await, Ok(Deletion::Deleted));
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn interrupted_record_operations_are_unavailable_or_unknown() {
    let (records, directory) = records();
    let server = Uuid::new_v4();
    let auth = ServerAuth::consent_needed(server, "docs", "https://mcp.example/mcp");
    records.probe.panic_before.store(true, Ordering::SeqCst);
    assert!(matches!(
        records.load(server).await,
        Err(RecordFailure::Unavailable)
    ));
    records.probe.panic_before.store(true, Ordering::SeqCst);
    assert!(records.load_secret(server).await == Err(RecordFailure::Unavailable));
    records.probe.panic_after.store(true, Ordering::SeqCst);
    assert_eq!(records.store(&auth).await, Err(RecordFailure::Unavailable));
    assert!(records.load(server).await.unwrap().is_some());
    records.probe.panic_after.store(true, Ordering::SeqCst);
    assert_eq!(
        records.store_secret(server, &secret()).await,
        Ok(Publication::Unknown)
    );
    assert!(records.load_secret(server).await.unwrap() == Some(secret()));
    records.probe.panic_after.store(true, Ordering::SeqCst);
    assert_eq!(records.delete_secret(server).await, Ok(Deletion::Unknown));
    assert!(records.load_secret(server).await.unwrap().is_none());
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn independent_instances_progress() {
    let (first, directory) = records();
    let (second, other_directory) = records();
    let first = Arc::new(first);
    let probe = first.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let worker = first.clone();
    let held = tokio::spawn(async move { worker.store_secret(Uuid::new_v4(), &secret()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    assert_eq!(
        second.store_secret(Uuid::new_v4(), &secret()).await,
        Ok(Publication::Acknowledged)
    );
    assert_eq!(probe.cleaned.load(std::sync::atomic::Ordering::SeqCst), 0);
    held.await.unwrap().unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    let _ = std::fs::remove_dir_all(directory);
    let _ = std::fs::remove_dir_all(other_directory);
}

#[test]
fn no_runtime_first_poll_is_typed() {
    let (records, directory) = records();
    let server = Uuid::new_v4();
    let auth = ServerAuth::consent_needed(server, "docs", "https://mcp.example/mcp");
    let run = super::super::physical_tests::plain_thread;
    assert!(matches!(
        run(records.load(server)),
        Err(RecordFailure::Unavailable)
    ));
    assert_eq!(
        super::super::physical_tests::plain_thread(records.store(&auth)),
        Err(RecordFailure::Unavailable)
    );
    assert!(
        super::super::physical_tests::plain_thread(records.load_secret(server))
            == Err(RecordFailure::Unavailable)
    );
    assert_eq!(
        super::super::physical_tests::plain_thread(records.store_secret(server, &secret())),
        Err(RecordFailure::Unavailable)
    );
    assert_eq!(
        super::super::physical_tests::plain_thread(records.delete_secret(server)),
        Err(RecordFailure::Unavailable)
    );
    assert_eq!(
        records
            .probe
            .submitted
            .load(std::sync::atomic::Ordering::SeqCst),
        0
    );
    assert!(!directory.exists());
}

#[test]
fn queued_send_future_resumes_on_plain_thread() {
    let (records, directory) = records();
    let records = Arc::new(records);
    let server = Uuid::new_v4();
    let probe = records.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_time()
        .build()
        .unwrap();
    let first = records.clone();
    let entered_runtime = runtime.enter();
    let held = tokio::spawn(async move { first.store_secret(server, &secret()).await });
    drop(entered_runtime);
    runtime.block_on(super::super::physical_tests::reached(&probe.started, 1));
    let later = records.clone();
    let mut future = Box::pin(async move { later.load_secret(server).await });
    runtime.block_on(std::future::poll_fn(|context| {
        assert!(future.as_mut().poll(context).is_pending());
        std::task::Poll::Ready(())
    }));
    let result = std::thread::spawn(move || super::super::physical_tests::plain_thread(future))
        .join()
        .unwrap();
    assert!(result.unwrap() == Some(secret()));
    runtime.block_on(held).unwrap().unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn ready_effect_input_drop_fault_is_uncertain() {
    let (records, directory) = records();
    let server = Uuid::new_v4();
    let auth = ServerAuth::consent_needed(server, "docs", "https://mcp.example/mcp");
    records.probe.panic_input_drop.store(true, Ordering::SeqCst);
    assert_eq!(records.store(&auth).await, Err(RecordFailure::Unavailable));
    assert!(records.load(server).await.unwrap().is_some());
    records.probe.panic_input_drop.store(true, Ordering::SeqCst);
    assert!(matches!(
        records.load(server).await,
        Err(RecordFailure::Unavailable)
    ));
    records.probe.panic_input_drop.store(true, Ordering::SeqCst);
    assert_eq!(
        records.store_secret(server, &secret()).await,
        Ok(Publication::Unknown)
    );
    assert!(records.load_secret(server).await.unwrap() == Some(secret()));
    records.probe.panic_input_drop.store(true, Ordering::SeqCst);
    assert!(records.load_secret(server).await == Err(RecordFailure::Unavailable));
    records.probe.panic_input_drop.store(true, Ordering::SeqCst);
    assert_eq!(records.delete_secret(server).await, Ok(Deletion::Unknown));
    assert!(records.load_secret(server).await.unwrap().is_none());
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn operation_panic_input_drop_keeps_slot() {
    let (records, directory) = records();
    let records = Arc::new(records);
    let server = Uuid::new_v4();
    let probe = records.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.input_drop.lock().unwrap() = Some(gate);
    probe.panic_after.store(true, Ordering::SeqCst);
    let first = records.clone();
    let caller = tokio::spawn(async move { first.store_secret(server, &secret()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    caller.abort();
    let _ = caller.await;
    assert_eq!(records.delete_secret(server).await, Ok(Deletion::Deleted));
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    let _ = std::fs::remove_dir_all(directory);
}

#[tokio::test]
async fn admission_precedes_owned_input_transfer() {
    let (records, directory) = records();
    let records = Arc::new(records);
    let server = Uuid::new_v4();
    let probe = records.probe.clone();
    let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
    *probe.before.lock().unwrap() = Some(gate);
    let first = records.clone();
    let held = tokio::spawn(async move { first.store_secret(server, &secret()).await });
    super::super::physical_tests::reached(&probe.started, 1).await;
    let auth = ServerAuth::consent_needed(server, "docs", "https://mcp.example/mcp");
    records.store(&auth).await.unwrap();
    held.await.unwrap().unwrap();
    assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
    assert_eq!(probe.inputs_at_release.load(Ordering::SeqCst), 1);
    assert!(records.load(server).await.unwrap().is_some());
    let _ = std::fs::remove_dir_all(directory);
}

fn isolated_fault_case(name: &str, body: impl FnOnce()) {
    const CHILD: &str = "NESSA_627_RECORD_FAULT_CASE";
    if std::env::var(CHILD).ok().as_deref() == Some(name) {
        body();
        return;
    }
    let exact = format!("mcp_authorization::infrastructure::records::tests::{name}");
    let mut child = std::process::Command::new(std::env::current_exe().unwrap())
        .args(["--exact", &exact])
        .env(CHILD, name)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .unwrap();
    let start = std::time::Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success(), "isolated records fault failed: {status}");
            break;
        }
        if start.elapsed() > std::time::Duration::from_secs(5) {
            child.kill().unwrap();
            let _ = child.wait();
            panic!("record fault subprocess watchdog expired");
        }
        std::thread::sleep(std::time::Duration::from_millis(10));
    }
}

#[test]
fn detached_caller_faulting_payload_is_contained() {
    isolated_fault_case("detached_caller_faulting_payload_is_contained", || {
        let (records, directory) = records();
        let records = Arc::new(records);
        let server = Uuid::new_v4();
        let probe = records.probe.clone();
        let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
        *probe.before.lock().unwrap() = Some(gate);
        probe.payload_before.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let first = records.clone();
                let caller =
                    tokio::spawn(async move { first.store_secret(server, &secret()).await });
                super::super::physical_tests::reached(&probe.started, 1).await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                assert!(records.load_secret(server).await.unwrap().is_none());
            });
        assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
        assert_eq!(
            probe.payload_dropped.load(Ordering::SeqCst),
            0,
            "faulting physical payload was disposed after caller disappeared"
        );
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(directory);
    });
}

#[test]
fn queued_shutdown_input_drop_fault_keeps_slot() {
    isolated_fault_case("queued_shutdown_input_drop_fault_keeps_slot", || {
        let (adapter, directory) = records();
        let adapter = Arc::new(adapter);
        let server = Uuid::new_v4();
        let probe = adapter.probe.clone();
        let (pool_gate, pool_watchdog) = super::super::physical_tests::hold(probe.clone());
        let (input_gate, input_watchdog) = super::super::physical_tests::hold(probe.clone());
        *probe.input_drop.lock().unwrap() = Some(input_gate);
        probe.panic_input_drop.store(true, Ordering::SeqCst);
        let runtime = tokio::runtime::Builder::new_current_thread()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let _blocker = runtime.spawn_blocking(move || pool_gate.enter());
        let first = adapter.clone();
        let mut future = Box::pin(async move { first.store_secret(server, &secret()).await });
        runtime.block_on(std::future::poll_fn(|cx| {
            assert!(future.as_mut().poll(cx).is_pending());
            std::task::Poll::Ready(())
        }));
        assert_eq!(probe.submitted.load(Ordering::SeqCst), 1);
        runtime.shutdown_timeout(std::time::Duration::ZERO);
        drop(future); // Detached caller; origin shutdown discards the queued capture.
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(async {
                assert!(adapter.load_secret(server).await.unwrap().is_none());
            });
        assert_eq!(pool_watchdog.join().unwrap(), (1, 0, 0));
        assert_eq!(input_watchdog.join().unwrap(), (1, 0, 0));
        assert_eq!(probe.started.load(Ordering::SeqCst), 1);
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 2);
        assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
        let _ = std::fs::remove_dir_all(directory);
    });
}

#[test]
fn detached_ready_input_drop_fault_is_contained() {
    isolated_fault_case("detached_ready_input_drop_fault_is_contained", || {
        let (records, directory) = records();
        let records = Arc::new(records);
        let server = Uuid::new_v4();
        let probe = records.probe.clone();
        let (gate, watchdog) = super::super::physical_tests::hold(probe.clone());
        *probe.input_drop.lock().unwrap() = Some(gate);
        probe.panic_input_drop.store(true, Ordering::SeqCst);
        tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .build()
            .unwrap()
            .block_on(async {
                let first = records.clone();
                let caller =
                    tokio::spawn(async move { first.store_secret(server, &secret()).await });
                super::super::physical_tests::reached(&probe.started, 1).await;
                caller.abort();
                assert!(caller.await.unwrap_err().is_cancelled());
                assert!(records.load_secret(server).await.unwrap() == Some(secret()));
            });
        assert_eq!(watchdog.join().unwrap(), (1, 1, 0));
        assert_eq!(probe.payload_dropped.load(Ordering::SeqCst), 0);
        assert_eq!(probe.cleaned.load(Ordering::SeqCst), 2);
        let _ = std::fs::remove_dir_all(directory);
    });
}
