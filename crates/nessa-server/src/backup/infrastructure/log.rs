//! The deletion inventory on disk. The mac key sits beside it and is not
//! part of an export bundle.

use std::fs::{self, File};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::Mutex;

use nessa_local_storage::{create_directory, open, sync_directory, OpenMode, PrivateTempFile};

use async_trait::async_trait;

use crate::backup::application::{
    Authenticator, DeletionLogStore, InventoryReconciliationLease, LogStoreError, MacKey,
};
use crate::backup::domain::DeletionLog;

/// `directory/deletion-inventory.txt` plus `directory/mac.key`.
pub struct FileDeletionLog {
    directory: PathBuf,
    key: MacKey,
    gate: Mutex<()>,
}

impl FileDeletionLog {
    /// Open or create `directory`. The mac key is created when absent.
    pub fn open(directory: impl Into<PathBuf>) -> std::io::Result<Self> {
        let directory = directory.into();
        create_directory(&directory)?;
        let key_path = directory.join("mac.key");
        let key = if key_path.is_file() {
            let bytes = fs::read(&key_path)?;
            let bytes: [u8; 32] = bytes
                .try_into()
                .map_err(|_| std::io::Error::new(std::io::ErrorKind::InvalidData, "mac key"))?;
            MacKey(bytes)
        } else {
            create_key(&directory, &key_path)?
        };
        Ok(Self {
            directory,
            key,
            gate: Mutex::new(()),
        })
    }

    pub fn key(&self) -> MacKey {
        self.key
    }

    fn path(&self) -> PathBuf {
        self.directory.join("deletion-inventory.txt")
    }
}

#[async_trait]
impl DeletionLogStore for FileDeletionLog {
    async fn hold_reconciliation(&self) -> Result<Box<dyn InventoryReconciliationLease>, ()> {
        let directory = self.directory.clone();
        let file = tokio::task::spawn_blocking(move || {
            open(
                &directory.join("reconciliation.lock"),
                OpenMode::OpenOrCreate,
            )
            .map_err(|_| ())
        })
        .await
        .map_err(|_| ())??;
        loop {
            match file.try_lock() {
                Ok(()) => return Ok(Box::new(FileReconciliationLease { _file: file })),
                Err(std::fs::TryLockError::WouldBlock) => {
                    // The holder's snapshot may need the blocking pool. Wait
                    // asynchronously instead of occupying one of its workers
                    // (`waiting_captures_leave_a_blocking_worker_for_the_holder`).
                    tokio::time::sleep(std::time::Duration::from_millis(10)).await;
                }
                Err(std::fs::TryLockError::Error(_)) => return Err(()),
            }
        }
    }

    async fn load(&self) -> Result<DeletionLog, ()> {
        let path = self.path();
        let key = self.key;
        let gate = self.gate.lock().map_err(|_| ())?;
        let _file = exclusive(&self.directory).map_err(|_| ())?;
        let log = read_log(&path, key)?;
        drop(gate);
        Ok(log)
    }

    async fn store(&self, base: &DeletionLog, next: &DeletionLog) -> Result<(), LogStoreError> {
        let path = self.path();
        let directory = self.directory.clone();
        let key = self.key;
        let text = sealed(next, key);
        let expected = base.canonical();
        let gate = self.gate.lock().map_err(|_| LogStoreError::Failed)?;
        let _file = exclusive(&directory).map_err(|_| LogStoreError::Failed)?;
        let current = read_log(&path, key).map_err(|_| LogStoreError::Failed)?;
        if current.canonical() != expected {
            return Err(LogStoreError::Stale);
        }
        persist(&directory, &path, &text).map_err(|_| LogStoreError::Failed)?;
        drop(gate);
        Ok(())
    }
}

/// Own the process-safe reconciliation lock across fence reads and publication.
struct FileReconciliationLease {
    _file: File,
}

impl InventoryReconciliationLease for FileReconciliationLease {}

