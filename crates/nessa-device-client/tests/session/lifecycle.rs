//! Admission and scope lifetime use actual private state and held connector effects.
use super::*;
use crate::test_support::PrivateFixture;
use nessa_local_storage::create_directory_beneath;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Condvar,
    },
    thread,
};

struct Refusing {
    calls: AtomicUsize,
}
impl EnrollmentConnector for Refusing {
    fn connect(&self, _: SocketAddr) -> Result<TcpStream, NativeClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    }
}
struct Held {
    state: Mutex<(bool, bool)>,
    changed: Condvar,
}
impl Held {
    fn new() -> Self {
        Self {
            state: Mutex::new((false, false)),
            changed: Condvar::new(),
        }
    }
    fn entered(&self) {
        let held = self.state.lock().unwrap();
        let (held, timeout) = self
            .changed
            .wait_timeout_while(held, Duration::from_secs(3), |value| !value.0)
            .unwrap();
        assert!(
            !timeout.timed_out() && held.0,
            "connector admission arrived"
        );
    }
    fn release(&self) {
        self.state.lock().unwrap().1 = true;
        self.changed.notify_all();
    }
}
impl EnrollmentConnector for Held {
    fn connect(&self, _: SocketAddr) -> Result<TcpStream, NativeClientError> {
        let mut held = self.state.lock().unwrap();
        held.0 = true;
        self.changed.notify_all();
        while !held.1 {
            held = self.changed.wait(held).unwrap();
        }
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    }
}
fn root() -> PrivateFixture {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    root
}
fn compose(root: &Path, connector: Arc<dyn EnrollmentConnector>) -> NativeSession {
    NativeSession::compose(Dependencies {
        state: Arc::new(FilePairingState::open(root, Path::new("private")).unwrap()),
        path: root.join("private/cache.sqlite"),
        address: "127.0.0.1:9".parse().unwrap(),
        connector,
        entropy: Arc::new(OsEntropyFactory),
        wall: Arc::new(Wall),
        clock: Arc::new(Monotonic::new()),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
    })
    .unwrap()
}
#[test]
fn stopped_before_dispatch_opens_no_connection_or_cache() {
    let root = root();
    let connector = Arc::new(Refusing {
        calls: AtomicUsize::new(0),
    });
    let session = compose(root.path(), connector.clone());
    let stop = ReadStop::new();
    stop.stop();
    assert!(matches!(
        session.refresh(stop),
        Err(SessionFailure::Stopped)
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("private/cache.sqlite").exists());
    session.close().unwrap();
}
#[test]
fn saved_selection_without_issued_receiver_does_not_create_cache_or_dial() {
    let root = root();
    let connector = Arc::new(Refusing {
        calls: AtomicUsize::new(0),
    });
    let session = compose(root.path(), connector.clone());
    assert!(matches!(
        session.catalogue_page(None, 1),
        Err(SessionFailure::NoCredential)
    ));
    assert!(matches!(
        session.transcript(
            &ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            Uuid::nil()
        ),
        Err(SessionFailure::NoCredential)
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    assert!(!root.path().join("private/cache.sqlite").exists());
    session.close().unwrap();
}
#[test]
fn unreachable_status_retains_outer_failure_and_does_not_read() {
    let root = root();
    let connector = Arc::new(Refusing {
        calls: AtomicUsize::new(0),
    });
    let session = compose(root.path(), connector.clone());
    let refresh = session.refresh(ReadStop::new()).unwrap();
    assert!(matches!(
        refresh.initial(),
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    ));
    assert!(refresh.read().is_none());
    assert!(refresh.recheck().is_none());
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
    assert!(!root.path().join("private/cache.sqlite").exists());
    session.close().unwrap();
}
#[test]
fn closed_before_dispatch_and_repeated_close_release_private_scope() {
    let root = root();
    let connector = Arc::new(Refusing {
        calls: AtomicUsize::new(0),
    });
    let session = compose(root.path(), connector.clone());
    session.request_close().unwrap();
    assert!(matches!(
        session.refresh(ReadStop::new()),
        Err(SessionFailure::Closed)
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    session.close().unwrap();
    session.close().unwrap();
    let reopened =
        FilePairingState::open(&root.path().canonicalize().unwrap(), Path::new("private")).unwrap();
    drop(reopened);
    assert!(matches!(session.status(), Err(SessionFailure::Closed)));
}
#[test]
fn concurrent_calls_refuse_and_close_waits_for_physical_connector_before_reuse() {
    let root = root();
    let connector = Arc::new(Held::new());
    let session = Arc::new(compose(root.path(), connector.clone()));
    let operation = {
        let session = session.clone();
        thread::spawn(move || session.refresh(ReadStop::new()))
    };
    connector.entered();
    assert!(matches!(session.status(), Err(SessionFailure::Busy)));
    session.request_close().unwrap();
    // Signalling does not release the private lock while a physical call remains.
    assert!(matches!(
        FilePairingState::open(&root.path().canonicalize().unwrap(), Path::new("private")),
        Err(PrivateStateError::Locked)
    ));
    let closed = Arc::new(AtomicUsize::new(0));
    let closer = {
        let session = session.clone();
        let closed = closed.clone();
        thread::spawn(move || {
            session.close().unwrap();
            closed.store(1, Ordering::SeqCst);
        })
    };
    assert_eq!(closed.load(Ordering::SeqCst), 0);
    connector.release();
    let refresh = operation.join().unwrap().unwrap();
    assert!(matches!(
        refresh.initial(),
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    ));
    closer.join().unwrap();
    assert_eq!(closed.load(Ordering::SeqCst), 1);
    let reopened =
        FilePairingState::open(&root.path().canonicalize().unwrap(), Path::new("private")).unwrap();
    drop(reopened);
}
