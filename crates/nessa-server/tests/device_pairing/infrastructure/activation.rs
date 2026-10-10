//! Approval through to an issued device credential, cleanup of ended stages,
//! and delivery to the device: slice 2b rows A1–A8, A10, S7, S8, D5 and D6 in
//! `docs/design/auth/device-pairing.md` ("Activation and credential delivery").
//! Real registry, Cedar, receiver authority, TLS and private storage; the
//! receiver port is substituted only to lose an answer, block, or fail.
use super::support::{pending, private_root, sockets, Fixture, Time, WAIT};
use nessa_auth::domain::pairing::ConsentClass;
use nessa_auth::{
    adapters::cedar::CedarPolicyEvaluator,
    adapters::pairing::{
        FilePairingState, GatewayTrust, ManualCode, NativeIdentity, NativeTransport, OsEntropy,
    },
    application::{
        authorization::AuthorizeAction,
        pairing::{
            ClientPendingStore, DeviceCredential, OwnerDecision, PairingStore, PairingStoreError,
            PendingEnrollment, PrivateKeyMaterial, PrivateStateError, ReceiverOutcome,
        },
        ports::{AccessError, CredentialEvidence, Decision},
        session::{AuthenticateSession, AuthenticatedSession},
    },
    domain::{
        pairing::{
            DeviceKey, InvitationId, PairingError, PairingInitiator, PairingPhase, PairingRecord,
            PublicIntent, TerminalCause,
        },
        Action, AudienceId, CredentialId, OrganizationId, PrincipalId, Resource, ResourceId,
    },
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_protocol::pairing::{
    wire::{
        decode_reply, encode_request, NativePairingReply, NativePairingRequest, NativePairingStatus,
    },
    EnrollmentChannel,
};
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    conversation::{
        domain::{PairedReceiver, ReceiverBinding},
        infrastructure::LocalReceiverAuthority,
    },
    device_pairing::{
        application::{
            ActivationError, Approval, CleanupError, PairingReceivers, ReceiverError,
            ReceiverRequest,
        },
        infrastructure::{
            ConversationReceivers, NativeConnectionFailure, NativeEnrollmentConnections,
            PairingRuntimeError,
        },
    },
};
use std::{
    net::{SocketAddr, TcpStream},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        mpsc::{sync_channel, Receiver, SyncSender},
        Arc, Mutex,
    },
};
use tokio::sync::oneshot;