/// Serialize inventory and decision publication in this directory, including
/// temporary cleanup and rollback. The lock lasts until the file is dropped
/// (`a_decision_write_waits_for_inventory_rollback`).
pub(super) fn exclusive(directory: &Path) -> std::io::Result<File> {
    let file = open(
        &directory.join("deletion-inventory.lock"),
        OpenMode::OpenOrCreate,
    )?;
    file.lock()?;
    Ok(file)
}

/// Write a new mac key and sync the file and its directory before the key
/// is returned. A crash after `open` returns cannot leave a sealed inventory
/// whose key is missing.
fn create_key(directory: &Path, key_path: &Path) -> std::io::Result<MacKey> {
    let mut bytes = [0u8; 32];
    getrandom::fill(&mut bytes).map_err(|error| std::io::Error::other(error.to_string()))?;
    let mut file = open(key_path, OpenMode::CreateNew)?;
    let written = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        Ok(())
    })();
    if let Err(error) = written {
        drop(file);
        let _ = fs::remove_file(key_path);
        return Err(error);
    }
    drop(file);
    sync_directory(directory)?;
    Ok(MacKey(bytes))
}

/// Write `text` to a private temporary file, sync that file, rename it over
/// `path`, then sync `directory`. Success means the rename is durable.
///
/// The temporary name is this attempt's. A directory sync that fails after
/// the rename puts the previous bytes back, or removes `path` when it was
/// absent, before the error is returned.
pub(crate) fn persist(directory: &Path, path: &Path, text: &str) -> std::io::Result<()> {
    persist_with(directory, path, text, &sync_directory)
}

fn persist_with(
    directory: &Path,
    path: &Path,
    text: &str,
    sync_dir: &dyn Fn(&Path) -> std::io::Result<()>,
) -> std::io::Result<()> {
    PrivateTempFile::clear_stale(directory)?;
    let mut temporary = PrivateTempFile::new_in(directory)?;
    temporary.as_file_mut().write_all(text.as_bytes())?;
    temporary.as_file().sync_all()?;
    let previous = preserve_previous(directory, path)?;
    temporary.persist(path)?;
    if let Err(error) = sync_dir(directory) {
        restore_previous(path, previous)?;
        return Err(error);
    }
    Ok(())
}

/// A private copy of `path`, synced, so a failed directory sync can put it
/// back. `None` means `path` was absent.
fn preserve_previous(directory: &Path, path: &Path) -> std::io::Result<Option<PrivateTempFile>> {
    if !path.is_file() {
        return Ok(None);
    }
    let bytes = fs::read(path)?;
    let mut copy = PrivateTempFile::new_in(directory)?;
    copy.as_file_mut().write_all(&bytes)?;
    copy.as_file().sync_all()?;
    Ok(Some(copy))
}

fn restore_previous(path: &Path, previous: Option<PrivateTempFile>) -> std::io::Result<()> {
    match previous {
        Some(copy) => copy.persist(path),
        None => fs::remove_file(path),
    }
}

fn sealed(log: &DeletionLog, key: MacKey) -> String {
    let canonical = log.canonical();
    let mac = key.mac(&canonical).expect("a 32-byte key macs");
    let mut text = canonical;
    text.push_str(&format!("mac {}\n", mac.hex()));
    text
}

fn read_log(path: &Path, key: MacKey) -> Result<DeletionLog, ()> {
    if !path.exists() {
        return Ok(DeletionLog::default());
    }
    let text = fs::read_to_string(path).map_err(|_| ())?;
    let (canonical, mac_hex) = split_mac(&text)?;
    let expected = key.mac(canonical).map_err(|_| ())?;
    if expected.hex() != mac_hex {
        return Err(());
    }
    super::deletion_decode::decode_inventory(canonical).map_err(|_| ())
}

/// Authenticate a deletion-inventory file that carries its own mac line.
pub(crate) fn authenticate_inventory(text: &str, key: MacKey) -> Result<DeletionLog, ()> {
    let (canonical, mac_hex) = split_mac(text)?;
    let expected = key.mac(canonical).map_err(|_| ())?;
    if expected.hex() != mac_hex {
        return Err(());
    }
    super::deletion_decode::decode_inventory(canonical).map_err(|_| ())
}

