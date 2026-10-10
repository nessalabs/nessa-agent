//! Native device pairing, mounted when `config.json` names a native listen
//! address: private state and the gateway key before anything is bound, one
//! connection owner per gateway, and a stop that joins the gateway's drain.
//!
//! ```text
//! prepare: native-pairing/ --> FilePairingState --> restore_gateway_identity
//!                          --> GatewayPairing::open --> reconcile_cleanup
//!                          --> (PreparedNative, PairingOwnerCommands,
//!                               PeerCommands over peer-gateways/ and the key,
//!                               audited in peer-gateways-audit/)
//! bind:    PreparedNative --> TcpEnrollmentAccept --> the one NativeEnrollmentConnections
//!          (with NativeSessions: the product state + the registry's device verifier)
//! start:   BoundNative --> listener task (failed --> watch)
//!          --> PeerPoller (reads each peer) --> RunningNative
//! stop:    signal_stop (listener and poller) --> join: the poller's workers,
//!          then listener drain, then GatewayPairing::shutdown, then reconcile_cleanup
//! ```
//! Arrows are construction and ownership handoffs, in order. Design rows
//! S1–S14 in `docs/design/auth/device-pairing.md` ("Owner routes and mounting")
//! and S7, S8, D5, D6 ("Activation and credential delivery"), and PR1, PR13
//! ("Protected reads over the native channel").
use super::runtime_config::NativeConfig;
use crate::conversation::infrastructure::LocalReceiverAuthority;
use crate::core::{NativeFailure, NativeShutdownFailure, RunError};
use crate::device_pairing::infrastructure::{
    restore_gateway_identity, ConversationReceivers, GatewayPairing, InvitationEntropy,
    NativeEnrollmentConnections, NativeEnrollmentListener, PairingOwnerCommands,
    PairingRuntimeDependencies, TcpEnrollmentAccept,
};
use crate::peer_gateways::infrastructure::{
    DurablePeerAudit, PeerCommands, PeerPoller, PeerRecords, PollPolicy, TcpPeerConnector,
};
use crate::product::{DeviceCredentials, NativeSessions, ProductRouteState};
use nessa_auth::{
    adapters::{
        local::LocalCredentialStore,
        pairing::{FilePairingState, OsEntropy},
    },
    application::{
        pairing::{DeviceConnectionProof, PairingWorkerFault},
        ports::{
            AccessError, Clock, CredentialEvidence, CredentialVerifier, PolicyEvaluator,
            PortFuture, VerifiedCredential,
        },
    },
    domain::{AudienceId, Resource},
};
use nessa_protocol::clock::Clock as MonotonicClock;
use std::{
    io::{ErrorKind, Result as IoResult},
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::TcpListener,
    sync::{oneshot, watch},
    task::JoinHandle,
};

/// Where native pairing keeps its key, beneath the namespace directory.
const PRIVATE_DIRECTORY: &str = "native-pairing";
/// Where this gateway keeps what it learned from each gateway it dialed.
const PEER_DIRECTORY: &str = "peer-gateways";
/// Where the owner's peer enrolls and forgets are audited.
const PEER_AUDIT_DIRECTORY: &str = "peer-gateways-audit";

/// What startup needs from the rest of composition to prepare native pairing.
pub(super) struct NativeInputs {
    /// The namespace directory: the parent of `auth/`.
    pub namespace: PathBuf,
    pub registry: Arc<LocalCredentialStore>,
    pub policy: Arc<dyn PolicyEvaluator>,
    /// The receiver authority conversations read through, shared.
    pub receivers: Arc<LocalReceiverAuthority>,
    pub clock: Arc<dyn Clock>,
    /// The root's monotonic clock, for deadlines: peer enrollment's here,
    /// as inbound pairing's is given to `bind`.
    pub deadline_clock: Arc<dyn MonotonicClock>,
    /// The gateway resource every invitation is for, from the registry.
    pub gateway: Resource,
    /// The same gateway as the key store names it.
    pub audience: AudienceId,
}

/// A gateway whose key is restored and whose enrollments are settled, not yet
/// bound. Not `Clone`: `bind` consumes it, so one opened gateway gets one
/// connection owner (design row S13).
pub(super) struct PreparedNative {
    gateway: Arc<GatewayPairing>,
    address: SocketAddr,
    registry: Arc<LocalCredentialStore>,
    /// The peer commands the poller reads through once serving starts.
    peers: Arc<PeerCommands>,
    /// Wall time for when a peer read finished.
    clock: Arc<dyn Clock>,
}