/// The real receiver authority behind the port, with switches that lose a
/// pair's answer after it committed, answer for another credential, revoke the
/// receiver right after pairing, refuse lookups, or park a pair until released.
pub(crate) struct Receivers {
    authority: Arc<LocalReceiverAuthority>,
    real: ConversationReceivers,
    pub(crate) lose_pair_answer: AtomicBool,
    wrong_credential: AtomicBool,
    pub(crate) revoke_after_pair: AtomicBool,
    refuse_lookups: AtomicBool,
    refuse_fences: AtomicBool,
    not_holding: AtomicBool,
    park: Mutex<Option<(oneshot::Sender<()>, Receiver<()>)>>,
    pairs: AtomicUsize,
}
impl Receivers {
    fn new(authority: Arc<LocalReceiverAuthority>) -> Arc<Self> {
        Arc::new(Self {
            real: ConversationReceivers::new(authority.clone()),
            authority,
            lose_pair_answer: AtomicBool::new(false),
            wrong_credential: AtomicBool::new(false),
            revoke_after_pair: AtomicBool::new(false),
            refuse_lookups: AtomicBool::new(false),
            refuse_fences: AtomicBool::new(false),
            not_holding: AtomicBool::new(false),
            park: Mutex::new(None),
            pairs: AtomicUsize::new(0),
        })
    }
    /// The next pair parks after it commits: the receiver fires when it has
    /// parked, and it stays parked until the sender fires.
    fn park_next_pair(&self) -> (oneshot::Receiver<()>, SyncSender<()>) {
        let (entered, has_entered) = oneshot::channel();
        let (release, released) = sync_channel(1);
        *self.park.lock().unwrap() = Some((entered, released));
        (has_entered, release)
    }
}
impl PairingReceivers for Receivers {
    fn pair(&self, request: &ReceiverRequest) -> Result<ReceiverOutcome, ReceiverError> {
        self.pairs.fetch_add(1, Ordering::SeqCst);
        let outcome = self.real.pair(request)?;
        if let Some((entered, released)) = self.park.lock().unwrap().take() {
            entered.send(()).ok();
            released.recv().ok();
        }
        if self.revoke_after_pair.swap(false, Ordering::SeqCst) {
            let paired = PairedReceiver {
                receiver_id: outcome.receiver().as_str().to_owned(),
                credential_id: outcome.credential().clone(),
                organization_id: request.organization().clone(),
                owner_id: request.owner().clone(),
                paired_epoch: outcome.epoch(),
            };
            self.authority
                .fence(&paired, "revoked-meanwhile".into())
                .unwrap();
        }
        if self.lose_pair_answer.swap(false, Ordering::SeqCst) {
            return Err(ReceiverError::Unavailable);
        }
        if self.wrong_credential.load(Ordering::SeqCst) {
            return ReceiverOutcome::new(
                CredentialId::new("another-credential").unwrap(),
                outcome.request(),
                outcome.receiver().clone(),
                outcome.epoch(),
                outcome.generation(),
            )
            .map_err(|_| ReceiverError::Conflict);
        }
        Ok(outcome)
    }
    fn paired(&self, request: &ReceiverRequest) -> Result<Option<ReceiverOutcome>, ReceiverError> {
        if self.refuse_lookups.load(Ordering::SeqCst) {
            return Err(ReceiverError::Unavailable);
        }
        self.real.paired(request)
    }
    fn holding(
        &self,
        request: &ReceiverRequest,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<Option<u64>, ReceiverError> {
        if self.not_holding.load(Ordering::SeqCst) {
            return Ok(None);
        }
        self.real.holding(request, receiver, paired_epoch)
    }
    fn fence(
        &self,
        request: &ReceiverRequest,
        receiver: &ResourceId,
        paired_epoch: u64,
    ) -> Result<ReceiverOutcome, ReceiverError> {
        if self.refuse_lookups.load(Ordering::SeqCst) || self.refuse_fences.load(Ordering::SeqCst) {
            return Err(ReceiverError::Unavailable);
        }
        self.real.fence(request, receiver, paired_epoch)
    }
}

/// A fixture whose gateway reaches the real receiver authority through the
/// switchable `Receivers`.
pub(crate) async fn fixture() -> (Fixture, Arc<Receivers>) {
    let switches = Arc::new(Mutex::new(None));
    let captured = switches.clone();
    let fixture = Fixture::with_receivers(move |authority| {
        let receivers = Receivers::new(authority);
        *captured.lock().unwrap() = Some(receivers.clone());
        receivers as Arc<dyn PairingReceivers>
    })
    .await;
    let receivers = switches.lock().unwrap().take().unwrap();
    (fixture, receivers)
}

/// One device enrolled to Claimed through the real listener.
struct Claimed {
    client: NativeEnrollmentClient,
    store: Arc<FilePairingState>,
    address: SocketAddr,
    stop: oneshot::Sender<()>,
    listener: tokio::task::JoinHandle<std::io::Result<()>>,
    id: InvitationId,
    key: DeviceKey,
}
impl Claimed {
    async fn new(fixture: &Fixture, name: &str) -> Self {
        let created = fixture
            .gateway
            .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
            .await
            .unwrap();
        let (address, stop, listener, _) = fixture.listener().await;
        let (_, store) = pending(fixture.directory.path(), name);
        let client =
            NativeEnrollmentClient::new(store.clone(), RuntimeDependencies::default().clock);
        let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
        let claimed = tokio::time::timeout(
            WAIT,
            client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
        )
        .await
        .unwrap()
        .unwrap();
        assert!(matches!(claimed, NativePairingStatus::Claimed(_)));
        let id = created.record().id();
        let (_, key) = fixture
            .registry
            .read_pairing(id)
            .unwrap()
            .claim_binding()
            .unwrap();
        Self {
            client,
            store,
            address,
            stop,
            listener,
            id,
            key,
        }
    }
    async fn status(&self) -> Result<NativePairingStatus, NativeClientError> {
        tokio::time::timeout(
            WAIT,
            self.client
                .status(TcpStream::connect(self.address).unwrap(), None),
        )
        .await
        .unwrap()
    }
    async fn finish(self, fixture: &Fixture) {
        self.client.shutdown().await;
        self.stop.send(()).unwrap();
        tokio::time::timeout(WAIT, self.listener)
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        fixture.gateway.shutdown().await;
    }
}

async fn approval(fixture: &Fixture, claimed: &Claimed) -> Approval {
    fixture
        .gateway
        .approve(&fixture.session, claimed.id, claimed.key, OsEntropy)
        .await
        .unwrap()
}
async fn approve(fixture: &Fixture, claimed: &Claimed) -> PairingRecord {
    approval(fixture, claimed).await.record
}

/// Authenticate a fresh TLS connection made with the device's saved key as
/// the credential it was issued, through the gateway registry's device
/// verifier: the way the gateway will accept the device.
async fn authenticate(
    fixture: &Fixture,
    device: DeviceCredential,
) -> Result<AuthenticatedSession, AccessError> {
    let credential = device.credential().clone();
    let (key, pin, _) = device.into_enrollment().into_parts();
    let (server, client) = sockets();
    let connect = std::thread::spawn(move || {
        let identity = NativeIdentity::restore(key).unwrap();
        NativeTransport::connect(client, &identity, GatewayTrust::Pinned(pin)).map(drop)
    });
    let accepted = NativeTransport::accept(server, fixture.gateway.identity()).unwrap();
    connect.join().unwrap().unwrap();
    let verifier = fixture.registry.device_verifier(accepted.device_proof());
    AuthenticateSession {
        verifier: &verifier,
        access: fixture.registry.as_ref(),
        clock: &Time,
    }
    .execute(
        &CredentialEvidence::new(credential.as_str().as_bytes().to_vec()).unwrap(),
        &AudienceId::new("gateway").unwrap(),
    )
    .await
}

async fn allows(fixture: &Fixture, session: &AuthenticatedSession, action: &str) -> bool {
    AuthorizeAction {
        access: fixture.registry.as_ref(),
        clock: &Time,
        policy: &CedarPolicyEvaluator::new().unwrap(),
    }
    .execute(
        session,
        &Action::new(action).unwrap(),
        &Resource::new(
            OrganizationId::new("org").unwrap(),
            ResourceId::new("gateway").unwrap(),
        ),
    )
    .await
    .unwrap()
        == Decision::Allow
}

fn transitions(fixture: &Fixture, credential: &CredentialId) -> Option<ReceiverBinding> {
    fixture.receivers.binding(credential).unwrap()
}

/// Rows A1, A2, A6, A10: approval stages, pairs and publishes; the device's
/// status delivers the credential with its receiver and current epoch, again
/// on a repeat and to no other key; the client replaces its pending record
/// with it; and the gateway authenticates that key as that credential, scoped
/// to `conversation.read`.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_stages_pairs_and_publishes_a_device_credential() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    let active = approve(&fixture, &claimed).await;
    assert_eq!(active.phase(), PairingPhase::Active);
    let credential = active.credential().unwrap().clone();
    let (receiver, epoch) = active.receiver_binding().unwrap();
    assert_eq!(epoch, 1);
    assert_eq!(receivers.pairs.load(Ordering::SeqCst), 1);
    let binding = transitions(&fixture, &credential).unwrap();
    assert!(binding.active);
    assert_eq!(binding.receiver_id, receiver.as_str());
    assert_eq!(binding.owner_id, PrincipalId::new("owner").unwrap());
    // A repeated approval is the historical receipt: no second stage or pair.
    assert_eq!(approve(&fixture, &claimed).await, active);
    assert_eq!(receivers.pairs.load(Ordering::SeqCst), 1);

