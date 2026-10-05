//! A caller's panicking waker at a record-storage wait.
//!
//! Rows of "Caller wakers" in `docs/agent_execution/lifecycle.md`. An exclusive
//! lock holds the SQLite worker until the caller's waker is installed.
//! `runtime` is woken by that worker's open oneshot, whether the first caller
//! is `initialize` or `record_identity`, and `read_committed` by the read
//! thread's reply: without `contain_caller_wake` the later use fails. After
//! the runtime exists, `open`, `open_existing`, and the record-source lookups
//! are woken by event-stream `tracked_read`'s Tokio task, which already
//! catches a plain waker panic; those tests show the worker still answers.

use nessa_sdk::{
    application::agent_execution::sessions::SessionStorage,
    domain::agent_execution::sessions::SessionId, infrastructure::session_storage::RecordStorage,
};
use nessa_sync::replication::domain::Id;
use rusqlite::Connection;
use std::{
    future::Future,
    path::{Path, PathBuf},
    pin::Pin,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    task::{Context, Poll, Wake, Waker},
    time::Duration,
};

const BOUND: Duration = Duration::from_secs(5);

struct PanicWake {
    seen: Mutex<Option<tokio::sync::oneshot::Sender<()>>>,
}
impl Wake for PanicWake {
    fn wake(self: Arc<Self>) {
        if let Some(seen) = self.seen.lock().unwrap().take() {
            let _ = seen.send(());
        }
        panic!("caller waker");
    }
}

struct CountWake(AtomicUsize);
impl Wake for CountWake {
    fn wake(self: Arc<Self>) {
        self.0.fetch_add(1, Ordering::SeqCst);
    }
}

fn panic_waker() -> (Waker, tokio::sync::oneshot::Receiver<()>) {
    let (seen, notified) = tokio::sync::oneshot::channel();
    (
        Waker::from(Arc::new(PanicWake {
            seen: Mutex::new(Some(seen)),
        })),
        notified,
    )
}

fn origin() -> Id {
    Id::new("origin").unwrap()
}

fn opened() -> (tempfile::TempDir, Arc<RecordStorage>, PathBuf) {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("sessions");
    let storage = Arc::new(RecordStorage::new(&root).unwrap());
    (directory, storage, root)
}

fn database(root: &Path) -> PathBuf {
    root.join("records.sqlite3")
}

/// Holds `BEGIN EXCLUSIVE` until dropped, so the worker's next statement waits.
fn exclusive(path: &Path) -> Connection {
    let connection = Connection::open(path).unwrap();
    connection.execute_batch("BEGIN EXCLUSIVE").unwrap();
    connection
}

fn park<F>(wait: Pin<&mut F>, waker: &Waker)
where
    F: Future + ?Sized,
{
    assert!(
        wait.poll(&mut Context::from_waker(waker)).is_pending(),
        "the wait must be parked before the worker sends"
    );
}

async fn worker_answers(storage: &RecordStorage) {
    let id = SessionId::new("later").unwrap();
    let found = tokio::time::timeout(BOUND, storage.record_identity(&id, origin()))
        .await
        .expect("the sqlite worker still answers");
    assert!(found.expect("identity lookup").is_none());
}

/// Drive `wait` with a noop waker until the fast path before the worker has
/// finished, so the next poll's waker is the one the worker sends to.
async fn reach_worker<F>(mut wait: Pin<&mut F>)
where
    F: Future + ?Sized,
{
    let noop = Waker::noop();
    let start = std::time::Instant::now();
    while start.elapsed() < Duration::from_millis(200) {
        assert!(
            wait.as_mut()
                .poll(&mut Context::from_waker(noop))
                .is_pending(),
            "the worker answered while the database was locked"
        );
        tokio::task::yield_now().await;
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
}

#[tokio::test]
async fn panicking_initialize_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut init = Box::pin(storage.initialize());
    park(init.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        init.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    ));
    worker_answers(&storage).await;
}

/// The first `record_identity` opens the runtime. Its waker is the one
/// `ready.send` invokes, the same oneshot as `initialize`.
#[tokio::test]
async fn panicking_first_identity_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    let id = SessionId::new("conversation").unwrap();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut identity = Box::pin(storage.record_identity(&id, origin()));
    park(identity.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    // The ready oneshot wakes first; find_stream is a second await.
    let found = tokio::time::timeout(BOUND, async {
        loop {
            if let Poll::Ready(result) = identity
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop()))
            {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the first identity lookup finishes");
    assert!(found.expect("identity lookup").is_none());
    worker_answers(&storage).await;
}

#[tokio::test]
async fn panicking_record_identity_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut identity = Box::pin(storage.record_identity(&id, origin()));
    park(identity.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        identity
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(None))
    ));
    worker_answers(&storage).await;
}

#[tokio::test]
async fn panicking_record_source_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut source = Box::pin(storage.record_source(&id, origin()));
    park(source.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        source
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(None))
    ));
    worker_answers(&storage).await;
}

#[tokio::test]
async fn panicking_record_source_expected_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lease = storage.open(id.clone()).await.unwrap();
    let identity = storage
        .record_identity(&id, origin())
        .await
        .unwrap()
        .expect("open created a stream");
    drop(lease);
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut source = Box::pin(storage.record_source_expected(&id, &identity));
    park(source.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        source
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(_))
    ));
    worker_answers(&storage).await;
}

#[tokio::test]
async fn panicking_open_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut open = Box::pin(storage.open(id));
    reach_worker(open.as_mut()).await;
    park(open.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    // create_stream wakes first; replay is a second await. Drive until the
    // lease is ready. A dead worker fails that replay.
    let opened = tokio::time::timeout(BOUND, async {
        loop {
            if let Poll::Ready(result) = open.as_mut().poll(&mut Context::from_waker(Waker::noop()))
            {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("open finishes after the contained wake");
    opened.expect("open keeps its lease");
    worker_answers(&storage).await;
}

#[tokio::test]
async fn panicking_open_existing_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut open = Box::pin(storage.open_existing(id));
    reach_worker(open.as_mut()).await;
    park(open.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        open.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(None))
    ));
    worker_answers(&storage).await;
}

#[tokio::test]
async fn panicking_committed_read_waiter_is_not_a_shutdown_failure() {
    let (_directory, storage, root) = opened();
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut read = Box::pin(storage.read_committed(id));
    park(read.as_mut(), &waker);
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        read.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(_))
    ));
    tokio::time::timeout(BOUND, storage.shutdown())
        .await
        .expect("shutdown finishes")
        .expect("a contained read-thread wake is not ReadWorkerPanicked");
}

#[tokio::test]
async fn panicking_shutdown_waiter_still_wakes_the_other_shutdown() {
    let (_directory, storage, root) = opened();
    storage.initialize().await.unwrap();
    let id = SessionId::new("conversation").unwrap();
    let lock = exclusive(&database(&root));
    let mut read = Box::pin(storage.read_committed(id));
    park(read.as_mut(), Waker::noop());
    let (waker, notified) = panic_waker();
    let mut first = Box::pin(storage.shutdown());
    park(first.as_mut(), &waker);
    let seen = Arc::new(CountWake(AtomicUsize::new(0)));
    let mut second = Box::pin(storage.shutdown());
    park(second.as_mut(), &Waker::from(seen.clone()));
    drop(lock);
    tokio::time::timeout(BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(BOUND, async {
        while seen.0.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the other shutdown waiter was woken");
    assert!(matches!(
        first.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    ));
    assert!(matches!(
        second
            .as_mut()
            .poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(()))
    ));
}
