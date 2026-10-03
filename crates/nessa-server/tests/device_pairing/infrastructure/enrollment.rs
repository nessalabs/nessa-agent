use super::support::{pending, sockets, Fault, FaultyStore, Fixture, Time, NOW_MS, WAIT};
use nessa_auth::{
    adapters::pairing::{
        ClientAttempt, FilePairingState, GatewayTrust, ManualCode, NativeIdentity, NativeTransport,
        OsEntropy, PairingCryptoError,
    },
    application::{
        pairing::{
            ClientPendingStore, OwnerDecision, PairingStore, PairingStoreError, PendingEnrollment,
            PrivateKeyMaterial, PrivateStateError,
        },
        ports::AccessError,
        session::ReadCurrentSession,
    },
    domain::pairing::{
        AttemptFailure, AttemptId, AttemptOutcome, DeviceKey, PairingError, PairingInitiator,
        PairingPhase, PairingPolicy, PairingRecord, PublicIntent, TerminalCause,
    },
};
use nessa_server::{
    app::{dependencies::RuntimeDependencies, ports::Clock as ServerClock},
    device_pairing::{
        application::OwnerError,
        infrastructure::{
            wire::{
                decode_reply, decode_request, encode_challenge, encode_hello, encode_request,
                NativePairingReply, NativePairingRequest, NativePairingStatus,
            },
            BeginPairing, CreatedInvitation, EnrollmentChannel, NativeClientError,
            NativeConnectionError, NativeConnectionFailure, NativeEnrollmentClient,
            NativeEnrollmentConnections, NativeFrameError, PairingRuntimeError,
        },
    },
};
use std::{
    io::ErrorKind,
    net::TcpStream,
    path::Path,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc,
    },
};

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_create_claim_approve_and_reopen_status() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    assert_eq!(created.record().phase(), PairingPhase::Available);
    assert_eq!(
        fixture
            .registry
            .read_pairing(created.record().id())
            .unwrap(),
        *created.record()
    );
    let (address, stop, listener, connections) = fixture.listener().await;
    let (root, pending) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(pending.clone(), RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let claimed = tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(claimed, NativePairingStatus::Claimed(_)));
    let public = claimed.public();
    assert_eq!(public.invitation(), created.record().id());
    let saved = pending.load_pending().unwrap().unwrap();
    assert_eq!(saved.intent(), public);
    assert_eq!(
        *saved.gateway_pin(),
        fixture.gateway.identity().public_spki()
    );
    let record = fixture.registry.read_pairing(public.invitation()).unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    assert_eq!(
        tokio::time::timeout(
            WAIT,
            client.retry(
                TcpStream::connect(address).unwrap(),
                TcpStream::connect(address).unwrap(),
                code,
                OsEntropy
            )
        )
        .await
        .unwrap()
        .unwrap_err(),
        NativeClientError::OriginalNotRetryable
    );
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    assert_eq!(
        client
            .enroll(TcpStream::connect(address).unwrap(), code, OsEntropy)
            .await
            .unwrap_err(),
        NativeClientError::PendingExists
    );
    assert_eq!(pending.load_pending().unwrap().unwrap().intent(), public);
    assert_eq!(
        fixture.registry.read_pairing(public.invitation()).unwrap(),
        record
    );
    let (_, key) = record.claim_binding().unwrap();
    let mut other = *key.bytes();
    other[0] ^= 1;
    assert_eq!(
        fixture
            .gateway
            .decide(
                &fixture.session,
                public.invitation(),
                OwnerDecision::Approve(DeviceKey::new(other))
            )
            .await
            .unwrap_err(),
        PairingRuntimeError::Owner(OwnerError::Enrollment(PairingStoreError::Domain(
            PairingError::Conflict
        )))
    );
    let approved = fixture
        .gateway
        .decide(
            &fixture.session,
            public.invitation(),
            OwnerDecision::Approve(key),
        )
        .await
        .unwrap();
    assert_eq!(approved.phase(), PairingPhase::Approved);
    assert!(approved.credential().is_none());
    let consent = claimed.consent().unwrap().clone();
    client.shutdown().await;
    drop(client);
    drop(saved);
    drop(pending);
    let reopened = Arc::new(FilePairingState::open(&root, Path::new("state")).unwrap());
    let client = NativeEnrollmentClient::new(reopened, RuntimeDependencies::default().clock);
    let status = tokio::time::timeout(
        WAIT,
        client.status(TcpStream::connect(address).unwrap(), Some(consent.clone())),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(status, NativePairingStatus::Approved(Box::new(consent)));
    assert_eq!(
        fixture.registry.read_pairing(public.invitation()).unwrap(),
        approved
    );
    fixture
        .gateway
        .decide(&fixture.session, public.invitation(), OwnerDecision::Cancel)
        .await
        .unwrap();
    assert!(matches!(
        client
            .status(TcpStream::connect(address).unwrap(), None)
            .await
            .unwrap(),
        NativePairingStatus::Terminal { .. }
    ));
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_owner_policy_denial_preserves_session_and_registry() {
    let fixture = Fixture::with_read(false).await;
    let before = fixture.registry.pending_pairings().unwrap();
    let result = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await;
    assert!(matches!(
        result,
        Err(PairingRuntimeError::Owner(OwnerError::Authorization(
            AccessError::Denied
        )))
    ));
    assert_eq!(fixture.registry.pending_pairings().unwrap(), before);
    ReadCurrentSession {
        access: fixture.registry.as_ref(),
        clock: &Time,
    }
    .execute(&fixture.session)
    .await
    .unwrap();
    fixture.gateway.shutdown().await;
    // Actual canonical Cedar with the proposed read grant is the accepted neighbor.
    let allowed = Fixture::new().await;
    assert_eq!(
        allowed
            .gateway
            .create(allowed.session.clone(), OsEntropy)
            .await
            .unwrap()
            .record()
            .phase(),
        PairingPhase::Available
    );
    allowed.gateway.shutdown().await;
}

/// A substitute for the client's private-store port whose save is refused.
struct RefusingSave(Arc<FilePairingState>);
impl ClientPendingStore for RefusingSave {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.0.load_pending()
    }
    fn save_pending(
        &self,
        _: &PrivateKeyMaterial,
        _: &[u8; 44],
        _: PublicIntent,
        _: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        Err(PrivateStateError::Unavailable)
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_pending_save_failure_sends_no_claim_and_retry_recovers() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (_, pending) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(pending.clone(), RuntimeDependencies::default().clock);
    assert_eq!(
        client
            .status(TcpStream::connect(address).unwrap(), None)
            .await
            .unwrap_err(),
        NativeClientError::NoPending
    );
    // The store's port refuses the save that follows KE2; its load still works,
    // so the device reaches KE2 before the save fails.
    let failing = NativeEnrollmentClient::new(
        Arc::new(RefusingSave(pending.clone())),
        RuntimeDependencies::default().clock,
    );
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    assert_eq!(
        tokio::time::timeout(
            WAIT,
            failing.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy)
        )
        .await
        .unwrap()
        .unwrap_err(),
        NativeClientError::Storage(PrivateStateError::Unavailable)
    );
    failing.shutdown().await;
    assert!(pending.load_pending().unwrap().is_none());
    // Collect the actual original server closure before attempting new work.
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    let (address, stop, listener, connections) = fixture.listener().await;
    // The attempt was charged and settled without a claim: no KE3 was sent.
    let settled = fixture
        .registry
        .read_pairing(created.record().id())
        .unwrap();
    assert_eq!(settled.phase(), PairingPhase::Available);
    assert_eq!(settled.charged_attempts(), 1);
    assert!(settled.claim_binding().is_none());
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    assert!(matches!(
        tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy)
        )
        .await
        .unwrap()
        .unwrap(),
        NativePairingStatus::Claimed(_)
    ));
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_claim_reply_loss_preserves_pinned_status() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (_, pending) = pending(fixture.directory.path(), "client-private");
    let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let key = DeviceKey::new(device.public_spki()[12..].try_into().unwrap());
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    stream.set_write_timeout(Some(WAIT)).unwrap();
    let transport =
        NativeTransport::connect(stream, &device, GatewayTrust::ManualBootstrap).unwrap();
    let mut channel = EnrollmentChannel::new(transport);
    let attempt = AttemptId::new([17; AttemptId::LENGTH]);
    channel
        .send_envelope(&encode_request(&NativePairingRequest::Hello(attempt)).unwrap())
        .unwrap();
    let NativePairingReply::Hello(public) =
        decode_reply(&channel.receive_envelope().unwrap()).unwrap()
    else {
        panic!("hello")
    };
    let (state, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    channel
        .send_envelope(&encode_request(&NativePairingRequest::Begin { public, request }).unwrap())
        .unwrap();
    let NativePairingReply::Challenge {
        public: received,
        response,
    } = decode_reply(&channel.receive_envelope().unwrap()).unwrap()
    else {
        panic!("challenge")
    };
    assert_eq!(received, public);
    let context = channel.transport().pairing_context(public).unwrap();
    let message = state
        .finish(
            &mut OsEntropy,
            &code,
            &response,
            &context,
            |authenticated| {
                pending
                    .save_pending(
                        device.key_material(),
                        &authenticated.gateway_spki(),
                        public,
                        None,
                    )
                    .map_err(|_| PairingCryptoError::PendingStorage)
            },
        )
        .unwrap();
    channel
        .send_envelope(&encode_request(&NativePairingRequest::Confirm { public, message }).unwrap())
        .unwrap();
    // Lose the actual claim answer; no supplied claimed/status flag creates evidence.
    drop(channel);
    tokio::time::timeout(WAIT, async {
        loop {
            if fixture
                .registry
                .read_pairing(public.invitation())
                .unwrap()
                .phase()
                == PairingPhase::Claimed
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    let original = fixture.registry.read_pairing(public.invitation()).unwrap();
    assert_eq!(original.claim_binding(), Some((public.attempt(), key)));
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    drop(connections);
    fixture.gateway.shutdown().await;
    let pin = fixture.gateway.identity().public_spki();
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.gateway.identity().public_spki(), pin);
    assert_eq!(
        fixture.registry.read_pairing(public.invitation()).unwrap(),
        original
    );
    let (address, stop, listener, connections) = fixture.listener().await;
    let client = NativeEnrollmentClient::new(pending, RuntimeDependencies::default().clock);
    let recovered = tokio::time::timeout(
        WAIT,
        client.status(TcpStream::connect(address).unwrap(), None),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(recovered, NativePairingStatus::Claimed(_)));
    assert_eq!(recovered.public(), public);
    assert_eq!(
        fixture.registry.read_pairing(public.invitation()).unwrap(),
        original
    );
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_restart_ends_available_setup_and_preserves_key() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let pin = fixture.gateway.identity().public_spki();
    fixture.gateway.shutdown().await;
    let fixture = fixture.reopen().await;
    assert_eq!(fixture.gateway.identity().public_spki(), pin);
    let ended = fixture
        .registry
        .read_pairing(created.record().id())
        .unwrap();
    assert_eq!(ended.phase(), PairingPhase::Terminal);
    assert_eq!(ended.terminal().unwrap().0, TerminalCause::Restarted);
    assert_eq!(
        fixture
            .gateway
            .hello(AttemptId::new([19; AttemptId::LENGTH]))
            .await
            .unwrap_err(),
        PairingRuntimeError::NoInvitation
    );
    let neighbor = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    assert_eq!(neighbor.record().phase(), PairingPhase::Available);
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_status_refuses_another_device_and_accepts_original() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (_, state) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(state, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let claimed = tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    let public = claimed.public();
    let before = fixture.registry.read_pairing(public.invitation()).unwrap();
    let (server, stream) = sockets();
    let owner = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let serving = owner.clone();
    let observer = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
    let pin = fixture.gateway.identity().public_spki();
    let wrong = tokio::task::spawn_blocking(move || {
        let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let transport =
            NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut channel = EnrollmentChannel::new(transport);
        channel
            .send_envelope(&encode_request(&NativePairingRequest::Status(public)).unwrap())
            .unwrap();
        channel.receive_envelope()
    });
    let refusal = tokio::time::timeout(WAIT, observer)
        .await
        .unwrap()
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refusal.failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::Enrollment(
            PairingStoreError::Domain(PairingError::WrongActor)
        ))
    );
    let reply = tokio::time::timeout(WAIT, wrong)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    assert!(matches!(
        decode_reply(&reply).unwrap(),
        NativePairingReply::Refused
    ));
    assert_eq!(
        fixture.registry.read_pairing(public.invitation()).unwrap(),
        before
    );
    assert_eq!(
        tokio::time::timeout(
            WAIT,
            client.status(TcpStream::connect(address).unwrap(), None)
        )
        .await
        .unwrap()
        .unwrap(),
        claimed
    );
    owner.shutdown().await;
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

async fn actual_channel(
    fixture: &Fixture,
    identity: &NativeIdentity,
) -> (NativeTransport<TcpStream>, NativeTransport<TcpStream>) {
    let (server, stream) = sockets();
    let owner = fixture.gateway.clone();
    let accepting = tokio::task::spawn_blocking(move || {
        NativeTransport::accept(server, owner.identity()).unwrap()
    });
    let client = NativeTransport::connect(
        stream,
        identity,
        GatewayTrust::Pinned(fixture.gateway.identity().public_spki()),
    )
    .unwrap();
    (accepting.await.unwrap(), client)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_finish_refuses_another_channel_without_claim() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let (server, client) = actual_channel(&fixture, &identity).await;
    let (other_server, other_client) = actual_channel(&fixture, &identity).await;
    let public = fixture
        .gateway
        .hello(AttemptId::new([21; AttemptId::LENGTH]))
        .await
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (state, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    let BeginPairing::Admitted(handshake) = fixture
        .gateway
        .begin(&server, public, &request, &mut OsEntropy)
        .await
        .unwrap()
    else {
        panic!("actual new attempt")
    };
    let (_, state_file) = pending(fixture.directory.path(), "original-pending");
    let context = client.pairing_context(public).unwrap();
    let message = state
        .finish(
            &mut OsEntropy,
            &code,
            handshake.response(),
            &context,
            |authenticated| {
                state_file
                    .save_pending(
                        identity.key_material(),
                        &authenticated.gateway_spki(),
                        public,
                        None,
                    )
                    .map_err(|_| PairingCryptoError::PendingStorage)
            },
        )
        .unwrap();
    let before = fixture.registry.read_pairing(public.invitation()).unwrap();
    assert_eq!(
        fixture
            .gateway
            .finish(&other_server, *handshake, &message)
            .await
            .unwrap_err(),
        PairingRuntimeError::Crypto(PairingCryptoError::InvalidContext)
    );
    assert_eq!(
        fixture.registry.read_pairing(public.invitation()).unwrap(),
        before
    );
    assert!(before.claim_binding().is_none());
    assert_eq!(state_file.load_pending().unwrap().unwrap().intent(), public);
    // A channel accepted under another gateway key is not this runtime's channel.
    let foreign = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let (stray, stray_client) = sockets();
    let foreign_pin = foreign.public_spki();
    let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let connecting = tokio::task::spawn_blocking(move || {
        NativeTransport::connect(stray_client, &device, GatewayTrust::Pinned(foreign_pin)).unwrap()
    });
    let stray =
        tokio::task::spawn_blocking(move || NativeTransport::accept(stray, &foreign).unwrap())
            .await
            .unwrap();
    let stray_client = connecting.await.unwrap();
    assert!(matches!(
        fixture.gateway.status(&stray, public).await,
        Err(PairingRuntimeError::Crypto(
            PairingCryptoError::InvalidContext
        ))
    ));
    drop(stray);
    drop(stray_client);
    drop(server);
    drop(client);
    drop(other_server);
    drop(other_client);
    // Explicit current owner cancellation settles this interrupted original;
    // a new actual listener exchange is the accepted same-channel neighbor.
    fixture
        .gateway
        .decide(&fixture.session, public.invitation(), OwnerDecision::Cancel)
        .await
        .unwrap();
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (_, pending) = pending(fixture.directory.path(), "neighbor-pending");
    let client = NativeEnrollmentClient::new(pending, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    assert!(matches!(
        tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy)
        )
        .await
        .unwrap()
        .unwrap(),
        NativePairingStatus::Claimed(_)
    ));
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

/// One complete client/server exchange on a fresh socket pair, returning both
/// sides after the server worker has ended, so the next attempt starts after
/// this one has been settled.
async fn one_exchange(
    fixture: &Fixture,
    client: &NativeEnrollmentClient,
    code: ManualCode,
) -> (
    Result<NativePairingStatus, NativeClientError>,
    Result<(), NativeConnectionError>,
) {
    let (server, stream) = sockets();
    let connections = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let serving = connections.clone();
    let served = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
    let enrolled = tokio::time::timeout(WAIT, client.enroll(stream, code, OsEntropy))
        .await
        .unwrap();
    let served = tokio::time::timeout(WAIT, served).await.unwrap().unwrap();
    connections.shutdown().await;
    (enrolled, served)
}

fn other_code(created: &CreatedInvitation) -> ManualCode {
    loop {
        let code = ManualCode::generate(&mut OsEntropy);
        if code.expose_bytes() != created.code().expose_bytes() {
            return code;
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_wrong_codes_are_charged_within_the_attempt_bound() {
    let fixture = Fixture::new().await;
    let (_, state) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(state.clone(), RuntimeDependencies::default().clock);
    let bound = usize::from(PairingPolicy::initial().attempts());

    // Wrong codes short of the bound leave the invitation open for the right one.
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    for charged in 1..bound {
        let (enrolled, served) = one_exchange(&fixture, &client, other_code(&created)).await;
        assert_eq!(
            enrolled.unwrap_err(),
            NativeClientError::Crypto(PairingCryptoError::InvalidProof)
        );
        // The device sends no KE3 for a code that fails PAKE, so the gateway
        // settles the attempt as a closed connection, not as a claim.
        let served = served.unwrap_err();
        assert!(matches!(served.failure, NativeConnectionFailure::Io(_)));
        assert_eq!(served.settlement, None);
        let record = fixture.registry.read_pairing(id).unwrap();
        assert_eq!(record.phase(), PairingPhase::Available);
        assert_eq!(record.charged_attempts(), charged);
        assert!(record.claim_binding().is_none());
        assert!(state.load_pending().unwrap().is_none());
    }
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, served) = one_exchange(&fixture, &client, code).await;
    assert!(matches!(enrolled.unwrap(), NativePairingStatus::Claimed(_)));
    served.unwrap();
    assert_eq!(
        fixture
            .registry
            .read_pairing(id)
            .unwrap()
            .charged_attempts(),
        bound
    );

    // Once every attempt is charged, even the right code is refused before PAKE.
    let (_, state) = pending(fixture.directory.path(), "second-client-private");
    let client = NativeEnrollmentClient::new(state.clone(), RuntimeDependencies::default().clock);
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    for _ in 0..bound {
        let (enrolled, _) = one_exchange(&fixture, &client, other_code(&created)).await;
        assert_eq!(
            enrolled.unwrap_err(),
            NativeClientError::Crypto(PairingCryptoError::InvalidProof)
        );
    }
    let exhausted = fixture.registry.read_pairing(id).unwrap();
    assert_eq!(exhausted.charged_attempts(), bound);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, served) = one_exchange(&fixture, &client, code).await;
    assert_eq!(enrolled.unwrap_err(), NativeClientError::Refused);
    assert_eq!(
        served.unwrap_err().failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::Enrollment(
            PairingStoreError::Domain(PairingError::AttemptsExhausted)
        ))
    );
    assert_eq!(fixture.registry.read_pairing(id).unwrap(), exhausted);
    assert!(state.load_pending().unwrap().is_none());
    client.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_expired_and_used_codes_are_refused_without_a_claim() {
    let fixture = Fixture::new().await;
    let (_, first) = pending(fixture.directory.path(), "first-private");
    let first = NativeEnrollmentClient::new(first, RuntimeDependencies::default().clock);
    let (_, second) = pending(fixture.directory.path(), "second-private");
    let second_client =
        NativeEnrollmentClient::new(second.clone(), RuntimeDependencies::default().clock);

    // A code is used once: a second device presenting it after the claim is refused.
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, _) = one_exchange(&fixture, &first, code).await;
    assert!(matches!(enrolled.unwrap(), NativePairingStatus::Claimed(_)));
    let claimed = fixture
        .registry
        .read_pairing(created.record().id())
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, served) = one_exchange(&fixture, &second_client, code).await;
    assert_eq!(enrolled.unwrap_err(), NativeClientError::Refused);
    assert_eq!(
        served.unwrap_err().failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::NoInvitation)
    );
    assert_eq!(
        fixture
            .registry
            .read_pairing(created.record().id())
            .unwrap(),
        claimed
    );
    assert!(second.load_pending().unwrap().is_none());

    // Expiry is exclusive: the last millisecond still opens, the deadline does not.
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let expires = created.record().expires_at_ms();
    fixture.time.set(expires - 1);
    assert_eq!(
        fixture
            .gateway
            .hello(AttemptId::new([23; AttemptId::LENGTH]))
            .await
            .unwrap()
            .invitation(),
        id
    );
    fixture.time.set(expires);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, served) = one_exchange(&fixture, &second_client, code).await;
    assert_eq!(enrolled.unwrap_err(), NativeClientError::Refused);
    assert_eq!(
        served.unwrap_err().failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::NoInvitation)
    );
    let expired = fixture.registry.read_pairing(id).unwrap();
    assert_eq!(expired.phase(), PairingPhase::Terminal);
    assert_eq!(expired.terminal().unwrap().0, TerminalCause::Expired);
    assert_eq!(expired.charged_attempts(), 0);
    assert!(second.load_pending().unwrap().is_none());
    first.shutdown().await;
    second_client.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_owner_discovers_unfinished_enrollments_through_current_policy() {
    let fixture = Fixture::short_lived_owner().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let listed = fixture.gateway.pending(&fixture.session).await.unwrap();
    assert_eq!(&*listed, std::slice::from_ref(created.record()));
    let (_, state) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(state, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, _) = one_exchange(&fixture, &client, code).await;
    assert!(matches!(enrolled.unwrap(), NativePairingStatus::Claimed(_)));
    let claimed = fixture.registry.read_pairing(id).unwrap();
    assert_eq!(
        &*fixture.gateway.pending(&fixture.session).await.unwrap(),
        std::slice::from_ref(&claimed)
    );
    assert_eq!(
        fixture
            .gateway
            .owner_status(&fixture.session, id)
            .await
            .unwrap(),
        claimed
    );
    // Past the owner credential's expiry (500 s) but before the invitation's,
    // current access is withdrawn and the stored enrollment is unchanged.
    assert!(claimed.expires_at_ms() > 600_000);
    fixture.time.set(600_000);
    assert_eq!(
        fixture
            .gateway
            .owner_status(&fixture.session, id)
            .await
            .unwrap_err(),
        PairingRuntimeError::Owner(OwnerError::Authorization(AccessError::CredentialExpired))
    );
    assert_eq!(
        fixture.gateway.pending(&fixture.session).await.unwrap_err(),
        PairingRuntimeError::Owner(OwnerError::Authorization(AccessError::CredentialExpired))
    );
    assert_eq!(fixture.registry.read_pairing(id).unwrap(), claimed);
    fixture.time.set(NOW_MS);
    fixture
        .gateway
        .decide(&fixture.session, id, OwnerDecision::Cancel)
        .await
        .unwrap();
    assert!(fixture
        .gateway
        .pending(&fixture.session)
        .await
        .unwrap()
        .is_empty());
    client.shutdown().await;
    fixture.gateway.shutdown().await;
}