    let status = claimed.status().await.unwrap();
    let NativePairingStatus::Active {
        credential: delivered,
        receiver: delivered_receiver,
        access_epoch,
        ..
    } = &status
    else {
        panic!("the device reads Active: {status:?}");
    };
    assert_eq!(delivered, &credential);
    assert_eq!(delivered_receiver, receiver);
    assert_eq!(*access_epoch, 1);
    // Saved before the status returned, replacing the pending record.
    assert!(claimed.store.load_pending().unwrap().is_none());
    let saved = claimed.store.load_credential().unwrap().unwrap();
    assert_eq!(saved.credential(), &credential);
    assert_eq!(saved.receiver(), receiver);
    // The reply is delivered again, identically; nothing new is issued.
    assert_eq!(claimed.status().await.unwrap(), status);
    assert_eq!(fixture.registry.read_pairing(claimed.id).unwrap(), active);
    // The epoch delivered is the receiver's current one, not the pair
    // receipt's: a policy change advances it, and the record keeps epoch 1.
    drop(
        LocalReceiverAuthority::open(
            &private_root(fixture.directory.path(), "receiver-access")
                .join("receiver-access.sqlite3"),
            "another-policy",
            Arc::new(Time),
        )
        .unwrap(),
    );
    let advanced = claimed.status().await.unwrap();
    assert!(
        matches!(&advanced, NativePairingStatus::Active { access_epoch: 2, credential: same, .. } if same == &credential),
        "{advanced:?}"
    );
    assert_eq!(fixture.registry.read_pairing(claimed.id).unwrap(), active);
    // A device holding a credential does not enroll again.
    let code = ManualCode::parse(b"ABCD-EFGH").unwrap();
    assert_eq!(
        claimed
            .client
            .enroll(
                TcpStream::connect(claimed.address).unwrap(),
                code,
                OsEntropy
            )
            .await
            .unwrap_err(),
        NativeClientError::Enrolled
    );

    let session = authenticate(&fixture, saved).await.unwrap();
    assert_eq!(session.context().credential_id(), &credential);
    assert_eq!(
        session.context().principal_id(),
        &PrincipalId::new("owner").unwrap()
    );
    assert!(allows(&fixture, &session, "conversation.read").await);
    assert!(!allows(&fixture, &session, "credential.manage").await);
    assert!(!allows(&fixture, &session, "conversation.write").await);
    claimed.finish(&fixture).await;
}