/// Prepare native pairing before either socket is bound (design rows S3–S8):
/// the private directory and state, then the gateway key (restored, or first
/// published through Auth's guarded publication), then `GatewayPairing::open`,
/// which settles this gateway's unfinished enrollments, then the receivers of
/// ended enrollments, lookup only. A cleanup that cannot complete refuses
/// startup (row S8). The owner commands are for the product socket; they give
/// no way back to the runtime. The peer commands enroll this gateway into
/// others with the same key, which their records refer to and never copy.
pub(super) async fn prepare(
    config: &NativeConfig,
    inputs: NativeInputs,
) -> Result<(PreparedNative, PairingOwnerCommands, Arc<PeerCommands>), RunError> {
    // The private-state owner wants a trusted absolute root; on Unix that is
    // the canonical spelling, as its own tests use.
    #[cfg(unix)]
    let root = inputs
        .namespace
        .canonicalize()
        .map_err(|error| RunError::Native(NativeFailure::Directory(error)))?;
    #[cfg(not(unix))]
    let root = inputs.namespace.clone();
    for directory in [PRIVATE_DIRECTORY, PEER_DIRECTORY, PEER_AUDIT_DIRECTORY] {
        nessa_local_storage::create_directory_beneath(&root, Path::new(directory))
            .map_err(|error| RunError::Native(NativeFailure::Directory(error)))?;
    }
    let keys = Arc::new(
        FilePairingState::open(&root, Path::new(PRIVATE_DIRECTORY))
            .map_err(|error| RunError::Native(NativeFailure::PrivateState(error)))?,
    );
    let identity = restore_gateway_identity(
        inputs.registry.clone(),
        keys.clone(),
        inputs.clock.clone(),
        OsEntropy,
    )
    .await
    .map_err(|error| RunError::Native(NativeFailure::Identity(error)))?;
    let registry = inputs.registry;
    let peers = Arc::new(PeerCommands::new(
        Arc::new(
            PeerRecords::open(
                &root,
                Path::new(PEER_DIRECTORY),
                keys.clone(),
                inputs.audience,
                inputs.clock.clone(),
            )
            .map_err(|error| RunError::Native(NativeFailure::PrivateState(error)))?,
        ),
        inputs.deadline_clock,
        Arc::new(DurablePeerAudit::new(
            root.join(PEER_AUDIT_DIRECTORY),
            inputs.clock.clone(),
        )),
        Arc::new(TcpPeerConnector),
    ));
    let wall = inputs.clock.clone();
    let gateway = Arc::new(
        GatewayPairing::open(PairingRuntimeDependencies {
            enrollments: registry.clone(),
            access: registry.clone(),
            policy: inputs.policy,
            receivers: Arc::new(ConversationReceivers::new(inputs.receivers)),
            clock: inputs.clock,
            gateway: inputs.gateway,
            key_store: keys,
            identity,
        })
        .map_err(|error| RunError::Native(NativeFailure::Open(error)))?,
    );
    gateway
        .reconcile_cleanup()
        .await
        .map_err(|error| RunError::Native(NativeFailure::Open(error)))?;
    let commands = PairingOwnerCommands::new(
        gateway.clone(),
        Arc::new(|| Box::new(OsEntropy) as Box<dyn InvitationEntropy>),
    );
    Ok((
        PreparedNative {
            gateway,
            address: config.listen_address,
            registry,
            peers: peers.clone(),
            clock: wall,
        },
        commands,
        peers,
    ))
}

/// Native pairing with its socket bound and its one connection owner built.
pub(super) struct BoundNative {
    gateway: Arc<GatewayPairing>,
    listener: NativeEnrollmentListener,
    address: SocketAddr,
    peers: Arc<PeerCommands>,
    clock: Arc<dyn Clock>,
}

impl BoundNative {
    /// The address the listener is bound to.
    pub(super) fn local_address(&self) -> SocketAddr {
        self.address
    }
}