fn split_mac(text: &str) -> Result<(&str, &str), ()> {
    let mac_at = text.rfind("\nmac ").ok_or(())?;
    let canonical = &text[..mac_at + 1];
    let mac_line = text[mac_at + 1..].trim_end_matches('\n');
    let hex = mac_line.strip_prefix("mac ").ok_or(())?;
    Ok((canonical, hex))
}

#[cfg(test)]
mod tests {
    use super::*;

    use crate::backup::domain::{DeletionFence, GrantRole};

    fn inventory() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("inventory");
        create_directory(&directory).unwrap();
        let path = directory.join("deletion-inventory.txt");
        (root, directory, path)
    }

    /// The decision publisher must not sweep the inventory's live rollback
    /// copy. The directory-sync seam pauses exactly after replacement.
    #[test]
    fn a_decision_write_waits_for_inventory_rollback() {
        use std::sync::{mpsc, Arc};
        use std::time::Duration;

        use nessa_auth::application::ports::Clock;

        use crate::backup::application::{
            InventoryAction, InventoryCause, InventoryDecision, InventoryDecisionAudit,
            InventoryFacts,
        };
        use crate::backup::infrastructure::decision_audit::FileInventoryDecisions;

        struct FixedClock;
        impl Clock for FixedClock {
            fn unix_milliseconds(&self) -> u64 {
                1
            }
        }

        let (_root, directory, path) = inventory();
        persist_with(&directory, &path, "previous\n", &sync_directory).unwrap();
        let decisions = FileInventoryDecisions::open(&directory, Arc::new(FixedClock)).unwrap();
        let held = exclusive(&directory).unwrap();
        let (started, ready) = mpsc::channel();
        let (finished, result) = mpsc::channel();
        let writer = Mutex::new(Some((decisions, started, finished)));
        let error = persist_with(&directory, &path, "replacement\n", &|_| {
            let (decisions, started, finished) = writer.lock().unwrap().take().unwrap();
            std::thread::spawn(move || {
                started.send(()).unwrap();
                let stored = decisions.record(
                    &InventoryFacts {
                        kind: "gateway",
                        cause: InventoryCause::CommittedFence,
                        action: InventoryAction::Record,
                    },
                    &InventoryDecision::Allow {
                        policies: vec!["inventory.record.gateway".to_owned()],
                    },
                );
                finished.send(stored).unwrap();
            });
            ready.recv_timeout(Duration::from_secs(10)).unwrap();
            assert!(
                matches!(
                    result.recv_timeout(Duration::from_secs(1)),
                    Err(mpsc::RecvTimeoutError::Timeout)
                ),
                "the audit must wait while the inventory rollback copy is live"
            );
            Err(std::io::Error::other("directory sync failed"))
        });
        assert!(error.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "previous\n");
        drop(held);
        result
            .recv_timeout(Duration::from_secs(10))
            .unwrap()
            .unwrap();
        let audit = fs::read_to_string(directory.join("inventory-decisions.jsonl")).unwrap();
        assert!(audit.contains("inventory-decision-1"));
        assert_eq!(fs::read_to_string(&path).unwrap(), "previous\n");
    }

    #[test]
    fn a_directory_sync_failure_restores_the_previous_log() {
        let (_root, directory, path) = inventory();
        persist_with(&directory, &path, "previous\n", &sync_directory).unwrap();
        let error = persist_with(&directory, &path, "replacement\n", &|_| {
            Err(std::io::Error::other("directory sync failed"))
        });
        assert!(error.is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "previous\n");
    }

    #[test]
    fn a_directory_sync_failure_removes_a_log_that_was_absent() {
        let (_root, directory, path) = inventory();
        let error = persist_with(&directory, &path, "replacement\n", &|_| {
            Err(std::io::Error::other("directory sync failed"))
        });
        assert!(error.is_err());
        assert!(!path.exists());
    }

    #[test]
    fn a_stored_log_is_synced_before_it_is_acknowledged() {
        let (_root, directory, path) = inventory();
        let synced = Mutex::new(false);
        persist_with(&directory, &path, "body\n", &|directory| {
            *synced.lock().unwrap() = true;
            sync_directory(directory)
        })
        .unwrap();
        assert!(*synced.lock().unwrap());
        assert_eq!(fs::read_to_string(&path).unwrap(), "body\n");
    }

    /// A real inventory lease remains held while its fence source needs the
    /// blocking pool. A waiting capture must leave that worker available.
    #[test]
    fn waiting_captures_leave_a_blocking_worker_for_the_holder() {
        use std::sync::Arc;
        use std::time::Duration;

        use crate::backup::application::{capture_fences, FenceSource};

        struct PausedSource {
            entered: tokio::sync::Notify,
            resume: tokio::sync::Notify,
        }

        #[async_trait::async_trait]
        impl FenceSource for PausedSource {
            async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
                self.entered.notify_one();
                self.resume.notified().await;
                // Metadata snapshot reads use this pool after lease acquisition.
                tokio::task::spawn_blocking(|| {
                    vec![DeletionFence::share_grant("c", "r", "cred", GrantRole::Read, 4).unwrap()]
                })
                .await
                .map_err(|_| ())
            }
        }

        struct CurrentGrant;
        #[async_trait::async_trait]
        impl FenceSource for CurrentGrant {
            async fn fences(&self) -> Result<Vec<DeletionFence>, ()> {
                Ok(Vec::new())
            }
        }

        let root = tempfile::tempdir().unwrap();
        let log = Arc::new(FileDeletionLog::open(root.path().join("inventory")).unwrap());
        let source = Arc::new(PausedSource {
            entered: tokio::sync::Notify::new(),
            resume: tokio::sync::Notify::new(),
        });
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let first_log = log.clone();
            let first_source = source.clone();
            let mut first = tokio::spawn(async move {
                capture_fences(&[first_source.as_ref()], first_log.as_ref()).await
            });
            source.entered.notified().await;
            let current = CurrentGrant;
            let sources: [&dyn FenceSource; 1] = [&current];
            let mut second = Box::pin(capture_fences(&sources, log.as_ref()));
            std::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(second.as_mut(), cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            source.resume.notify_one();
            let progress = tokio::time::timeout(Duration::from_secs(2), async {
                let older = (&mut first).await.unwrap().unwrap();
                assert_eq!(older.entries().len(), 1);
                let newer = second.as_mut().await.unwrap();
                assert!(newer.entries().is_empty());
            })
            .await;
            // Release the holder even on regression, then join the blocking
            // queue before asserting, so a failed test cannot hang shutdown.
            if progress.is_err() {
                first.abort();
                let _ = first.await;
            }
            drop(second);
            tokio::time::timeout(Duration::from_secs(10), tokio::task::spawn_blocking(|| ()))
                .await
                .unwrap()
                .unwrap();
            assert!(
                progress.is_ok(),
                "lock waiters starved the holder's snapshot work"
            );
            assert!(log.load().await.unwrap().entries().is_empty());
        });
    }

    #[test]
    fn a_cancelled_reconciliation_waiter_releases_its_eventual_lock() {
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("inventory");
        let log = FileDeletionLog::open(&directory).unwrap();
        let held = open(
            &directory.join("reconciliation.lock"),
            OpenMode::OpenOrCreate,
        )
        .unwrap();
        held.lock().unwrap();
        // The sentinel cannot finish before the cancelled blocking acquisition:
        // one blocking thread runs their FIFO work queue.
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_time()
            .max_blocking_threads(1)
            .build()
            .unwrap();
        runtime.block_on(async {
            let mut waiting = Box::pin(log.hold_reconciliation());
            std::future::poll_fn(|cx| {
                assert!(std::future::Future::poll(waiting.as_mut(), cx).is_pending());
                std::task::Poll::Ready(())
            })
            .await;
            drop(waiting);
            let completed = tokio::task::spawn_blocking(|| ());
            drop(held);
            tokio::time::timeout(Duration::from_secs(10), completed)
                .await
                .unwrap()
                .unwrap();
            let next = tokio::time::timeout(Duration::from_secs(10), log.hold_reconciliation())
                .await
                .unwrap()
                .unwrap();
            drop(next);
        });
    }

    #[tokio::test]
    async fn separately_opened_inventories_share_the_reconciliation_lease() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("inventory");
        let first = FileDeletionLog::open(&directory).unwrap();
        let second = FileDeletionLog::open(&directory).unwrap();
        let contender = open(
            &directory.join("reconciliation.lock"),
            OpenMode::OpenOrCreate,
        )
        .unwrap();
        for inventory in [&first, &second] {
            let lease = inventory.hold_reconciliation().await.unwrap();
            assert!(matches!(
                contender.try_lock(),
                Err(std::fs::TryLockError::WouldBlock)
            ));
            drop(lease);
            contender.try_lock().unwrap();
            contender.unlock().unwrap();
        }
    }

    #[tokio::test]
    async fn a_stale_writer_does_not_replace_the_log() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("inventory");
        let first = FileDeletionLog::open(&directory).unwrap();
        let second = FileDeletionLog::open(&directory).unwrap();
        let base = first.load().await.unwrap();
        let kept = DeletionFence::conversation("kept").unwrap();
        let (next, _) = base.append(kept);
        first.store(&base, &next).await.unwrap();
        let lost = DeletionFence::conversation("lost").unwrap();
        let (other, _) = base.append(lost);
        let error = second.store(&base, &other).await.unwrap_err();
        assert_eq!(error, LogStoreError::Stale);
        let loaded = second.load().await.unwrap();
        assert!(loaded.canonical().contains("conversation kept"));
        assert!(!loaded.canonical().contains("conversation lost"));
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn concurrent_stores_keep_one_complete_log() {
        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("inventory");
        let first = FileDeletionLog::open(&directory).unwrap();
        let second = FileDeletionLog::open(&directory).unwrap();
        let base = first.load().await.unwrap();
        let (next_a, _) = base.append(DeletionFence::conversation("writer-a").unwrap());
        let (next_b, _) = base.append(DeletionFence::conversation("writer-b").unwrap());
        let base_for_b = base.clone();
        let left = tokio::spawn(async move { first.store(&base, &next_a).await });
        let right = tokio::spawn(async move { second.store(&base_for_b, &next_b).await });
        let (left, right) = tokio::join!(left, right);
        let left = left.unwrap();
        let right = right.unwrap();
        let results = [left, right];
        assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
        assert_eq!(
            results
                .iter()
                .filter(|result| matches!(result, Err(LogStoreError::Stale)))
                .count(),
            1
        );
        let text = FileDeletionLog::open(&directory)
            .unwrap()
            .load()
            .await
            .unwrap()
            .canonical();
        let has_a = text.contains("writer-a");
        let has_b = text.contains("writer-b");
        assert!(has_a ^ has_b);
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn a_store_waits_while_the_directory_lock_is_held() {
        use std::time::Duration;

        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("inventory");
        let log = FileDeletionLog::open(&directory).unwrap();
        let held = open(
            &directory.join("deletion-inventory.lock"),
            OpenMode::OpenOrCreate,
        )
        .unwrap();
        held.lock().unwrap();
        let base = DeletionLog::default();
        let (next, _) = base.append(DeletionFence::conversation("waiting").unwrap());
        let task = tokio::spawn(async move { log.store(&base, &next).await });
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            !task.is_finished(),
            "store must wait while deletion-inventory.lock is held"
        );
        drop(held);
        task.await.unwrap().unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn the_inventory_directory_and_key_are_owner_only() {
        use std::os::unix::fs::PermissionsExt;

        let root = tempfile::tempdir().unwrap();
        let directory = root.path().join("inventory");
        FileDeletionLog::open(&directory).unwrap();
        let mode = |path: &Path| fs::metadata(path).unwrap().permissions().mode() & 0o777;
        assert_eq!(mode(&directory), 0o700);
        assert_eq!(mode(&directory.join("mac.key")), 0o600);
    }
}