/// Row A3: a receiver answer for another credential is refused by Auth before
/// it is remembered; the record stays Staging without a credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn receiver_result_must_match_the_stage() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.wrong_credential.store(true, Ordering::SeqCst);
    let staging = approve(&fixture, &claimed).await;
    assert_eq!(staging.phase(), PairingPhase::Staging);
    assert!(staging.receiver_binding().is_none());
    assert!(matches!(
        claimed.status().await.unwrap(),
        NativePairingStatus::Staging(_)
    ));
    assert!(claimed.store.load_pending().unwrap().is_some());
    // The correct answer, under the same stage, then activates.
    receivers.wrong_credential.store(false, Ordering::SeqCst);
    let active = approve(&fixture, &claimed).await;
    assert_eq!(active.phase(), PairingPhase::Active);
    assert_eq!(active.stage_binding(), staging.stage_binding());
    claimed.finish(&fixture).await;
}

/// Row A4: a pair that committed but whose answer was lost is found again by
/// the same stage on the next approval, also after a gateway restart: one
/// receiver, epoch 1, one credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_retry_rejoins_the_original_receiver() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.lose_pair_answer.store(true, Ordering::SeqCst);
    let staging = approve(&fixture, &claimed).await;
    assert_eq!(staging.phase(), PairingPhase::Staging);
    assert!(staging.receiver_binding().is_none());
    let (credential, _) = staging.stage_binding().unwrap();
    let original = transitions(&fixture, credential).unwrap();
    assert_eq!(original.access_epoch, 1);
    claimed.client.shutdown().await;
    claimed.stop.send(()).unwrap();
    claimed.listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
    // Restart: the same records, a fresh runtime over the real receivers.
    let fixture = fixture.reopen().await;
    let active = fixture
        .gateway
        .approve(&fixture.session, claimed.id, claimed.key, OsEntropy)
        .await
        .unwrap()
        .record;
    assert_eq!(active.phase(), PairingPhase::Active);
    assert_eq!(active.stage_binding(), staging.stage_binding());
    let (receiver, epoch) = active.receiver_binding().unwrap();
    assert_eq!(receiver.as_str(), original.receiver_id);
    assert_eq!(epoch, 1);
    assert_eq!(transitions(&fixture, credential), Some(original));
    fixture.gateway.shutdown().await;
}

/// Row A5: the paired receiver is revoked before the publication read, so
/// nothing is published; the record stays Staging.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_receiver_before_publication_stays_staging() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.revoke_after_pair.store(true, Ordering::SeqCst);
    let staging = approve(&fixture, &claimed).await;
    assert_eq!(staging.phase(), PairingPhase::Staging);
    assert!(staging.receiver_binding().is_some());
    let (credential, _) = staging.stage_binding().unwrap();
    assert!(!transitions(&fixture, credential).unwrap().active);
    // Approving again cannot publish over the revoked receiver either.
    assert_eq!(approve(&fixture, &claimed).await, staging);
    assert!(matches!(
        claimed.status().await.unwrap(),
        NativePairingStatus::Staging(_)
    ));
    claimed.finish(&fixture).await;
}

/// Row A7: cancelling a Staging enrollment settles its receiver at once, as
/// the system, and keeps the owner's first cause; with no receiver yet, the
/// lookup finds no receipt and cleanup completes without one.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_during_staging_settles_the_exact_receiver() {
    for remembered in [true, false] {
        let (fixture, receivers) = fixture().await;
        let claimed = Claimed::new(&fixture, "device").await;
        if remembered {
            // Stays Staging with its receiver remembered: the current read
            // finds it revoked.
            receivers.revoke_after_pair.store(true, Ordering::SeqCst);
        } else {
            // Stays Staging without a receiver: Auth refuses the answer.
            receivers.wrong_credential.store(true, Ordering::SeqCst);
        }
        let staging = approve(&fixture, &claimed).await;
        receivers.wrong_credential.store(false, Ordering::SeqCst);
        assert_eq!(staging.phase(), PairingPhase::Staging);
        assert_eq!(staging.receiver_binding().is_some(), remembered);
        let cancelled = fixture
            .gateway
            .decide(&fixture.session, claimed.id, OwnerDecision::Cancel)
            .await
            .unwrap();
        assert_eq!(cancelled.phase(), PairingPhase::Terminal);
        assert!(!cancelled.cleanup_pending());
        let (cause, initiator) = cancelled.terminal().unwrap();
        assert_eq!(cause, TerminalCause::Cancelled);
        assert_eq!(
            initiator,
            &PairingInitiator::Principal(PrincipalId::new("owner").unwrap())
        );
        let (credential, _) = cancelled.stage_binding().unwrap();
        // The receiver Auth refused was paired all the same; cleanup found its
        // receipt and fenced it rather than pairing again.
        let fenced = transitions(&fixture, credential).unwrap();
        assert!(!fenced.active);
        assert_eq!(receivers.pairs.load(Ordering::SeqCst), 1);
        claimed.finish(&fixture).await;
    }
}

