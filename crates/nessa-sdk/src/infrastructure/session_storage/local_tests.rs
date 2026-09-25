//! Deterministically cancel callers while real file operations are queued.
use super::*;
use crate::application::agent_execution::providers::ProviderIdentity;
use crate::domain::agent_execution::sessions::{ExecutionSessionId, ProviderContext};
use std::{
    future::{poll_fn, Future},
    sync::mpsc,
    task::Poll,
};
use tokio::runtime::Builder;

#[test]
fn abandoned_file_operations_retain_the_lease_until_io_finishes() {
    #[derive(Clone, Copy, PartialEq)]
    enum Operation {
        Load,
        Save,
        Erase,
    }
    for kind in [Operation::Load, Operation::Save, Operation::Erase] {
        let runtime = Builder::new_multi_thread()
            .worker_threads(2)
            .max_blocking_threads(1)
            .build()
            .unwrap();
        let root = tempfile::tempdir().unwrap();
        private::create_directory(&root.path().join("private")).unwrap();
        let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
        let id = SessionId::new("abandoned").unwrap();
        let lease = runtime.block_on(storage.open(id.clone())).unwrap();
        let value = SessionSnapshot {
            queue_history: Vec::new(),
            id: id.clone(),
            provider: ProviderIdentity::new("test", "test", "test").unwrap(),
            provider_context: ProviderContext::Recorded(
                ExecutionSessionId::new("provider").unwrap(),
            ),
            invocations: vec![],
        };
        runtime.block_on(lease.save(value.clone())).unwrap();

        // Occupy the only blocking worker so the operation cannot finish before
        // the caller future and its lease handle have both been dropped.
        let (entered_tx, entered_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        let blocker = runtime.spawn_blocking(move || {
            entered_tx.send(()).unwrap();
            release_rx.recv().unwrap();
        });
        entered_rx.recv().unwrap();
        let mut operation = Box::pin(async {
            match kind {
                Operation::Load => lease.load().await.map(|_| ()),
                Operation::Save => lease.save(value).await,
                Operation::Erase => lease.erase().await,
            }
        });
        runtime.block_on(poll_fn(|cx| {
            assert!(operation.as_mut().poll(cx).is_pending());
            Poll::Ready(())
        }));
        drop(operation);
        drop(lease);

        // Probe the actual OS lock without queuing behind the blocked worker.
        let probe = private::open(
            &SessionPaths::new(&root.path().join("private"), &id).lock,
            OpenMode::ReadWrite,
        )
        .unwrap();
        let excluded = matches!(probe.try_lock(), Err(TryLockError::WouldBlock));
        drop(probe);
        release_tx.send(()).unwrap();
        runtime.block_on(blocker).unwrap();
        // Runtime drop waits for outstanding blocking I/O to finish.
        drop(runtime);
        let runtime = Builder::new_current_thread().build().unwrap();
        assert!(excluded, "abandoned I/O released the writer lock too early");
        let reopened = runtime.block_on(storage.open(id)).unwrap();
        // The abandoned erase still finished, under the lease it retained.
        assert_eq!(
            runtime.block_on(reopened.load()).unwrap().is_some(),
            kind != Operation::Erase
        );
    }
}

#[cfg(unix)]
mod fork_inheritance {
    use super::*;
    use std::{
        io::{Read, Write},
        os::{
            fd::AsRawFd,
            unix::{net::UnixStream, process::CommandExt},
        },
        process::Command,
        thread,
        time::Duration,
    };

    #[test]
    fn dropped_lease_unlocks_while_unrelated_child_is_paused_before_exec() {
        let runtime = Builder::new_current_thread().build().unwrap();
        let root = tempfile::tempdir().unwrap();
        private::create_directory(&root.path().join("private")).unwrap();
        let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
        let id = SessionId::new("forked-lock").unwrap();
        let lease = runtime.block_on(storage.open(id.clone())).unwrap();
        let (mut parent_gate, child_gate) = UnixStream::pair().unwrap();
        parent_gate
            .set_read_timeout(Some(Duration::from_secs(5)))
            .unwrap();
        let child = thread::spawn(move || {
            let mut command = Command::new("/bin/sh");
            command.args(["-c", "exit 0"]);
            // The child inherits the lease's open file description. Only async-
            // signal-safe syscalls run between fork and exec; no Rust locks or I/O.
            unsafe {
                command.pre_exec(move || {
                    let mut byte = [1_u8];
                    if libc::write(child_gate.as_raw_fd(), byte.as_ptr().cast(), 1) != 1 {
                        return Err(io::Error::last_os_error());
                    }
                    if libc::read(child_gate.as_raw_fd(), byte.as_mut_ptr().cast(), 1) != 1 {
                        return Err(io::Error::last_os_error());
                    }
                    Ok(())
                });
            }
            command.spawn().unwrap().wait().unwrap()
        });
        let mut ready = [0_u8];
        parent_gate.read_exact(&mut ready).unwrap();
        assert_eq!(ready, [1]);
        drop(lease);
        // Do not retry: lease drop must release the lock even though an unrelated
        // pre-exec child still holds its inherited description open.
        let reopened = runtime.block_on(storage.open(id));
        parent_gate.write_all(&[1]).unwrap();
        assert!(child.join().unwrap().success());
        assert!(
            reopened.is_ok(),
            "unrelated fork retained the dropped writer lease"
        );
    }
}

#[tokio::test]
async fn failed_append_and_sync_retry_reconcile_disk_before_acknowledgement() {
    for first_save in [true, false] {
        for fault in [
            SaveFault::PartialWrite,
            SaveFault::FileSync,
            SaveFault::DirectorySync,
        ] {
            let root = tempfile::tempdir().unwrap();
            let directory = root.path().join("private");
            private::create_directory(&directory).unwrap();
            let lock = private::open(
                &SessionPaths::new(&directory, &SessionId::new("retry").unwrap()).lock,
                OpenMode::OpenOrCreate,
            )
            .unwrap();
            lock.try_lock().unwrap();
            let store = LocalStore {
                root: directory.clone(),
                path: SessionPaths::new(&directory, &SessionId::new("retry").unwrap()).journal,
                id: SessionId::new("retry").unwrap(),
                lease: Arc::new(Lease {
                    lock,
                    operation: Mutex::new(None),
                    fault: Mutex::new(None),
                }),
            };
            let mut value = SessionSnapshot {
                queue_history: Vec::new(),
                id: store.id.clone(),
                provider: ProviderIdentity::new("test", "test", "test").unwrap(),
                provider_context: ProviderContext::Recorded(
                    ExecutionSessionId::new("provider").unwrap(),
                ),
                invocations: vec![],
            };
            if !first_save {
                store.save(value.clone()).await.unwrap();
            }
            value.provider_context =
                ProviderContext::Recorded(ExecutionSessionId::new("changed").unwrap());
            *store.lease.fault.lock().unwrap() = Some(fault);
            assert!(matches!(
                store.save(value.clone()).await,
                Err(StorageError::Io(_))
            ));
            let uncertain = std::fs::read(&store.path).unwrap();
            // A full-line failed save must still run sync on its no-op retry.
            if fault != SaveFault::PartialWrite {
                *store.lease.fault.lock().unwrap() = Some(SaveFault::FileSync);
                assert!(matches!(
                    store.save(value.clone()).await,
                    Err(StorageError::Io(_))
                ));
                *store.lease.fault.lock().unwrap() = Some(SaveFault::DirectorySync);
                assert!(matches!(
                    store.save(value.clone()).await,
                    Err(StorageError::Io(_))
                ));
                assert_eq!(std::fs::read(&store.path).unwrap(), uncertain);
            }
            store.save(value.clone()).await.unwrap();
            let saved = std::fs::read(&store.path).unwrap();
            assert_eq!(
                saved.iter().filter(|byte| **byte == b'\n').count(),
                if first_save { 1 } else { 2 }
            );
            assert_eq!(
                store.load().await.unwrap().unwrap().provider_context,
                value.provider_context
            );
            store.save(value).await.unwrap();
            assert_eq!(std::fs::read(&store.path).unwrap(), saved);
        }
    }
}

#[tokio::test]
async fn a_journal_that_cannot_be_read_is_logged_with_its_path() {
    use std::io::Write;
    #[derive(Clone, Default)]
    struct Capture(Arc<std::sync::Mutex<Vec<u8>>>);
    impl Write for Capture {
        fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(bytes);
            Ok(bytes.len())
        }
        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }
    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Capture {
        type Writer = Self;
        fn make_writer(&'a self) -> Self {
            self.clone()
        }
    }
    let root = tempfile::tempdir().unwrap();
    private::create_directory(&root.path().join("private")).unwrap();
    let storage = LocalFileStorage::new(root.path().join("private")).unwrap();
    let id = SessionId::new("damaged").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let journal = SessionPaths::new(&storage.root, &id).journal;
    // A private journal whose bytes are not one.
    private::open(&journal, OpenMode::CreateNew)
        .unwrap()
        .write_all(b"{broken}\n")
        .unwrap();
    // The process's one subscriber, not a scoped one: a call site another
    // test's thread reached first would otherwise be cached as uninteresting
    // to it. Nothing else in this binary sets one.
    static LOGS: std::sync::OnceLock<Capture> = std::sync::OnceLock::new();
    let captured = LOGS.get_or_init(|| {
        let captured = Capture::default();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .with_writer(captured.clone())
                .with_ansi(false)
                .finish(),
        )
        .unwrap();
        captured
    });
    assert!(matches!(lease.load().await, Err(StorageError::Corrupt(_))));
    let logged = String::from_utf8(captured.0.lock().unwrap().clone()).unwrap();
    // The file an operator would move aside, by the name it has on disk.
    assert!(logged.contains(&journal.display().to_string()), "{logged}");
}
