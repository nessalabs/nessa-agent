//! A publication stamp reads only the receiver it was bound under, and a
//! replaced stamp neither reads nor dials.
use super::*;
use crate::test_support::PrivateFixture;
use nessa_auth::{
    adapters::pairing::{FilePairingState, NativeIdentity},
    application::pairing::{ClientPendingStore, PrivateKeyMaterial},
    domain::{
        pairing::{AttemptId, ConsentClass, ConsentIntentId, InvitationId, PublicIntent},
        CredentialId, ResourceId,
    },
};
use nessa_client_core::retained::{ArtifactFreshness, ArtifactRequest};
use nessa_local_storage::create_directory_beneath;
use std::fs;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};

struct Refusing {
    calls: AtomicUsize,
}
impl EnrollmentConnector for Refusing {
    fn connect(&self, _: SocketAddr) -> Result<TcpStream, NativeClientError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Err(NativeClientError::Io(ErrorKind::ConnectionRefused))
    }
}
fn intent() -> PublicIntent {
    PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        u64::MAX,
        u64::MAX,
        ConsentClass::DeviceRead,
    )
    .unwrap()
}
fn identity(seed: u8) -> NativeIdentity {
    NativeIdentity::restore(PrivateKeyMaterial::new(zeroize::Zeroizing::new([seed; 32]))).unwrap()
}
fn issued(state: &FilePairingState, credential: &str, receiver: &str) {
    state
        .save_pending(
            identity(7).key_material(),
            &identity(8).public_spki(),
            intent(),
            None,
        )
        .unwrap();
    state
        .save_credential(
            &CredentialId::new(credential).unwrap(),
            &ResourceId::new(receiver).unwrap(),
            intent(),
        )
        .unwrap();
}

struct Opened {
    root: PrivateFixture,
    state: Arc<FilePairingState>,
    connector: Arc<Refusing>,
    session: NativeSession,
}
struct Steps(AtomicU64);
impl Clock for Steps {
    fn elapsed_ms(&self) -> u64 {
        self.0.fetch_add(7, Ordering::SeqCst)
    }
}
fn opened() -> Opened {
    compose_opened(Arc::new(Monotonic::new()))
}
fn compose_opened(clock: Arc<dyn Clock>) -> Opened {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let state = Arc::new(FilePairingState::open(root.path(), Path::new("private")).unwrap());
    issued(&state, "issued-a", "receiver-a");
    let connector = Arc::new(Refusing {
        calls: AtomicUsize::new(0),
    });
    let session = NativeSession::compose(Dependencies {
        state: state.clone(),
        path: root.path().join("private/cache.sqlite"),
        address: "127.0.0.1:9".parse().unwrap(),
        connector: connector.clone(),
        entropy: Arc::new(OsEntropyFactory),
        wall: Arc::new(Wall),
        clock,
        pause: Arc::new(ThreadPause),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
        product: ProductDependencies::default(),
    })
    .unwrap();
    Opened {
        root,
        state,
        connector,
        session,
    }
}

#[test]
fn a_replaced_receiver_stamp_reads_nothing_and_dials_nothing() {
    let Opened {
        root,
        state,
        connector,
        session,
    } = opened();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let cache = root.path().join("private/cache.sqlite");
    let first = session.bind_saved_receiver().unwrap();
    assert_eq!(session.bind_saved_receiver().unwrap(), first);
    state.end_enrollment(intent()).unwrap();
    issued(&state, "issued-b", "receiver-b");
    assert!(matches!(
        session.published_transcript(&first, &conversation, Uuid::nil()),
        Err(SessionFailure::Fenced(PhoneFence::Replaced))
    ));
    assert!(!cache.exists(), "a replaced stamp opens no cache");
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);

    let second = session.bind_saved_receiver().unwrap();
    assert_ne!(second.generation(), first.generation());
    assert!(matches!(
        session.published_transcript(&second, &conversation, Uuid::nil()),
        Ok(RetainedTranscript::NotLoaded)
    ));
    let output = root.path().join("private/held.bin");
    fs::write(&output, b"ab").unwrap();
    let digest = "sha256:fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603";
    let request = ArtifactRequest::new(
        &conversation,
        digest,
        "application/octet-stream",
        2,
        &output,
    );
    let fetched = session
        .fetch_artifact(&second, &request, ReadStop::new())
        .unwrap();
    assert_eq!(
        fetched.outcome(),
        Ok(ArtifactFreshness::LocalMatch { bytes: 2 })
    );
    assert!(matches!(
        session.fetch_artifact(&first, &request, ReadStop::new()),
        Err(SessionFailure::Fenced(PhoneFence::Replaced))
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    session.close().unwrap();
    assert_eq!(session.publication(), Err(SessionFailure::Closed));
}

#[test]
fn a_missing_file_reports_the_connector_failure() {
    let Opened {
        root,
        connector,
        session,
        ..
    } = opened();
    let stamp = session.bind_saved_receiver().unwrap();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let output = root.path().join("private/missing.bin");
    let request = ArtifactRequest::new(
        &conversation,
        "sha256:fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603",
        "application/octet-stream",
        2,
        &output,
    );
    assert!(matches!(
        session.fetch_artifact(&stamp, &request, ReadStop::new()),
        Err(SessionFailure::Enrollment(NativeClientError::Io(
            ErrorKind::ConnectionRefused
        )))
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
    session.close().unwrap();
}

#[test]
fn a_successful_purge_releases_the_publication_without_an_observation() {
    let Opened {
        root: _root,
        session,
        ..
    } = opened();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let first = session.bind_saved_receiver().unwrap();
    {
        let mut operation = session.enter(None).unwrap();
        operation.driver().note_publication(None, false);
    }
    assert_eq!(session.publication().unwrap(), first);
    {
        let mut operation = session.enter(None).unwrap();
        operation.driver().note_publication(None, true);
    }
    assert!(matches!(
        session.publication(),
        Err(SessionFailure::Fenced(PhoneFence::Unbound))
    ));
    let second = session.bind_saved_receiver().unwrap();
    assert_ne!(second.generation(), first.generation());
    assert!(matches!(
        session.published_transcript(&first, &conversation, Uuid::nil()),
        Err(SessionFailure::Fenced(PhoneFence::Replaced))
    ));
    session.close().unwrap();
}

#[test]
fn a_local_match_reports_the_session_clock() {
    let Opened {
        root,
        connector,
        session,
        ..
    } = compose_opened(Arc::new(Steps(AtomicU64::new(0))));
    let stamp = session.bind_saved_receiver().unwrap();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let output = root.path().join("private/held.bin");
    fs::write(&output, b"ab").unwrap();
    let request = ArtifactRequest::new(
        &conversation,
        "sha256:fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603",
        "application/octet-stream",
        2,
        &output,
    );
    let fetched = session
        .fetch_artifact(&stamp, &request, ReadStop::new())
        .unwrap();
    assert_eq!(
        fetched.outcome(),
        Ok(ArtifactFreshness::LocalMatch { bytes: 2 })
    );
    assert_eq!(fetched.elapsed_ms(), 7);
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    session.close().unwrap();
}