/// Row A8: a cancel while the activation holds the stage leaves cleanup
/// pending; the activation, still holding it, remembers the late receiver and
/// fences it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn cancel_during_activation_is_settled_by_the_activation() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    let (parked, release) = receivers.park_next_pair();
    let gateway = fixture.gateway.clone();
    let session = fixture.session.clone();
    let (id, key) = (claimed.id, claimed.key);
    let approval = tokio::spawn(async move { gateway.approve(&session, id, key, OsEntropy).await });
    tokio::time::timeout(WAIT, parked).await.unwrap().unwrap();
    let cancelled = fixture
        .gateway
        .decide(&fixture.session, claimed.id, OwnerDecision::Cancel)
        .await
        .unwrap();
    assert_eq!(cancelled.phase(), PairingPhase::Terminal);
    assert!(
        cancelled.cleanup_pending(),
        "the activation holds the stage"
    );
    release.send(()).unwrap();
    let settled = tokio::time::timeout(WAIT, approval)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(settled.phase(), PairingPhase::Terminal);
    assert!(!settled.cleanup_pending());
    assert_eq!(settled.terminal().unwrap().0, TerminalCause::Cancelled);
    assert!(settled
        .credential()
        .is_some_and(|credential| !transitions(&fixture, credential).unwrap().active));
    claimed.finish(&fixture).await;
}

/// Rows S7, S8: an ended enrollment whose cleanup could not run is settled by
/// reconciliation, lookup only, as composition runs it before the native bind;
/// while the receiver authority is unavailable, reconciliation fails and the
/// record keeps its obligation and first cause.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn startup_settles_ended_enrollments_before_serving() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.lose_pair_answer.store(true, Ordering::SeqCst);
    approve(&fixture, &claimed).await;
    receivers.refuse_lookups.store(true, Ordering::SeqCst);
    let pending = fixture
        .gateway
        .decide(&fixture.session, claimed.id, OwnerDecision::Cancel)
        .await
        .unwrap();
    assert!(pending.cleanup_pending());
    // S8: still unavailable.
    assert_eq!(
        fixture.gateway.reconcile_cleanup().await,
        Err(PairingRuntimeError::Cleanup(CleanupError::Receiver(
            ReceiverError::Unavailable
        )))
    );
    assert_eq!(fixture.registry.read_pairing(claimed.id).unwrap(), pending);
    claimed.client.shutdown().await;
    claimed.stop.send(()).unwrap();
    claimed.listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
    // S7: after a restart the lookup finds the original pair and fences it.
    let fixture = fixture.reopen().await;
    fixture.gateway.reconcile_cleanup().await.unwrap();
    let settled = fixture.registry.read_pairing(claimed.id).unwrap();
    assert!(!settled.cleanup_pending());
    assert_eq!(settled.terminal().unwrap().0, TerminalCause::Cancelled);
    let (credential, _) = settled.stage_binding().unwrap();
    assert!(!transitions(&fixture, credential).unwrap().active);
    fixture.gateway.shutdown().await;
}

/// Rows D5, D6: shutdown waits for an admitted approval's receiver work
/// before it returns; reconciliation afterwards, as composition runs it after
/// the drains, reports and keeps an obligation it cannot settle, and settles
/// it once the receiver authority answers.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn shutdown_settles_ended_enrollments_after_the_drains() {
    let (fixture, receivers) = fixture().await;
    // One ended enrollment whose cleanup could not run.
    let first = Claimed::new(&fixture, "first").await;
    receivers.lose_pair_answer.store(true, Ordering::SeqCst);
    approve(&fixture, &first).await;
    receivers.refuse_lookups.store(true, Ordering::SeqCst);
    let pending = fixture
        .gateway
        .decide(&fixture.session, first.id, OwnerDecision::Cancel)
        .await
        .unwrap();
    assert!(pending.cleanup_pending());
    // A second device's approval is in its worker when shutdown begins.
    let second = Claimed::new(&fixture, "second").await;
    let (parked, release) = receivers.park_next_pair();
    let gateway = fixture.gateway.clone();
    let session = fixture.session.clone();
    let (id, key) = (second.id, second.key);
    let approval = tokio::spawn(async move { gateway.approve(&session, id, key, OsEntropy).await });
    tokio::time::timeout(WAIT, parked).await.unwrap().unwrap();
    let gateway = fixture.gateway.clone();
    let shutdown = tokio::spawn(async move { gateway.shutdown().await });
    tokio::time::sleep(std::time::Duration::from_millis(200)).await;
    assert!(
        !shutdown.is_finished(),
        "an approval is still in its worker"
    );
    assert_eq!(
        fixture
            .gateway
            .owner_status(&fixture.session, first.id)
            .await
            .unwrap_err(),
        PairingRuntimeError::ShuttingDown,
        "owner admission is closed while it drains"
    );
    release.send(()).unwrap();
    let active = tokio::time::timeout(WAIT, approval)
        .await
        .unwrap()
        .unwrap()
        .unwrap()
        .record;
    assert_eq!(active.phase(), PairingPhase::Active);
    tokio::time::timeout(WAIT, shutdown).await.unwrap().unwrap();
    // D6: the receiver authority still refuses; the obligation stays.
    assert_eq!(
        fixture.gateway.reconcile_cleanup().await,
        Err(PairingRuntimeError::Cleanup(CleanupError::Receiver(
            ReceiverError::Unavailable
        )))
    );
    assert_eq!(fixture.registry.read_pairing(first.id).unwrap(), pending);
    // D5: once it answers, reconciliation settles it and leaves Active alone.
    receivers.refuse_lookups.store(false, Ordering::SeqCst);
    fixture.gateway.reconcile_cleanup().await.unwrap();
    let settled = fixture.registry.read_pairing(first.id).unwrap();
    assert!(!settled.cleanup_pending());
    assert_eq!(settled.terminal(), pending.terminal());
    assert_eq!(fixture.registry.read_pairing(second.id).unwrap(), active);
    for claimed in [first, second] {
        claimed.client.shutdown().await;
        claimed.stop.send(()).unwrap();
        claimed.listener.await.unwrap().unwrap();
    }
}

