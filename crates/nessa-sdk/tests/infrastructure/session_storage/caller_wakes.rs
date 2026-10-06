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
//!
//! `ready.send` runs only after the worker's `open_connection` returns. The
//! two first-runtime tests therefore time cold open, not a later statement.
//! An expired deadline records which of those waits is still active.

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

/// One locked statement after the runtime is already open.
const STATEMENT_BOUND: Duration = Duration::from_secs(5);

/// Hang ceiling for the caller wake after the exclusive lock is dropped.
///
/// The wait is the caller waker. `ready.send` runs only after
/// `open_connection`, which includes that connection's five-second busy
/// timeout and a turn on the blocking pool. A five-second bound expired
/// while the open was still running (#548); ten seconds is only one busy
/// timeout past that and still expires when the pool is behind the rest of
/// this crate's storage tests (#558). Expiry still classifies the wait.
const COLD_OPEN_BOUND: Duration = Duration::from_secs(60);

/// What the first-runtime wake deadline observed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FirstWake {
    /// The caller waker ran before the deadline.
    Invoked,
    /// The caller waker ran only while the expired deadline was being classified.
    Late,
    /// `open_connection` has not sent `ready`.
    ColdOpenStillRunning,
    /// The waited call already holds its result, and the caller waker did not run.
    PublishedWithoutWake,
    /// `ready` was stored and the call is pending on a later statement.
    ReadyThenStillPending,
}

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
    let found = tokio::time::timeout(STATEMENT_BOUND, storage.record_identity(&id, origin()))
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

struct ExpiredObservation {
    published: bool,
    woke_during_poll: bool,
    runtime_open: bool,
}

/// A wake observed while classifying wins over a stored result. Otherwise a
/// stored result is a missing wake, including when the call has already moved
/// on to a later statement.
fn classify_expired_wake(observation: ExpiredObservation) -> FirstWake {
    if observation.woke_during_poll {
        return FirstWake::Late;
    }
    if observation.published {
        return FirstWake::PublishedWithoutWake;
    }
    if observation.runtime_open {
        FirstWake::ReadyThenStillPending
    } else {
        FirstWake::ColdOpenStillRunning
    }
}

fn wake_arrived(notified: &mut tokio::sync::oneshot::Receiver<()>) -> bool {
    match notified.try_recv() {
        Ok(()) => true,
        Err(tokio::sync::oneshot::error::TryRecvError::Empty) => false,
        Err(tokio::sync::oneshot::error::TryRecvError::Closed) => {
            panic!("the caller waker dropped its notification without firing")
        }
    }
}

/// `true` when a second `initialize` poll finds the runtime already stored.
fn runtime_is_open(storage: &RecordStorage) -> bool {
    let mut init = Box::pin(storage.initialize());
    init.as_mut()
        .poll(&mut Context::from_waker(Waker::noop()))
        .is_ready()
}

/// Wait until the caller waker runs, or classify the wait still active at `bound`.
///
/// The classification poll uses `waker` again, so a `ready` that was already
/// stored does not count as a wake, and a `ready` that arrives during
/// classification still records that the caller waker ran.
async fn classify_first_wake<F>(
    bound: Duration,
    waker: &Waker,
    mut notified: tokio::sync::oneshot::Receiver<()>,
    mut wait: Pin<&mut F>,
    storage: &RecordStorage,
) -> FirstWake
where
    F: Future + ?Sized,
{
    let deadline = tokio::time::Instant::now() + bound;
    tokio::select! {
        biased;
        received = &mut notified => {
            received.expect("the caller waker keeps its notification sender");
            return FirstWake::Invoked;
        }
        _ = tokio::time::sleep_until(deadline) => {}
    }
    if wake_arrived(&mut notified) {
        return FirstWake::Invoked;
    }
    let published = wait
        .as_mut()
        .poll(&mut Context::from_waker(waker))
        .is_ready();
    let woke_during_poll = wake_arrived(&mut notified);
    let runtime_open = !woke_during_poll && !published && runtime_is_open(storage);
    classify_expired_wake(ExpiredObservation {
        published,
        woke_during_poll,
        runtime_open,
    })
}

#[test]
fn a_wake_during_classification_is_not_a_missing_wake() {
    assert_eq!(
        classify_expired_wake(ExpiredObservation {
            published: true,
            woke_during_poll: true,
            runtime_open: false,
        }),
        FirstWake::Late
    );
}

fn assert_first_wake(observed: FirstWake, bound: Duration) {
    assert_eq!(
        observed,
        FirstWake::Invoked,
        "after {bound:?}, the first-runtime caller wake was {observed:?}"
    );
}