/// The server's monotonic clock, frozen until a test moves it.
struct Elapsed(AtomicU64);
impl ServerClock for Elapsed {
    fn elapsed_ms(&self) -> u64 {
        self.0.load(Ordering::SeqCst)
    }
}

/// Run a device through Hello, Begin and KE2 on `stream`, saving its pending
/// record, and return the channel with the KE3 it has not yet sent.
fn ready_to_confirm(
    stream: TcpStream,
    attempt: AttemptId,
    code: &ManualCode,
    store: &FilePairingState,
) -> (
    EnrollmentChannel<TcpStream>,
    PublicIntent,
    DeviceKey,
    Vec<u8>,
) {
    let device = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let key = DeviceKey::new(device.public_spki()[12..].try_into().unwrap());
    let transport =
        NativeTransport::connect(stream, &device, GatewayTrust::ManualBootstrap).unwrap();
    let mut channel = EnrollmentChannel::new(transport);
    channel
        .send_envelope(&encode_request(&NativePairingRequest::Hello(attempt)).unwrap())
        .unwrap();
    let NativePairingReply::Hello(public) =
        decode_reply(&channel.receive_envelope().unwrap()).unwrap()
    else {
        panic!("hello")
    };
    let (state, request) = ClientAttempt::start(&mut OsEntropy, code).unwrap();
    channel
        .send_envelope(&encode_request(&NativePairingRequest::Begin { public, request }).unwrap())
        .unwrap();
    let NativePairingReply::Challenge { response, .. } =
        decode_reply(&channel.receive_envelope().unwrap()).unwrap()
    else {
        panic!("challenge")
    };
    let context = channel.transport().pairing_context(public).unwrap();
    let message = state
        .finish(&mut OsEntropy, code, &response, &context, |authenticated| {
            store
                .save_pending(
                    device.key_material(),
                    &authenticated.gateway_spki(),
                    public,
                    None,
                )
                .map_err(|_| PairingCryptoError::PendingStorage)
        })
        .unwrap();
    (channel, public, key, message)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_late_confirmation_is_settled_as_deadline_without_claim() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    // The enrollment phase is thirty seconds from the end of TLS, on the
    // injected clock. KE3 sent at the deadline is refused; one millisecond
    // earlier it claims.
    for (sent_at_ms, claims, attempt) in [(30_000, false, 29), (29_999, true, 31)] {
        let clock = Arc::new(Elapsed(AtomicU64::new(0)));
        let connections = Arc::new(NativeEnrollmentConnections::new(
            fixture.gateway.clone(),
            clock.clone(),
        ));
        let (server, stream) = sockets();
        let serving = connections.clone();
        let served = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
        let (_, store) = pending(fixture.directory.path(), &format!("client-{sent_at_ms}"));
        let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
        let device = tokio::task::spawn_blocking(move || {
            let (mut channel, public, key, message) = ready_to_confirm(
                stream,
                AttemptId::new([attempt; AttemptId::LENGTH]),
                &code,
                &store,
            );
            clock.0.store(sent_at_ms, Ordering::SeqCst);
            channel
                .send_envelope(
                    &encode_request(&NativePairingRequest::Confirm { public, message }).unwrap(),
                )
                .unwrap();
            (public, key, channel.receive_envelope())
        });
        let served = tokio::time::timeout(WAIT, served).await.unwrap().unwrap();
        let (public, key, reply) = tokio::time::timeout(WAIT, device).await.unwrap().unwrap();
        let record = fixture.registry.read_pairing(id).unwrap();
        if claims {
            served.unwrap();
            assert!(matches!(
                decode_reply(&reply.unwrap()).unwrap(),
                NativePairingReply::Status(NativePairingStatus::Claimed(_))
            ));
            assert_eq!(record.claim_binding(), Some((public.attempt(), key)));
        } else {
            let refusal = served.unwrap_err();
            assert_eq!(
                refusal.failure,
                NativeConnectionFailure::Io(ErrorKind::TimedOut)
            );
            assert_eq!(refusal.settlement, None);
            assert!(reply.is_err());
            assert_eq!(record.phase(), PairingPhase::Available);
            assert_eq!(
                record.attempt_status(public.attempt(), key).unwrap(),
                AttemptOutcome::Failed(AttemptFailure::HandshakeDeadline)
            );
        }
        connections.shutdown().await;
    }
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_retry_refuses_another_invitation_before_a_new_attempt() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (root, store) = pending(fixture.directory.path(), "client-private");
    // The device saves its pending record, then loses the connection before KE3.
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let saving = store.clone();
    let original = tokio::task::spawn_blocking(move || {
        let (_, public, _, _) = ready_to_confirm(
            stream,
            AttemptId::new([37; AttemptId::LENGTH]),
            &code,
            &saving,
        );
        public
    })
    .await
    .unwrap();
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    // The owner cancels and opens a different invitation.
    fixture
        .gateway
        .decide(
            &fixture.session,
            original.invitation(),
            OwnerDecision::Cancel,
        )
        .await
        .unwrap();
    let replacement = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    drop(store);
    let store = Arc::new(FilePairingState::open(&root, Path::new("state")).unwrap());
    let client = NativeEnrollmentClient::new(store.clone(), RuntimeDependencies::default().clock);
    let code = ManualCode::parse(replacement.code().expose_bytes()).unwrap();
    assert_eq!(
        tokio::time::timeout(
            WAIT,
            client.retry(
                TcpStream::connect(address).unwrap(),
                TcpStream::connect(address).unwrap(),
                code,
                OsEntropy,
            ),
        )
        .await
        .unwrap()
        .unwrap_err(),
        NativeClientError::Phase
    );
    assert_eq!(store.load_pending().unwrap().unwrap().intent(), original);
    assert_eq!(
        fixture
            .registry
            .read_pairing(replacement.record().id())
            .unwrap()
            .charged_attempts(),
        0
    );
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

/// Send `requests` in order on a fresh channel and return the gateway's last
/// reply, or the read error if the gateway closed instead.
async fn exchange_raw(
    fixture: &Fixture,
    requests: Vec<NativePairingRequest>,
) -> (
    Result<NativePairingReply, NativeFrameError>,
    Result<(), NativeConnectionError>,
) {
    let (server, stream) = sockets();
    let connections = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let serving = connections.clone();
    let served = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
    let pin = fixture.gateway.identity().public_spki();
    let device = tokio::task::spawn_blocking(move || {
        let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let transport =
            NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut channel = EnrollmentChannel::new(transport);
        let mut last = Err(NativeFrameError::Io(ErrorKind::NotConnected));
        for request in requests {
            channel
                .send_envelope(&encode_request(&request).unwrap())
                .unwrap();
            last = channel.receive_envelope();
            if last.is_err() {
                break;
            }
        }
        last.map(|bytes| decode_reply(&bytes).unwrap())
    });
    let reply = tokio::time::timeout(WAIT, device).await.unwrap().unwrap();
    let served = tokio::time::timeout(WAIT, served).await.unwrap().unwrap();
    connections.shutdown().await;
    (reply, served)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_out_of_order_requests_are_refused_before_an_attempt_is_charged() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (_, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    let public = fixture
        .gateway
        .hello(AttemptId::new([41; AttemptId::LENGTH]))
        .await
        .unwrap();
    // Begin without Hello, and Begin for an attempt other than the one Hello named.
    for requests in [
        vec![NativePairingRequest::Begin {
            public,
            request: request.clone(),
        }],
        vec![
            NativePairingRequest::Hello(AttemptId::new([43; AttemptId::LENGTH])),
            NativePairingRequest::Begin {
                public,
                request: request.clone(),
            },
        ],
    ] {
        let (reply, served) = exchange_raw(&fixture, requests).await;
        assert!(matches!(reply.unwrap(), NativePairingReply::Refused));
        assert_eq!(served.unwrap_err().failure, NativeConnectionFailure::Phase);
        assert_eq!(
            fixture
                .registry
                .read_pairing(id)
                .unwrap()
                .charged_attempts(),
            0
        );
    }
    // The same Begin after its own Hello is admitted and charged.
    let (reply, _) = exchange_raw(
        &fixture,
        vec![
            NativePairingRequest::Hello(public.attempt()),
            NativePairingRequest::Begin { public, request },
        ],
    )
    .await;
    assert!(matches!(
        reply.unwrap(),
        NativePairingReply::Challenge { .. }
    ));
    assert_eq!(
        fixture
            .registry
            .read_pairing(id)
            .unwrap()
            .charged_attempts(),
        1
    );
    fixture.gateway.shutdown().await;
}

fn expired_by_system(record: &PairingRecord) -> bool {
    record.phase() == PairingPhase::Terminal
        && record.terminal() == Some((TerminalCause::Expired, &PairingInitiator::System))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_expired_code_is_settled_before_the_next_create() {
    let fixture = Fixture::new().await;
    // create: the past-due invitation no longer holds the slot.
    let first = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    fixture.time.set(first.record().expires_at_ms());
    let second = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    assert!(expired_by_system(
        &fixture.registry.read_pairing(first.record().id()).unwrap()
    ));
    // pending: a past-due invitation is settled, not listed.
    fixture.time.set(second.record().expires_at_ms());
    assert!(fixture
        .gateway
        .pending(&fixture.session)
        .await
        .unwrap()
        .is_empty());
    assert!(expired_by_system(
        &fixture.registry.read_pairing(second.record().id()).unwrap()
    ));
    // decide: cancelling a past-due invitation keeps Expired as its cause.
    let third = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    fixture.time.set(third.record().expires_at_ms());
    let decided = fixture
        .gateway
        .decide(&fixture.session, third.record().id(), OwnerDecision::Cancel)
        .await
        .unwrap();
    assert!(expired_by_system(&decided));
    assert_eq!(
        fixture.registry.read_pairing(third.record().id()).unwrap(),
        decided
    );
    // owner status: reads the settled record.
    let fourth = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    fixture.time.set(fourth.record().expires_at_ms());
    assert!(expired_by_system(
        &fixture
            .gateway
            .owner_status(&fixture.session, fourth.record().id())
            .await
            .unwrap()
    ));
    // Before its deadline an invitation is left open.
    let open = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    fixture.time.set(open.record().expires_at_ms() - 1);
    assert_eq!(
        &*fixture.gateway.pending(&fixture.session).await.unwrap(),
        std::slice::from_ref(open.record())
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_claim_losing_to_expiry_is_settled_and_recoverable() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let expires = created.record().expires_at_ms();
    // KE3 arrives at the deadline.
    let connections = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let (server, stream) = sockets();
    let serving = connections.clone();
    let served = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
    let (root, store) = pending(fixture.directory.path(), "client-private");
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let time = fixture.time.clone();
    let saving = store.clone();
    let device = tokio::task::spawn_blocking(move || {
        let attempt = AttemptId::new([47; AttemptId::LENGTH]);
        let (mut channel, public, key, message) = ready_to_confirm(stream, attempt, &code, &saving);
        time.set(expires);
        channel
            .send_envelope(
                &encode_request(&NativePairingRequest::Confirm { public, message }).unwrap(),
            )
            .unwrap();
        (public, key, channel.receive_envelope().unwrap())
    });
    let (public, key, reply) = tokio::time::timeout(WAIT, device).await.unwrap().unwrap();
    assert!(matches!(
        decode_reply(&reply).unwrap(),
        NativePairingReply::Refused
    ));
    assert_eq!(
        tokio::time::timeout(WAIT, served)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::Enrollment(
            PairingStoreError::Domain(PairingError::Expired)
        ))
    );
    connections.shutdown().await;
    let record = fixture.registry.read_pairing(id).unwrap();
    assert!(expired_by_system(&record));
    assert_eq!(
        record.attempt_status(public.attempt(), key).unwrap(),
        AttemptOutcome::Superseded
    );
    // The device's pinned status reads the settled attempt, so it may retry.
    let (address, stop, listener, connections) = fixture.listener().await;
    drop(store);
    let store = Arc::new(FilePairingState::open(&root, Path::new("state")).unwrap());
    let client = NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock);
    assert_eq!(
        tokio::time::timeout(
            WAIT,
            client.status(TcpStream::connect(address).unwrap(), None)
        )
        .await
        .unwrap()
        .unwrap(),
        NativePairingStatus::Unclaimed {
            public,
            outcome: AttemptOutcome::Superseded,
            terminal: Some(TerminalCause::Expired),
        }
    );
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;

    // Begin arrives at the deadline, after a Hello one millisecond earlier.
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let expires = created.record().expires_at_ms();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (_, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    fixture.time.set(expires - 1);
    let public = fixture
        .gateway
        .hello(AttemptId::new([53; AttemptId::LENGTH]))
        .await
        .unwrap();
    fixture.time.set(expires);
    let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let (server, client) = actual_channel(&fixture, &identity).await;
    assert!(matches!(
        fixture
            .gateway
            .begin(&server, public, &request, &mut OsEntropy)
            .await,
        Err(PairingRuntimeError::Enrollment(PairingStoreError::Domain(
            PairingError::Expired
        )))
    ));
    drop(client);
    let record = fixture.registry.read_pairing(id).unwrap();
    assert!(expired_by_system(&record));
    assert_eq!(record.charged_attempts(), 0);
    assert_eq!(
        fixture
            .gateway
            .hello(AttemptId::new([59; AttemptId::LENGTH]))
            .await
            .unwrap_err(),
        PairingRuntimeError::NoInvitation
    );
    // The slot is free for the next invitation.
    assert_eq!(
        fixture
            .gateway
            .create(fixture.session.clone(), OsEntropy)
            .await
            .unwrap()
            .record()
            .phase(),
        PairingPhase::Available
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_restart_expires_due_claimed_enrollments() {
    let fixture = Fixture::new().await;
    let (address, stop, listener, connections) = fixture.listener().await;
    let mut claimed = Vec::new();
    for name in ["first-private", "second-private"] {
        let created = fixture
            .gateway
            .create(fixture.session.clone(), OsEntropy)
            .await
            .unwrap();
        let (_, store) = pending(fixture.directory.path(), name);
        let client = NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock);
        let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
        let status = tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(status, NativePairingStatus::Claimed(_)));
        claimed.push((created.record().expires_at_ms(), client));
    }
    let latest = claimed.iter().map(|(expires, _)| *expires).max().unwrap();
    fixture.time.set(latest);
    // A device's pinned status settles its own past-due claim.
    let (_, first) = &claimed[0];
    assert!(matches!(
        tokio::time::timeout(
            WAIT,
            first.status(TcpStream::connect(address).unwrap(), None)
        )
        .await
        .unwrap()
        .unwrap(),
        NativePairingStatus::Terminal {
            cause: TerminalCause::Expired,
            ..
        }
    ));
    let second = {
        let (_, client) = &claimed[1];
        client.shutdown().await;
        let first = &claimed[0].1;
        first.shutdown().await;
        fixture.registry.pending_pairings().unwrap()
    };
    // The second claim is untouched until the gateway restarts.
    assert_eq!(second.len(), 1);
    assert_eq!(second[0].phase(), PairingPhase::Claimed);
    let id = second[0].id();
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    drop(connections);
    fixture.gateway.shutdown().await;
    let fixture = fixture.reopen().await;
    assert!(expired_by_system(
        &fixture.registry.read_pairing(id).unwrap()
    ));
    assert!(fixture.registry.pending_pairings().unwrap().is_empty());
    fixture.gateway.shutdown().await;
}

/// What a test device sends in place of its KE3, given the operation and the real KE3.
type InsteadOfKe3 = fn(PublicIntent, Vec<u8>) -> NativePairingRequest;

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_attempt_failures_record_their_cause() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    // What the device sends where KE3 belongs, and the cause the gateway records.
    let cases: [(u8, Option<InsteadOfKe3>, AttemptFailure); 3] = [
        (61, None, AttemptFailure::ConnectionClosed),
        (
            67,
            Some(|public, message| NativePairingRequest::Confirm {
                public,
                message: message.iter().map(|byte| byte ^ 0xff).collect(),
            }),
            AttemptFailure::InvalidProof,
        ),
        (
            71,
            Some(|public, _| NativePairingRequest::Status(public)),
            AttemptFailure::InvalidProof,
        ),
    ];
    for (attempt, instead, cause) in cases {
        let connections = Arc::new(NativeEnrollmentConnections::new(
            fixture.gateway.clone(),
            RuntimeDependencies::default().clock,
        ));
        let (server, stream) = sockets();
        let serving = connections.clone();
        let served = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
        let (_, store) = pending(fixture.directory.path(), &format!("client-{attempt}"));
        let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
        let device = tokio::task::spawn_blocking(move || {
            let (mut channel, public, key, message) = ready_to_confirm(
                stream,
                AttemptId::new([attempt; AttemptId::LENGTH]),
                &code,
                &store,
            );
            if let Some(instead) = instead {
                channel
                    .send_envelope(&encode_request(&instead(public, message)).unwrap())
                    .unwrap();
                channel.receive_envelope().ok();
            }
            (public, key)
        });
        let (public, key) = tokio::time::timeout(WAIT, device).await.unwrap().unwrap();
        let refusal = tokio::time::timeout(WAIT, served)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        assert_eq!(refusal.settlement, None);
        connections.shutdown().await;
        let record = fixture.registry.read_pairing(id).unwrap();
        assert_eq!(
            record.attempt_status(public.attempt(), key).unwrap(),
            AttemptOutcome::Failed(cause)
        );
        assert_eq!(record.phase(), PairingPhase::Available);
    }
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_failed_store_writes_stay_visible() {
    let store = std::sync::OnceLock::new();
    let fixture = Fixture::with_store(|registry| {
        let faulty = FaultyStore::new(registry);
        store.set(faulty.clone()).ok();
        faulty
    })
    .await;
    let store = store.get().unwrap().clone();
    let mut created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let mut id = created.record().id();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();

    // Reserve refused: no PAKE runs and nothing is charged.
    store.refuse(Some(Fault::Reserve));
    let (_, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    let public = fixture
        .gateway
        .hello(AttemptId::new([73; AttemptId::LENGTH]))
        .await
        .unwrap();
    let (reply, served) = exchange_raw(
        &fixture,
        vec![
            NativePairingRequest::Hello(public.attempt()),
            NativePairingRequest::Begin { public, request },
        ],
    )
    .await;
    assert!(matches!(reply.unwrap(), NativePairingReply::Refused));
    assert_eq!(
        served.unwrap_err().failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::Enrollment(
            PairingStoreError::Unavailable
        ))
    );
    assert_eq!(
        fixture
            .registry
            .read_pairing(id)
            .unwrap()
            .charged_attempts(),
        0
    );

    // Settlement refused, on both settlement paths: the primary failure and the
    // settlement failure arrive together, and the attempt stays Pending.
    store.refuse(Some(Fault::Settle));
    for (attempt, garbage) in [(79u8, false), (83, true)] {
        let connections = Arc::new(NativeEnrollmentConnections::new(
            fixture.gateway.clone(),
            RuntimeDependencies::default().clock,
        ));
        let (server, stream) = sockets();
        let serving = connections.clone();
        let served = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
        let (_, state) = pending(fixture.directory.path(), &format!("client-{attempt}"));
        let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
        let device = tokio::task::spawn_blocking(move || {
            let (mut channel, public, key, message) = ready_to_confirm(
                stream,
                AttemptId::new([attempt; AttemptId::LENGTH]),
                &code,
                &state,
            );
            if garbage {
                let message = message.iter().map(|byte| byte ^ 0xff).collect();
                channel
                    .send_envelope(
                        &encode_request(&NativePairingRequest::Confirm { public, message })
                            .unwrap(),
                    )
                    .unwrap();
                channel.receive_envelope().ok();
            }
            (public, key)
        });
        let (public, key) = tokio::time::timeout(WAIT, device).await.unwrap().unwrap();
        let refusal = tokio::time::timeout(WAIT, served)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err();
        connections.shutdown().await;
        if garbage {
            assert_eq!(
                refusal.failure,
                NativeConnectionFailure::Runtime(PairingRuntimeError::Handshake {
                    failure: PairingCryptoError::InvalidProof,
                    settlement: Some(PairingStoreError::Unavailable),
                })
            );
        } else {
            assert!(matches!(refusal.failure, NativeConnectionFailure::Io(_)));
            assert_eq!(refusal.settlement, Some(PairingStoreError::Unavailable));
        }
        assert_eq!(
            fixture
                .registry
                .read_pairing(id)
                .unwrap()
                .attempt_status(public.attempt(), key)
                .unwrap(),
            AttemptOutcome::Pending
        );
        // The owner cancels the stuck attempt so the next case can reserve.
        store.refuse(None);
        fixture
            .gateway
            .decide(&fixture.session, id, OwnerDecision::Cancel)
            .await
            .unwrap();
        created = fixture
            .gateway
            .create(fixture.session.clone(), OsEntropy)
            .await
            .unwrap();
        id = created.record().id();
        store.refuse(Some(Fault::Settle));
    }

    // Claim refused: `Refused`; the attempt stays Pending and the device keeps
    // its pending record for a later pinned status.
    store.refuse(Some(Fault::Claim));
    let (_, state) = pending(fixture.directory.path(), "client-claim");
    let client = NativeEnrollmentClient::new(state.clone(), RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, served) = one_exchange(&fixture, &client, code).await;
    assert_eq!(enrolled.unwrap_err(), NativeClientError::Refused);
    assert_eq!(
        served.unwrap_err().failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::Enrollment(
            PairingStoreError::Unavailable
        ))
    );
    let public = state.load_pending().unwrap().unwrap().intent();
    let record = fixture.registry.read_pairing(id).unwrap();
    assert_eq!(record.phase(), PairingPhase::Available);
    assert_eq!(record.charged_attempts(), 1);
    assert!(record.claim_binding().is_none());
    assert_eq!(public.invitation(), id);
    client.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_committed_claim_is_reported_without_a_later_read() {
    let store = std::sync::OnceLock::new();
    let fixture = Fixture::with_store(|registry| {
        let faulty = FaultyStore::new(registry);
        store.set(faulty.clone()).ok();
        faulty
    })
    .await;
    let store = store.get().unwrap().clone();
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    // Every store read after the claim commits fails.
    store.refuse(Some(Fault::ReadsAfterClaim));
    let (_, state) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(state, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let (enrolled, served) = one_exchange(&fixture, &client, code).await;
    let NativePairingStatus::Claimed(consent) = enrolled.unwrap() else {
        panic!("the committed claim is reported")
    };
    served.unwrap();
    store.refuse(None);
    let record = fixture
        .registry
        .read_pairing(created.record().id())
        .unwrap();
    assert_eq!(record.phase(), PairingPhase::Claimed);
    assert_eq!(
        record.claim_binding().map(|(attempt, _)| attempt),
        Some(consent.public().attempt())
    );
    client.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_client_refuses_a_gateway_that_changes_the_operation() {
    let fixture = Fixture::new().await;
    fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let genuine = fixture
        .gateway
        .hello(AttemptId::new([89; AttemptId::LENGTH]))
        .await
        .unwrap();
    // true: Hello names another attempt. false: Hello is right, Challenge is not.
    for wrong_hello in [true, false] {
        let (gateway_socket, stream) = sockets();
        // A first-contact device accepts any gateway key, so the fake has its own.
        let gateway_identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let fake = tokio::task::spawn_blocking(move || {
            let transport = NativeTransport::accept(gateway_socket, &gateway_identity).unwrap();
            let mut channel = EnrollmentChannel::new(transport);
            let NativePairingRequest::Hello(attempt) =
                decode_request(&channel.receive_envelope().unwrap()).unwrap()
            else {
                panic!("hello")
            };
            let other = AttemptId::new([97; AttemptId::LENGTH]);
            let public = genuine.with_attempt(attempt);
            if wrong_hello {
                channel
                    .send_envelope(&encode_hello(genuine.with_attempt(other)).unwrap())
                    .unwrap();
            } else {
                channel
                    .send_envelope(&encode_hello(public).unwrap())
                    .unwrap();
                decode_request(&channel.receive_envelope().unwrap()).unwrap();
                channel
                    .send_envelope(
                        &encode_challenge(genuine.with_attempt(other), &[1, 2, 3]).unwrap(),
                    )
                    .unwrap();
            }
            // The device sends nothing further.
            channel.receive_envelope().is_err()
        });
        let (_, state) = pending(fixture.directory.path(), &format!("client-{wrong_hello}"));
        let client =
            NativeEnrollmentClient::new(state.clone(), RuntimeDependencies::default().clock);
        let code = ManualCode::generate(&mut OsEntropy);
        assert_eq!(
            tokio::time::timeout(WAIT, client.enroll(stream, code, OsEntropy))
                .await
                .unwrap()
                .unwrap_err(),
            NativeClientError::Phase
        );
        client.shutdown().await;
        assert!(tokio::time::timeout(WAIT, fake).await.unwrap().unwrap());
        assert!(state.load_pending().unwrap().is_none());
    }
    fixture.gateway.shutdown().await;
}

/// The default `#[tokio::test]` runtime has one thread, so a store call made on
/// the async thread would hold the task that releases the gate.
#[tokio::test]
async fn native_owner_store_work_runs_off_the_async_thread() {
    let store = std::sync::OnceLock::new();
    let fixture = Fixture::with_store(|registry| {
        let faulty = FaultyStore::new(registry);
        store.set(faulty.clone()).ok();
        faulty
    })
    .await;
    let store = store.get().unwrap().clone();
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    // Past due, so each command settles an expiry as well as reading.
    fixture.time.set(created.record().expires_at_ms());
    for command in 0..4 {
        let release = store.gate_next_read();
        let releaser = tokio::spawn(async move {
            release.send(()).ok();
        });
        match command {
            0 => {
                fixture.gateway.pending(&fixture.session).await.unwrap();
            }
            1 => {
                fixture
                    .gateway
                    .owner_status(&fixture.session, id)
                    .await
                    .unwrap();
            }
            2 => {
                fixture
                    .gateway
                    .decide(&fixture.session, id, OwnerDecision::Cancel)
                    .await
                    .unwrap();
            }
            _ => {
                fixture
                    .gateway
                    .create(fixture.session.clone(), OsEntropy)
                    .await
                    .unwrap();
            }
        }
        releaser.await.unwrap();
        assert!(
            !store.gate_timed_out(),
            "command {command} read on the async thread"
        );
    }
    assert!(expired_by_system(
        &fixture.registry.read_pairing(id).unwrap()
    ));
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_refused_owner_changes_no_enrollment() {
    let fixture = Fixture::short_lived_owner().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (_, state) = pending(fixture.directory.path(), "client-private");
    let client = NativeEnrollmentClient::new(state, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let claimed = tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    let public = claimed.public();
    // At the invitation's expiry the owner's credential (500 s) has expired too.
    fixture.time.set(created.record().expires_at_ms());
    let before = fixture.registry.read_pairing(id).unwrap();
    assert_eq!(before.phase(), PairingPhase::Claimed);
    let refused =
        PairingRuntimeError::Owner(OwnerError::Authorization(AccessError::CredentialExpired));
    assert_eq!(
        fixture
            .gateway
            .owner_status(&fixture.session, id)
            .await
            .unwrap_err(),
        refused
    );
    assert_eq!(
        fixture.gateway.pending(&fixture.session).await.unwrap_err(),
        refused
    );
    assert_eq!(
        fixture
            .gateway
            .decide(&fixture.session, id, OwnerDecision::Cancel)
            .await
            .unwrap_err(),
        refused
    );
    assert!(matches!(
        fixture
            .gateway
            .create(fixture.session.clone(), OsEntropy)
            .await,
        Err(error) if error == refused
    ));
    assert_eq!(fixture.registry.read_pairing(id).unwrap(), before);
    // A device whose key is not the attempt's is refused and changes nothing.
    let pin = fixture.gateway.identity().public_spki();
    let (server, stream) = sockets();
    let owner = Arc::new(NativeEnrollmentConnections::new(
        fixture.gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    let serving = owner.clone();
    let served = tokio::spawn(async move { serving.serve(server, OsEntropy).await });
    let wrong = tokio::task::spawn_blocking(move || {
        let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
        let transport =
            NativeTransport::connect(stream, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut channel = EnrollmentChannel::new(transport);
        channel
            .send_envelope(&encode_request(&NativePairingRequest::Status(public)).unwrap())
            .unwrap();
        channel.receive_envelope().unwrap()
    });
    assert!(matches!(
        decode_reply(&tokio::time::timeout(WAIT, wrong).await.unwrap().unwrap()).unwrap(),
        NativePairingReply::Refused
    ));
    assert_eq!(
        tokio::time::timeout(WAIT, served)
            .await
            .unwrap()
            .unwrap()
            .unwrap_err()
            .failure,
        NativeConnectionFailure::Runtime(PairingRuntimeError::Enrollment(
            PairingStoreError::Domain(PairingError::WrongActor)
        ))
    );
    owner.shutdown().await;
    assert_eq!(fixture.registry.read_pairing(id).unwrap(), before);
    // The attempt's own device settles the expiry and reads it.
    assert!(matches!(
        tokio::time::timeout(
            WAIT,
            client.status(TcpStream::connect(address).unwrap(), None)
        )
        .await
        .unwrap()
        .unwrap(),
        NativePairingStatus::Terminal {
            cause: TerminalCause::Expired,
            ..
        }
    ));
    assert!(expired_by_system(
        &fixture.registry.read_pairing(id).unwrap()
    ));
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_restart_records_expiry_before_restart() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let open = fixture.registry.read_pairing(id).unwrap();
    // Not yet due: a restart still ends it Restarted.
    fixture.time.set(open.expires_at_ms() - 1);
    fixture.gateway.shutdown().await;
    let fixture = fixture.reopen().await;
    assert_eq!(
        fixture.registry.read_pairing(id).unwrap().terminal(),
        Some((TerminalCause::Restarted, &PairingInitiator::System))
    );
    // Already due when the gateway restarts: Expired is the first cause.
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    fixture.time.set(created.record().expires_at_ms() + 10);
    fixture.gateway.shutdown().await;
    let fixture = fixture.reopen().await;
    assert!(expired_by_system(
        &fixture.registry.read_pairing(id).unwrap()
    ));
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_past_due_conflicting_reservation_settles_expiry() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let id = created.record().id();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let public = fixture
        .gateway
        .hello(AttemptId::new([101; AttemptId::LENGTH]))
        .await
        .unwrap();
    let first = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let (server, _client) = actual_channel(&fixture, &first).await;
    let (_, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    assert!(matches!(
        fixture
            .gateway
            .begin(&server, public, &request, &mut OsEntropy)
            .await,
        Ok(BeginPairing::Admitted(_))
    ));
    // Past due, another device replays the same attempt: Auth refuses the
    // replay as a conflict before it checks expiry.
    fixture.time.set(created.record().expires_at_ms());
    let second = NativeIdentity::generate(&mut OsEntropy).unwrap();
    let (other, _other_client) = actual_channel(&fixture, &second).await;
    let (_, request) = ClientAttempt::start(&mut OsEntropy, &code).unwrap();
    assert!(matches!(
        fixture
            .gateway
            .begin(&other, public, &request, &mut OsEntropy)
            .await,
        Err(PairingRuntimeError::Enrollment(PairingStoreError::Domain(
            PairingError::Conflict
        )))
    ));
    let record = fixture.registry.read_pairing(id).unwrap();
    assert!(expired_by_system(&record));
    let key = DeviceKey::new(first.public_spki()[12..].try_into().unwrap());
    assert_eq!(
        record.attempt_status(public.attempt(), key).unwrap(),
        AttemptOutcome::Superseded
    );
    fixture.gateway.shutdown().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_ended_invitation_setup_is_discarded() {
    let store = std::sync::OnceLock::new();
    let fixture = Fixture::with_store(|registry| {
        let faulty = FaultyStore::new(registry);
        store.set(faulty.clone()).ok();
        faulty
    })
    .await;
    let store = store.get().unwrap().clone();
    let hello = || {
        fixture
            .gateway
            .hello(AttemptId::new([103; AttemptId::LENGTH]))
    };
    // With store reads failing, Hello answers NoInvitation without a read only
    // when the setup is gone; with a setup present it reports the read failure.
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    store.refuse(Some(Fault::Reads));
    assert_eq!(
        hello().await.unwrap_err(),
        PairingRuntimeError::Enrollment(PairingStoreError::Unavailable)
    );
    store.refuse(None);
    // An owner command that settles the expiry drops the setup.
    fixture.time.set(created.record().expires_at_ms());
    fixture
        .gateway
        .owner_status(&fixture.session, created.record().id())
        .await
        .unwrap();
    store.refuse(Some(Fault::Reads));
    assert_eq!(
        hello().await.unwrap_err(),
        PairingRuntimeError::NoInvitation
    );
    store.refuse(None);

    // A device status that settles the expiry drops the setup.
    let created = fixture
        .gateway
        .create(fixture.session.clone(), OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, connections) = fixture.listener().await;
    let (root, state) = pending(fixture.directory.path(), "client-private");
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let stream = TcpStream::connect(address).unwrap();
    stream.set_read_timeout(Some(WAIT)).unwrap();
    let saving = state.clone();
    // The device saves its pending record and disconnects before KE3.
    tokio::task::spawn_blocking(move || {
        ready_to_confirm(
            stream,
            AttemptId::new([107; AttemptId::LENGTH]),
            &code,
            &saving,
        );
    })
    .await
    .unwrap();
    drop(state);
    let state = Arc::new(FilePairingState::open(&root, Path::new("state")).unwrap());
    let client = NativeEnrollmentClient::new(state, RuntimeDependencies::default().clock);
    fixture.time.set(created.record().expires_at_ms());
    let status = tokio::time::timeout(
        WAIT,
        client.status(TcpStream::connect(address).unwrap(), None),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(
        status,
        NativePairingStatus::Unclaimed {
            terminal: Some(TerminalCause::Expired),
            ..
        }
    ));
    store.refuse(Some(Fault::Reads));
    assert_eq!(
        hello().await.unwrap_err(),
        PairingRuntimeError::NoInvitation
    );
    store.refuse(None);
    client.shutdown().await;
    stop.send(()).unwrap();
    tokio::time::timeout(WAIT, listener)
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    connections.shutdown().await;
    fixture.gateway.shutdown().await;
}