/// The real client store, whose first credential save is refused.
struct RefusedCredentialSave {
    state: Arc<FilePairingState>,
    refuse: AtomicBool,
}
impl ClientPendingStore for RefusedCredentialSave {
    fn load_pending(&self) -> Result<Option<PendingEnrollment>, PrivateStateError> {
        self.state.load_pending()
    }
    fn load_credential(&self) -> Result<Option<DeviceCredential>, PrivateStateError> {
        self.state.load_credential()
    }
    fn end_enrollment(&self, expected: PublicIntent) -> Result<(), PrivateStateError> {
        self.state.end_enrollment(expected)
    }
    fn save_pending(
        &self,
        key: &PrivateKeyMaterial,
        pin: &[u8; 44],
        intent: PublicIntent,
        expected: Option<PublicIntent>,
    ) -> Result<(), PrivateStateError> {
        self.state.save_pending(key, pin, intent, expected)
    }
    fn save_credential(
        &self,
        credential: &CredentialId,
        receiver: &ResourceId,
        expected: PublicIntent,
    ) -> Result<(), PrivateStateError> {
        if self.refuse.swap(false, Ordering::SeqCst) {
            return Err(PrivateStateError::Unavailable);
        }
        self.state.save_credential(credential, receiver, expected)
    }
}

/// Row A10: when saving the delivered credential fails, the client reports the
/// storage failure, keeps its pending record, and the next status delivers the
/// same credential again, which is then saved in its place.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_keeps_the_issued_credential_and_clears_pending() {
    let fixture = Fixture::new().await;
    let created = fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let (address, stop, listener, _) = fixture.listener().await;
    let (_, state) = pending(fixture.directory.path(), "device");
    let store = Arc::new(RefusedCredentialSave {
        state: state.clone(),
        refuse: AtomicBool::new(true),
    });
    let client = NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(address).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    let id = created.record().id();
    let (_, key) = fixture
        .registry
        .read_pairing(id)
        .unwrap()
        .claim_binding()
        .unwrap();
    let active = fixture
        .gateway
        .approve(&fixture.session, id, key, OsEntropy)
        .await
        .unwrap()
        .record;
    assert_eq!(active.phase(), PairingPhase::Active);
    let status = || async {
        tokio::time::timeout(
            WAIT,
            client.status(TcpStream::connect(address).unwrap(), None),
        )
        .await
        .unwrap()
    };
    assert_eq!(
        status().await.unwrap_err(),
        NativeClientError::Storage(PrivateStateError::Unavailable)
    );
    assert!(state.load_pending().unwrap().is_some());
    assert!(state.load_credential().unwrap().is_none());
    let delivered = status().await.unwrap();
    assert!(matches!(
        &delivered,
        NativePairingStatus::Active { credential, .. } if Some(credential) == active.credential()
    ));
    assert!(state.load_pending().unwrap().is_none());
    assert_eq!(
        state.load_credential().unwrap().unwrap().credential(),
        active.credential().unwrap()
    );
    // Asking again reads from the saved credential and changes nothing.
    assert_eq!(status().await.unwrap(), delivered);
    client.shutdown().await;
    stop.send(()).unwrap();
    listener.await.unwrap().unwrap();
    fixture.gateway.shutdown().await;
}