/// Bind the configured address and build this gateway's only connection owner
/// (design rows S10, S13), on the root's monotonic clock for its deadlines. A
/// bind failure leaves the key and enrollment history as `prepare` found them.
/// A connection that opens a product session is served with `product`, the
/// same state the browser socket has, checking credentials against its TLS key
/// with the registry's device verifier (design rows PR1, PR3).
pub(super) async fn bind(
    prepared: PreparedNative,
    clock: Arc<dyn MonotonicClock>,
    product: ProductRouteState,
) -> Result<BoundNative, RunError> {
    let PreparedNative {
        gateway,
        address,
        registry,
        peers,
        clock: wall,
    } = prepared;
    let bind_failed = |source| RunError::Native(NativeFailure::Bind { address, source });
    let socket = TcpEnrollmentAccept::new(TcpListener::bind(address).await.map_err(bind_failed)?);
    let bound = socket.local_address().map_err(bind_failed)?;
    let sessions = NativeSessions::new(product, Arc::new(RegistryDevices(registry)));
    let connections = Arc::new(
        NativeEnrollmentConnections::new(gateway.clone(), clock)
            .with_protected_sessions(Arc::new(sessions)),
    );
    Ok(BoundNative {
        gateway,
        listener: NativeEnrollmentListener::new(socket, connections),
        address: bound,
        peers,
        clock: wall,
    })
}

/// The registry's device verifier, for credential evidence presented on a
/// native connection.
pub(super) struct RegistryDevices(pub(super) Arc<LocalCredentialStore>);
impl DeviceCredentials for RegistryDevices {
    fn verify<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
        evidence: &'a CredentialEvidence,
        audience: &'a AudienceId,
    ) -> PortFuture<'a, VerifiedCredential> {
        Box::pin(async move {
            self.0
                .device_verifier(proof)
                .verify(evidence, audience)
                .await
        })
    }
    fn holds_credential(
        &self,
        proof: &DeviceConnectionProof,
        audience: &AudienceId,
    ) -> Result<bool, AccessError> {
        self.0.device_verifier(proof).holds_credential(audience)
    }
}

/// The serving listener, and how to stop it.
pub(super) struct RunningNative {
    gateway: Arc<GatewayPairing>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<IoResult<()>>,
    /// Reads what each peer granted; stopped and joined with the listener.
    poller: PeerPoller,
}

/// Start serving. An accept failure that ends the listener is published on
/// `failure` before the listener drains (design rows P67, S14). The sender lives
/// inside the listener task, so a panic closes the channel (row S15); the root
/// stops the whole gateway on either.
pub(super) fn start(
    bound: BoundNative,
    failure: watch::Sender<Option<ErrorKind>>,
) -> RunningNative {
    let BoundNative {
        gateway,
        listener,
        peers,
        clock,
        ..
    } = bound;
    let poller = PeerPoller::start(peers, PollPolicy::default(), clock);
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(listener.run(
        OsEntropy::default,
        async move {
            // A dropped sender is a stop too: nothing can ask for one later.
            let _ = stopped.await;
        },
        move |kind| {
            failure.send_replace(Some(kind));
        },
    ));
    RunningNative {
        gateway,
        stop: Some(stop),
        task,
        poller,
    }
}

impl RunningNative {
    /// Ask the listener to stop admitting; it wakes its peers at once. Called
    /// before the cleanup owner waits on anything else (design row S12).
    pub(super) fn signal_stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        self.poller.signal_stop();
    }

    /// Wait for the listener to collect its peers and drain its connection
    /// owner, then close create and owner-command admission and wait for
    /// registration and every admitted command. Only then, with nothing left
    /// that could hold a stage, settle ended enrollments' receivers (design
    /// row D5). An accept failure was already published when it happened;
    /// here a fault of the listener task is a failure, and with its drain
    /// unknown no cleanup is attempted (row D6). A cleanup that does not
    /// complete is reported and stays pending in the registry.
    pub(super) async fn join(mut self) -> Result<(), NativeShutdownFailure> {
        self.signal_stop();
        // The poller's read and status workers end before anything else is
        // waited on: they hold sockets to peers and the peer turn.
        self.poller.join().await;
        let drained = match self.task.await {
            Ok(_) => Ok(()),
            Err(error) => Err(NativeShutdownFailure::ListenerFault(if error.is_panic() {
                PairingWorkerFault::Panic
            } else {
                PairingWorkerFault::Cancelled
            })),
        };
        self.gateway.shutdown().await;
        drained?;
        self.gateway
            .reconcile_cleanup()
            .await
            .map_err(NativeShutdownFailure::Cleanup)
    }
}

#[cfg(test)]
#[path = "../../tests/composition/native_pairing.rs"]
mod tests;
