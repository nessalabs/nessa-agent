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
use nessa_client_core::retained::{
    ArtifactFault, ArtifactFaultKind, ArtifactFetch, ArtifactFileStep, ArtifactFiles,
    ArtifactFreshness, ArtifactOpenError, ArtifactRequest, FetchFiles, FileStep,
};
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
        files: Arc::new(OsCacheFiles),
        artifacts: Arc::new(DiskArtifactFiles),
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
    assert!(matches!(
        session.published_catalogue_page(&first, None, 1),
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
    assert!(matches!(
        session.published_catalogue_page(&second, None, 1),
        Ok(None)
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
    assert!(fetched.initial().is_none() && fetched.recheck().is_none());
    assert_eq!(
        fetched.fetched().unwrap().as_ref().unwrap().outcome(),
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
    let admitted = session
        .fetch_artifact(&stamp, &request, ReadStop::new())
        .unwrap();
    assert!(matches!(
        admitted.initial(),
        Some(SessionStatus::ReadOnly(Err(NativeClientError::Io(
            ErrorKind::ConnectionRefused
        ))))
    ));
    assert!(admitted.fetched().is_none() && admitted.recheck().is_none());
    assert_eq!(connector.calls.load(Ordering::SeqCst), 1);
    session.close().unwrap();
}

struct RefusingFiles;
impl ArtifactFiles for RefusingFiles {
    fn open(
        &self,
        _: &ArtifactRequest<'_>,
    ) -> Result<Box<dyn FetchFiles + Send>, ArtifactOpenError> {
        Err(ArtifactOpenError::File(FileStep::Open))
    }
}

#[test]
fn an_injected_held_file_port_is_the_fetch_filesystem() {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let session = compose_files(
        &root,
        Arc::new(OsEntropyFactory),
        Arc::new(OsCacheFiles),
        Arc::new(RefusingFiles),
    )
    .unwrap();
    let stamp = session.bind_saved_receiver().unwrap();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let output = root.path().join("private/not-created.bin");
    let request = ArtifactRequest::new(
        &conversation,
        "sha256:fb8e20fc2e4c3f248c60c39bd652f3c1347298bb977b8b4d5903b85055620603",
        "application/octet-stream",
        2,
        &output,
    );
    let admitted = session
        .fetch_artifact(&stamp, &request, ReadStop::new())
        .unwrap();
    assert!(admitted.initial().is_none() && admitted.recheck().is_none());
    assert_eq!(
        admitted.fetched().unwrap().as_ref().unwrap().outcome(),
        Err(ArtifactFault::progress(
            ArtifactFaultKind::File(ArtifactFileStep::Open),
            0,
            0,
        ))
    );
    assert!(!output.exists());
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
fn a_replacement_session_refuses_the_closed_sessions_stamp() {
    let Opened {
        root,
        state,
        connector,
        session,
    } = opened();
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    let earlier = session.bind_saved_receiver().unwrap();
    session.close().unwrap();
    let replacement = NativeSession::compose(Dependencies {
        state,
        path: root.path().join("private/cache.sqlite"),
        address: "127.0.0.1:9".parse().unwrap(),
        connector: connector.clone(),
        entropy: Arc::new(OsEntropyFactory),
        files: Arc::new(OsCacheFiles),
        artifacts: Arc::new(DiskArtifactFiles),
        wall: Arc::new(Wall),
        clock: Arc::new(Monotonic::new()),
        pause: Arc::new(ThreadPause),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
        product: ProductDependencies::default(),
    })
    .unwrap();
    let current = replacement.bind_saved_receiver().unwrap();
    assert_eq!(current.generation(), earlier.generation());
    assert_eq!(current.receiver(), earlier.receiver());
    assert_ne!(current.lease(), earlier.lease());
    assert!(matches!(
        replacement.published_transcript(&earlier, &conversation, Uuid::nil()),
        Err(SessionFailure::Fenced(PhoneFence::Replaced))
    ));
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    assert!(matches!(
        replacement.published_transcript(&current, &conversation, Uuid::nil()),
        Ok(RetainedTranscript::NotLoaded)
    ));
    replacement.close().unwrap();
}

struct StampEntropy([u8; 16]);
impl RngCore for StampEntropy {
    fn next_u32(&mut self) -> u32 {
        0
    }
    fn next_u64(&mut self) -> u64 {
        0
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.try_fill_bytes(dest).unwrap();
    }
    fn try_fill_bytes(&mut self, dest: &mut [u8]) -> Result<(), EntropyError> {
        for (index, byte) in dest.iter_mut().enumerate() {
            let step = u8::try_from(index / self.0.len()).unwrap_or(u8::MAX);
            *byte = self.0[index % self.0.len()].wrapping_add(step);
        }
        Ok(())
    }
}
impl CryptoRng for StampEntropy {}
struct StampFactory([u8; 16]);
impl EnrollmentEntropyFactory for StampFactory {
    fn create(&self) -> Box<dyn EnrollmentEntropy> {
        Box::new(StampEntropy(self.0))
    }
}
struct DryEntropy;
impl RngCore for DryEntropy {
    fn next_u32(&mut self) -> u32 {
        0
    }
    fn next_u64(&mut self) -> u64 {
        0
    }
    fn fill_bytes(&mut self, dest: &mut [u8]) {
        self.try_fill_bytes(dest).unwrap();
    }
    fn try_fill_bytes(&mut self, _: &mut [u8]) -> Result<(), EntropyError> {
        Err(EntropyError::from(
            std::num::NonZeroU32::new(EntropyError::CUSTOM_START).unwrap(),
        ))
    }
}
impl CryptoRng for DryEntropy {}
struct DryFactory;
impl EnrollmentEntropyFactory for DryFactory {
    fn create(&self) -> Box<dyn EnrollmentEntropy> {
        Box::new(DryEntropy)
    }
}
fn compose_with(
    root: &PrivateFixture,
    entropy: Arc<dyn EnrollmentEntropyFactory>,
) -> Result<NativeSession, SessionFailure> {
    compose_files(
        root,
        entropy,
        Arc::new(OsCacheFiles),
        Arc::new(DiskArtifactFiles),
    )
}
fn compose_files(
    root: &PrivateFixture,
    entropy: Arc<dyn EnrollmentEntropyFactory>,
    files: Arc<dyn CacheFiles>,
    artifacts: Arc<dyn ArtifactFiles>,
) -> Result<NativeSession, SessionFailure> {
    let state = Arc::new(FilePairingState::open(root.path(), Path::new("private")).unwrap());
    issued(&state, "issued-a", "receiver-a");
    NativeSession::compose(Dependencies {
        state,
        path: root.path().join("private/cache.sqlite"),
        address: "127.0.0.1:9".parse().unwrap(),
        connector: Arc::new(Refusing {
            calls: AtomicUsize::new(0),
        }),
        entropy,
        files,
        artifacts,
        wall: Arc::new(Wall),
        clock: Arc::new(Monotonic::new()),
        pause: Arc::new(ThreadPause),
        runtime: Builder::new_current_thread().enable_all().build().unwrap(),
        product: ProductDependencies::default(),
    })
}

#[test]
fn the_publication_lease_is_the_identity_composition_injected() {
    let first = PrivateFixture::new();
    let second = PrivateFixture::new();
    create_directory_beneath(first.path(), Path::new("private")).unwrap();
    create_directory_beneath(second.path(), Path::new("private")).unwrap();
    let instance = [9u8; 16];
    let session = compose_with(&first, Arc::new(StampFactory(instance))).unwrap();
    let stamp = session.bind_saved_receiver().unwrap();
    assert_eq!(stamp.lease().as_bytes(), &instance);
    let other = compose_with(&second, Arc::new(StampFactory([4u8; 16]))).unwrap();
    let rebound = other.bind_saved_receiver().unwrap();
    assert_eq!(rebound.lease().as_bytes(), &[4u8; 16]);
    let conversation = ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap();
    assert!(matches!(
        other.published_transcript(&stamp, &conversation, Uuid::nil()),
        Err(SessionFailure::Fenced(PhoneFence::Replaced))
    ));
    session.close().unwrap();
    other.close().unwrap();
}

struct BlindFiles;
impl CacheFiles for BlindFiles {
    fn presence(&self, _: &Path) -> Result<CacheFilePresence, std::io::Error> {
        Err(std::io::Error::new(
            std::io::ErrorKind::PermissionDenied,
            "cache metadata",
        ))
    }
}

#[test]
fn a_refused_artifact_is_an_authority_failure_and_a_transport_failure_is_not() {
    let refused = ArtifactFetch::decided(
        Err(ArtifactFault::progress(
            ArtifactFaultKind::Gateway(ReadFailure::Refused),
            0,
            0,
        )),
        1,
    );
    assert!(super::artifact_authority_refused(refused.outcome()));
    let unreachable = ArtifactFetch::decided(
        Err(ArtifactFault::progress(
            ArtifactFaultKind::Gateway(ReadFailure::Unreachable),
            1,
            0,
        )),
        1,
    );
    assert!(!super::artifact_authority_refused(unreachable.outcome()));
    let fresh = ArtifactFetch::decided(Ok(ArtifactFreshness::LocalMatch { bytes: 2 }), 1);
    assert!(!super::artifact_authority_refused(fresh.outcome()));
}

#[test]
fn a_cache_file_that_cannot_be_stated_does_not_bind() {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    let session = compose_files(
        &root,
        Arc::new(StampFactory([1u8; 16])),
        Arc::new(BlindFiles),
        Arc::new(DiskArtifactFiles),
    )
    .unwrap();
    assert!(matches!(
        session.bind_saved_receiver(),
        Err(SessionFailure::Cache(ReadFailure::Cache))
    ));
    assert!(matches!(
        session.publication(),
        Err(SessionFailure::Fenced(PhoneFence::Unbound))
    ));
    session.close().unwrap();
}

#[test]
fn a_lease_identity_the_source_refuses_opens_no_session() {
    let root = PrivateFixture::new();
    create_directory_beneath(root.path(), Path::new("private")).unwrap();
    assert!(matches!(
        compose_with(&root, Arc::new(DryFactory)),
        Err(SessionFailure::Enrollment(NativeClientError::Entropy))
    ));
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
    assert!(fetched.initial().is_none() && fetched.recheck().is_none());
    let decided = fetched.fetched().unwrap().as_ref().unwrap();
    assert_eq!(
        decided.outcome(),
        Ok(ArtifactFreshness::LocalMatch { bytes: 2 })
    );
    assert_eq!(decided.elapsed_ms(), 7);
    assert_eq!(connector.calls.load(Ordering::SeqCst), 0);
    session.close().unwrap();
}