/// Row A6: another device key asking for an Active enrollment's status is
/// refused before disclosure, and its reply carries no credential.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn another_key_cannot_read_an_active_enrollment() {
    let (fixture, _) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    let active = approve(&fixture, &claimed).await;
    assert_eq!(active.phase(), PairingPhase::Active);
    let credential = active.credential().unwrap().as_str().to_owned();
    claimed.status().await.unwrap();
    let public = claimed.store.load_credential().unwrap().unwrap().intent();
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
    assert!(!String::from_utf8_lossy(&reply).contains(&credential));
    assert_eq!(fixture.registry.read_pairing(claimed.id).unwrap(), active);
    owner.shutdown().await;
    claimed.finish(&fixture).await;
}

/// Row A11: a stop approving again can finish is `retryable` (and does
/// finish); one it cannot is permanent.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn approval_reports_whether_a_stop_is_retryable() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.lose_pair_answer.store(true, Ordering::SeqCst);
    let stopped = approval(&fixture, &claimed).await;
    assert_eq!(stopped.record.phase(), PairingPhase::Staging);
    assert_eq!(
        stopped.stopped,
        Some(ActivationError::Receiver(ReceiverError::Unavailable))
    );
    assert!(stopped.stopped.unwrap().retryable());
    let finished = approval(&fixture, &claimed).await;
    assert_eq!(finished.record.phase(), PairingPhase::Active);
    assert_eq!(finished.stopped, None);
    claimed.finish(&fixture).await;

    let (fixture, receivers) = crate::activation::fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.revoke_after_pair.store(true, Ordering::SeqCst);
    let stopped = approval(&fixture, &claimed).await;
    assert_eq!(stopped.record.phase(), PairingPhase::Staging);
    assert_eq!(stopped.stopped, Some(ActivationError::ReceiverNotCurrent));
    assert!(!stopped.stopped.unwrap().retryable());
    claimed.finish(&fixture).await;
}

/// Row A12: an Active enrollment whose receiver no longer holds the pairing
/// gets no status, so no epoch; the device keeps what it saved.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn active_status_refuses_a_receiver_that_no_longer_holds_the_pairing() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    approve(&fixture, &claimed).await;
    let delivered = claimed.status().await.unwrap();
    receivers.not_holding.store(true, Ordering::SeqCst);
    assert_eq!(
        claimed.status().await.unwrap_err(),
        NativeClientError::Refused
    );
    assert!(claimed.store.load_credential().unwrap().is_some());
    receivers.not_holding.store(false, Ordering::SeqCst);
    assert_eq!(claimed.status().await.unwrap(), delivered);
    claimed.finish(&fixture).await;
}

/// Row A13: a Staging enrollment that reaches its expiry is settled by the
/// owner read or device status that expires it, not left for a restart.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_staging_is_settled_by_the_path_that_expires_it() {
    for owner_path in [true, false] {
        let (fixture, receivers) = fixture().await;
        let claimed = Claimed::new(&fixture, "device").await;
        // Staging without a remembered receiver; the pair itself committed.
        receivers.wrong_credential.store(true, Ordering::SeqCst);
        let staging = approve(&fixture, &claimed).await;
        receivers.wrong_credential.store(false, Ordering::SeqCst);
        assert_eq!(staging.phase(), PairingPhase::Staging);
        fixture.time.set(staging.expires_at_ms());
        if owner_path {
            let read = fixture
                .gateway
                .owner_status(&fixture.session, claimed.id)
                .await
                .unwrap();
            assert!(!read.cleanup_pending());
        } else {
            let status = claimed.status().await.unwrap();
            assert!(
                matches!(
                    status,
                    NativePairingStatus::Terminal {
                        cause: TerminalCause::Expired,
                        ..
                    }
                ),
                "{status:?}"
            );
        }
        let settled = fixture.registry.read_pairing(claimed.id).unwrap();
        assert_eq!(settled.terminal().unwrap().0, TerminalCause::Expired);
        assert_eq!(settled.terminal().unwrap().1, &PairingInitiator::System);
        assert!(!settled.cleanup_pending());
        let (credential, _) = settled.stage_binding().unwrap();
        assert!(!transitions(&fixture, credential).unwrap().active);
        assert_eq!(receivers.pairs.load(Ordering::SeqCst), 1);
        claimed.finish(&fixture).await;
    }
}