#[tokio::test]
async fn panicking_initialize_waiter_leaves_the_worker_answering() {
    let (_directory, storage, root) = opened();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut init = Box::pin(storage.initialize());
    park(init.as_mut(), &waker);
    drop(lock);
    let observed =
        classify_first_wake(COLD_OPEN_BOUND, &waker, notified, init.as_mut(), &storage).await;
    assert_first_wake(observed, COLD_OPEN_BOUND);
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
    let observed = classify_first_wake(
        COLD_OPEN_BOUND,
        &waker,
        notified,
        identity.as_mut(),
        &storage,
    )
    .await;
    assert_first_wake(observed, COLD_OPEN_BOUND);
    // The ready oneshot wakes first; find_stream is a second await.
    let found = tokio::time::timeout(STATEMENT_BOUND, async {
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

/// The exclusive lock stays held across the deadline. Expiry names cold open
/// still running; `panicking_initialize_waiter_leaves_the_worker_answering`
/// is the case where that open then invokes the caller waker.
#[tokio::test]
async fn held_exclusive_lock_past_the_deadline_is_still_cold_open() {
    let (_directory, storage, root) = opened();
    let lock = exclusive(&database(&root));
    let (waker, notified) = panic_waker();
    let mut init = Box::pin(storage.initialize());
    park(init.as_mut(), &waker);
    let observed = classify_first_wake(
        Duration::from_millis(50),
        &waker,
        notified,
        init.as_mut(),
        &storage,
    )
    .await;
    assert_eq!(observed, FirstWake::ColdOpenStillRunning);
    drop(lock);
    // Poll the parked open itself. A second `initialize` would wait on this
    // same cell and never drive it.
    tokio::time::timeout(COLD_OPEN_BOUND, async {
        let noop = Waker::noop();
        loop {
            if let Poll::Ready(result) = init.as_mut().poll(&mut Context::from_waker(noop)) {
                return result;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("open finishes once the lock drops")
    .expect("open");
}

/// `ready` is stored by a waker other than the caller waker under test.
/// Expiry must name that stored result, not cold open still running.
#[tokio::test]
async fn stored_ready_without_the_caller_waker_is_not_cold_open() {
    let (_directory, storage, root) = opened();
    let seen = Arc::new(CountWake(AtomicUsize::new(0)));
    let lock = exclusive(&database(&root));
    let mut init = Box::pin(storage.initialize());
    park(init.as_mut(), &Waker::from(Arc::clone(&seen)));
    drop(lock);
    tokio::time::timeout(COLD_OPEN_BOUND, async {
        while seen.0.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("open invoked the waker that was parked");
    let (waker, notified) = panic_waker();
    let observed = classify_first_wake(
        Duration::from_millis(20),
        &waker,
        notified,
        init.as_mut(),
        &storage,
    )
    .await;
    assert_eq!(observed, FirstWake::PublishedWithoutWake);
}

/// `record_identity` stores `ready` and then waits on `find_stream`. Expiry
/// must name that later statement, not cold open still running.
#[tokio::test]
async fn stored_identity_ready_without_the_caller_waker_is_a_later_statement() {
    let (_directory, storage, root) = opened();
    let id = SessionId::new("conversation").unwrap();
    let seen = Arc::new(CountWake(AtomicUsize::new(0)));
    let lock = exclusive(&database(&root));
    let mut identity = Box::pin(storage.record_identity(&id, origin()));
    park(identity.as_mut(), &Waker::from(Arc::clone(&seen)));
    drop(lock);
    tokio::time::timeout(COLD_OPEN_BOUND, async {
        while seen.0.load(Ordering::SeqCst) == 0 {
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("open invoked the waker that was parked");
    let (waker, notified) = panic_waker();
    let observed = classify_first_wake(
        Duration::from_millis(20),
        &waker,
        notified,
        identity.as_mut(),
        &storage,
    )
    .await;
    assert_eq!(observed, FirstWake::ReadyThenStillPending);
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
    tokio::time::timeout(STATEMENT_BOUND, notified)
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
    tokio::time::timeout(STATEMENT_BOUND, notified)
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
    tokio::time::timeout(STATEMENT_BOUND, notified)
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
    tokio::time::timeout(STATEMENT_BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    // create_stream wakes first; replay is a second await. Drive until the
    // lease is ready. A dead worker fails that replay.
    let opened = tokio::time::timeout(STATEMENT_BOUND, async {
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
    tokio::time::timeout(STATEMENT_BOUND, notified)
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
    tokio::time::timeout(STATEMENT_BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    assert!(matches!(
        read.as_mut().poll(&mut Context::from_waker(Waker::noop())),
        Poll::Ready(Ok(_))
    ));
    tokio::time::timeout(STATEMENT_BOUND, storage.shutdown())
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
    tokio::time::timeout(STATEMENT_BOUND, notified)
        .await
        .unwrap()
        .unwrap();
    tokio::time::timeout(STATEMENT_BOUND, async {
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