/// Row A5 (P46): a revocation of the credential on the paired receiver before
/// publication refuses it, even after the credential was regranted back.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn intervening_fence_refuses_publication_even_when_regranted_back() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.lose_pair_answer.store(true, Ordering::SeqCst);
    let staging = approve(&fixture, &claimed).await;
    let (credential, _) = staging.stage_binding().unwrap();
    let paired = transitions(&fixture, credential).unwrap();
    let owner = PrincipalId::new("owner").unwrap();
    for (replacement, active, request) in [
        (None, false, "revoke-1"),
        (
            Some(CredentialId::new("other").unwrap()),
            true,
            "regrant-other",
        ),
        (None, false, "revoke-2"),
        (Some(credential.clone()), true, "regrant-back"),
    ] {
        fixture
            .receivers
            .change(
                paired.receiver_id.clone(),
                replacement,
                active,
                owner.clone(),
                request.into(),
            )
            .await
            .unwrap();
    }
    assert!(transitions(&fixture, credential).unwrap().active);
    let stopped = approval(&fixture, &claimed).await;
    assert_eq!(stopped.record.phase(), PairingPhase::Staging);
    assert_eq!(stopped.stopped, Some(ActivationError::ReceiverNotCurrent));
    assert!(stopped.record.receiver_binding().is_some());
    claimed.finish(&fixture).await;
}

/// Row A14: the device's authenticated Terminal status removes its credential
/// and lets it enroll again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_status_ends_the_device_record_and_allows_a_new_enrollment() {
    let (fixture, _) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    approve(&fixture, &claimed).await;
    claimed.status().await.unwrap();
    assert!(claimed.store.load_credential().unwrap().is_some());
    fixture
        .gateway
        .decide(&fixture.session, claimed.id, OwnerDecision::Cancel)
        .await
        .unwrap();
    let ended = claimed.status().await.unwrap();
    assert!(matches!(
        ended,
        NativePairingStatus::Terminal {
            cause: TerminalCause::Cancelled,
            ..
        }
    ));
    assert!(claimed.store.load_credential().unwrap().is_none());
    assert!(claimed.store.load_pending().unwrap().is_none());
    // Nothing is left to ask about, and a new code enrolls the same device.
    assert_eq!(
        claimed.status().await.unwrap_err(),
        NativeClientError::NoPending
    );
    let created = fixture
        .gateway
        .create(fixture.session.clone(), ConsentClass::DeviceRead, OsEntropy)
        .await
        .unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    let again = tokio::time::timeout(
        WAIT,
        claimed.client.enroll(
            TcpStream::connect(claimed.address).unwrap(),
            code,
            OsEntropy,
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert!(matches!(again, NativePairingStatus::Claimed(_)));
    claimed.finish(&fixture).await;
}

/// Rows A9, A13: an Active credential revoked by its owner is fenced by the
/// next owner read, lookup only, with the revocation's cause kept.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn revoked_active_receiver_is_fenced_by_the_next_read() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    let active = approve(&fixture, &claimed).await;
    let credential = active.credential().unwrap().clone();
    fixture
        .registry
        .revoke_sync(
            nessa_auth::application::credential_admin::RevokeCredentialRequest {
                request_id: "revoke-device".into(),
                issuer_principal_id: "owner".into(),
                credential_id: credential.as_str().into(),
                revoked_at: 111,
            },
        )
        .unwrap();
    let revoked = fixture.registry.read_pairing(claimed.id).unwrap();
    assert!(revoked.cleanup_pending());
    assert!(transitions(&fixture, &credential).unwrap().active);
    let read = fixture
        .gateway
        .owner_status(&fixture.session, claimed.id)
        .await
        .unwrap();
    assert!(!read.cleanup_pending());
    assert_eq!(read.terminal(), revoked.terminal());
    assert_eq!(read.terminal().unwrap().0, TerminalCause::CredentialRevoked);
    assert!(!transitions(&fixture, &credential).unwrap().active);
    assert_eq!(receivers.pairs.load(Ordering::SeqCst), 1);
    claimed.finish(&fixture).await;
}

/// Rows A13, A15: an expiry whose cleanup remembers the receiver and then
/// cannot fence it still answers, with the record as cleanup left it; the
/// next read, once the receiver authority answers, finishes it.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_read_answers_with_the_record_as_cleanup_left_it() {
    let (fixture, receivers) = fixture().await;
    let claimed = Claimed::new(&fixture, "device").await;
    receivers.wrong_credential.store(true, Ordering::SeqCst);
    let staging = approve(&fixture, &claimed).await;
    receivers.wrong_credential.store(false, Ordering::SeqCst);
    assert!(staging.receiver_binding().is_none());
    receivers.refuse_fences.store(true, Ordering::SeqCst);
    fixture.time.set(staging.expires_at_ms());
    let read = fixture
        .gateway
        .owner_status(&fixture.session, claimed.id)
        .await
        .unwrap();
    assert_eq!(read.terminal().unwrap().0, TerminalCause::Expired);
    assert!(read.cleanup_pending());
    assert!(
        read.receiver_binding().is_some(),
        "the remembered receiver shows"
    );
    assert_eq!(read, fixture.registry.read_pairing(claimed.id).unwrap());
    receivers.refuse_fences.store(false, Ordering::SeqCst);
    let settled = fixture
        .gateway
        .owner_status(&fixture.session, claimed.id)
        .await
        .unwrap();
    assert!(!settled.cleanup_pending());
    claimed.finish(&fixture).await;
}
