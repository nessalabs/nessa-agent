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
//!          --> the tunnel's loopback ingress, when one is on (`composition::tunnel`)
//! start:   BoundNative --> listener task (failed --> watch)
//!          --> PeerPoller (reads each peer)
//!          --> the tunnel's ingress and its provider's program
//!          --> LinkingAnnouncer (only on a network address) --> RunningNative
//! stop:    signal_stop (announcement withdrawn, listener, poller, tunnel) --> join: the poller's workers,
//!          then the peer commands closed (running ones waited for, bounded by PREEMPT),
//!          then the peer audit closed and drained (bounded by AUDIT_DEADLINE),
//!          then the tunnel's ingress and supervisor, then listener drain, then GatewayPairing::shutdown, then reconcile_cleanup
//! ```
//! Arrows are construction and ownership handoffs, in order. Design rows
//! S1–S14 in `docs/design/auth/device-pairing.md` ("Owner routes and mounting")
//! and S7, S8, D5, D6 ("Activation and credential delivery"), and PR1, PR13
//! ("Protected reads over the native channel").
use super::runtime_config::{routable_advertised_host, NativeConfig, RelayAddress};
use crate::conversation::application::FenceCapture;
use crate::conversation::infrastructure::LocalReceiverAuthority;
use crate::core::{NativeFailure, NativeShutdownFailure, RunError};
use crate::device_pairing::application::{Announced, LinkingAnnouncement, LinkingAnnouncer};
use crate::device_pairing::infrastructure::{
    restore_gateway_identity, ConversationReceivers, EnvironmentPeers, GatewayPairing,
    InvitationEntropy, NativeEnrollmentConnections, NativeEnrollmentListener, PairingOwnerCommands,
    PairingRuntimeDependencies, RelayPermit, TcpEnrollmentAccept, RELAY_CAPACITY, TUNNEL_CAPACITY,
};
use crate::peer_gateways::infrastructure::{
    DurablePeerAudit, EnrollmentEntropy, PeerCommands, PeerPoller, PeerRecords, PollInputs,
    PollPolicy, TcpPeerConnector, AUDIT_DEADLINE, PREEMPT,
};
use crate::product::{DeviceCredentials, NativeSessions, ProductRouteState};
use crate::relay::gateway::{
    application::{Backoff, Relayed, ThroughRelay},
    infrastructure::{IdentityKey, RelayedAccept, TcpRelayDial},
};
use crate::timer::TokioTimer;
#[cfg(unix)]
use crate::tunnel::infrastructure::ingress::TunnelIngress;
use crate::tunnel::infrastructure::ingress::{Tunneled, TunneledAccept};
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
    domain::{pairing::OwnerPhoneRequest, AudienceId, Resource},
};
use nessa_protocol::clock::Clock as MonotonicClock;
use std::{
    io::{ErrorKind, Result as IoResult},
    net::{IpAddr, SocketAddr},
    path::{Path, PathBuf},
    sync::Arc,
};
use tokio::{
    net::{TcpListener, TcpStream},
    sync::{mpsc, oneshot, watch},
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
    /// The gateway's public id, as it is announced.
    audience: String,
    address: SocketAddr,
    registry: Arc<LocalCredentialStore>,
    /// The peer commands the poller reads through once serving starts.
    peers: Arc<PeerCommands>,
    /// Their audit, closed and drained when native pairing stops.
    audit: Arc<DurablePeerAudit>,
    /// Wall time for when a peer read finished.
    clock: Arc<dyn Clock>,
    /// The paired environments `openEnvironment` reaches; `None` refuses it.
    environments: Option<Arc<dyn EnvironmentPeers>>,
    /// The relay to serve through as well, if any.
    relay: Option<RelayAddress>,
    /// Host a wildcard bind is published as. The port is the listener's.
    advertised_address: Option<IpAddr>,
    /// A listener already bound to `address`. Production leaves this empty
    /// and `bind` binds `address`. A test that reserved the port hands the
    /// listener here so the port stays owned until accept.
    bound_socket: Option<TcpListener>,
    /// The web tunnel, when one is on.
    #[cfg(unix)]
    tunnel: Option<super::tunnel::PreparedTunnel>,
}

impl PreparedNative {
    /// Serve the web tunnel `tunnel` as well.
    #[cfg(unix)]
    pub(super) fn with_tunnel(mut self, tunnel: super::tunnel::PreparedTunnel) -> Self {
        self.tunnel = Some(tunnel);
        self
    }

    /// Record a pairing cancellation in the deletion inventory. The inventory
    /// is opened before `prepare`, and a cancellation commits inside Auth,
    /// which does not call the credential admin.
    pub(super) fn bind_fence_capture(&self, capture: Arc<dyn FenceCapture>) {
        self.gateway.bind_fence_capture(capture);
    }

    /// Serve `openEnvironment` with `environments`.
    pub(super) fn with_environments(mut self, environments: Arc<dyn EnvironmentPeers>) -> Self {
        self.environments = Some(environments);
        self
    }

    /// Keep `socket` until `bind`. It must already be bound to `address`.
    #[cfg(test)]
    fn keep_bound_socket(mut self, socket: TcpListener) -> Self {
        self.bound_socket = Some(socket);
        self
    }
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
    let audience = inputs.audience.as_str().to_owned();
    let audit = Arc::new(DurablePeerAudit::new(
        root.join(PEER_AUDIT_DIRECTORY),
        inputs.clock.clone(),
        inputs.deadline_clock.clone(),
    ));
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
        audit.clone(),
        Arc::new(TcpPeerConnector),
        Arc::new(|| Box::new(OsEntropy) as Box<dyn EnrollmentEntropy>),
    ));
    let wall = inputs.clock.clone();
    let gateway = Arc::new(
        GatewayPairing::open(PairingRuntimeDependencies {
            enrollments: registry.clone(),
            originals: registry.clone(),
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
            audience,
            address: config.listen_address,
            registry,
            peers: peers.clone(),
            audit,
            clock: wall,
            environments: None,
            relay: config.relay.clone(),
            advertised_address: config.advertised_address,
            bound_socket: None,
            #[cfg(unix)]
            tunnel: None,
        },
        commands,
        peers,
    ))
}

/// The listening socket, the readers a relay joined and the connections that
/// came through the tunnel, as the listener's one source of connections.
type NativeAccept = TunneledAccept<RelayedAccept<TcpEnrollmentAccept>>;

/// Native pairing with its socket bound and its one connection owner built.
pub(super) struct BoundNative {
    gateway: Arc<GatewayPairing>,
    audience: String,
    listener: NativeEnrollmentListener<NativeAccept>,
    address: SocketAddr,
    /// The address as configured: an ephemeral port reads 0 here, while
    /// `address` holds the port the operating system chose.
    configured: SocketAddr,
    /// Announces the listener on the local network once it serves; `None`
    /// announces nothing.
    announcer: Option<Arc<dyn LinkingAnnouncer>>,
    /// Serving through the relay, when one is configured: started with the
    /// listener.
    relay: Option<ThroughRelay<TcpStream, RelayPermit>>,
    peers: Arc<PeerCommands>,
    audit: Arc<DurablePeerAudit>,
    clock: Arc<dyn Clock>,
    /// The tunnel's bound ingress and what it serves, started with the
    /// listener.
    #[cfg(unix)]
    tunnel: Option<(TunnelIngress, super::tunnel::PreparedTunnel)>,
}

impl BoundNative {
    /// The address the listener is bound to.
    pub(super) fn local_address(&self) -> SocketAddr {
        self.address
    }
    /// Announce the listener with `announcer` once it serves.
    pub(super) fn with_announcer(mut self, announcer: Arc<dyn LinkingAnnouncer>) -> Self {
        self.announcer = Some(announcer);
        self
    }
    /// What the listener is announced as, when it is announced at all: only
    /// a listener a phone can reach, on a network address and a configured
    /// port. A loopback listener serves this machine alone, and an ephemeral
    /// port is not one the next start repeats.
    fn announcement(&self) -> Option<LinkingAnnouncement> {
        (self.configured.port() != 0 && !self.configured.ip().is_loopback()).then(|| {
            LinkingAnnouncement {
                address: SocketAddr::new(self.configured.ip(), self.address.port()),
                gateway: self.audience.clone(),
            }
        })
    }
    /// Start reading each peer: its waits and read budget on the peer
    /// commands' monotonic clock (the root's deadline clock), its finished
    /// reads stamped by the root's wall clock, and the operating system's
    /// entropy for its jitter.
    pub(super) fn start_poller(&self) -> PeerPoller {
        PeerPoller::start(
            self.peers.clone(),
            PollInputs {
                policy: PollPolicy::default(),
                wall: self.clock.clone(),
                entropy: Arc::new(|| Box::new(OsEntropy) as Box<dyn EnrollmentEntropy>),
            },
        )
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
        audience,
        address,
        registry,
        peers,
        audit,
        clock: wall,
        environments,
        relay,
        advertised_address,
        bound_socket,
        #[cfg(unix)]
        tunnel,
    } = prepared;
    let bind_failed = |source| RunError::Native(NativeFailure::Bind { address, source });
    let listener = match bound_socket {
        Some(socket) => {
            let bound = socket.local_addr().map_err(bind_failed)?;
            if bound != address {
                return Err(bind_failed(std::io::Error::new(
                    ErrorKind::InvalidInput,
                    "prebound listener is not the configured address",
                )));
            }
            socket
        }
        None => TcpListener::bind(address).await.map_err(bind_failed)?,
    };
    let socket = TcpEnrollmentAccept::new(listener);
    let bound = socket.local_address().map_err(bind_failed)?;
    // The configured port is the one a phone can keep. Binding port zero
    // assigns an ephemeral port that the next start will not repeat, so that
    // listener stays unpublished. A wildcard with a fixed port publishes the
    // configured address on that port.
    let route = (address.port() != 0 && bound.ip().is_unspecified())
        .then_some(advertised_address)
        .flatten();
    if address.port() != 0 {
        if let Some(endpoint) = advertised_endpoint(bound, route) {
            gateway
                .publish_native_endpoint(endpoint)
                .map_err(|error| RunError::Native(NativeFailure::Open(error)))?;
        }
    }
    let sessions = NativeSessions::new(product, Arc::new(RegistryDevices(registry)));
    let connections = NativeEnrollmentConnections::new(gateway.clone(), clock.clone())
        .with_protected_sessions(Arc::new(sessions));
    let connections = Arc::new(match environments {
        Some(environments) => connections.with_environment_peers(environments),
        None => connections,
    });
    // With no relay the sender is dropped here, and the listener's relay
    // source is closed from the start.
    let (joined, relayed) = mpsc::channel::<Relayed<TcpStream, RelayPermit>>(RELAY_CAPACITY);
    let relay = relay.map(|relay| ThroughRelay {
        dial: Arc::new(TcpRelayDial(relay.as_str().to_owned())),
        key: Arc::new(IdentityKey(gateway.clone())),
        admission: connections.clone(),
        timer: Arc::new(TokioTimer::new()),
        joined,
    });
    // Likewise with no tunnel: the listener's tunnel source is closed.
    #[cfg_attr(not(unix), allow(unused_variables))]
    let (joined, tunneled) = mpsc::channel::<Tunneled>(TUNNEL_CAPACITY);
    #[cfg(unix)]
    // Bound only for a tunnel that runs, so a setting Settings can still
    // fix (a token that cannot be read) never holds the port it would need.
    // A port another program holds fails the tunnel, not the gateway.
    let tunnel = match tunnel.filter(|tunnel| tunnel.setting().is_some()) {
        Some(tunnel) => match TcpListener::bind(tunnel.ingress()).await {
            Ok(listener) => Some((
                TunnelIngress::new(listener, connections.clone(), joined),
                tunnel,
            )),
            Err(error) => {
                tracing::warn!(
                    address = %tunnel.ingress(),
                    %error,
                    "the tunnel's ingress cannot be bound; the tunnel is not started"
                );
                tunnel.ingress_failed();
                None
            }
        },
        None => None,
    };
    let accept = TunneledAccept::new(RelayedAccept::new(socket, relayed), tunneled);
    Ok(BoundNative {
        gateway,
        audience,
        listener: NativeEnrollmentListener::new(accept, connections),
        address: bound,
        configured: address,
        announcer: None,
        relay,
        peers,
        audit,
        clock: wall,
        #[cfg(unix)]
        tunnel,
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
    /// Serving through the relay, if configured: stopped with the listener.
    relay: Option<(watch::Sender<bool>, JoinHandle<()>)>,
    /// Reads what each peer granted; stopped and joined with the listener.
    poller: PeerPoller,
    /// The peer commands: closed once the poller is joined.
    peers: Arc<PeerCommands>,
    /// Their audit: closed and drained once the commands are.
    audit: Arc<DurablePeerAudit>,
    /// The local-network announcement, withdrawn first on stop.
    announced: Option<Box<dyn Announced>>,
    /// The tunnel's ingress and supervisor, and their stop: stopped with
    /// the listener, joined before its drain (each bridge holds a tunnel
    /// permit).
    tunnel: Option<RunningTunnel>,
}

struct RunningTunnel {
    stop: watch::Sender<bool>,
    ingress: JoinHandle<()>,
    supervisor: Option<JoinHandle<()>>,
}

/// Start serving. An accept failure that ends the listener is published on
/// `failure` before the listener drains (design rows P67, S14). The sender lives
/// inside the listener task, so a panic closes the channel (row S15); the root
/// stops the whole gateway on either.
pub(super) fn start(
    bound: BoundNative,
    failure: watch::Sender<Option<ErrorKind>>,
) -> RunningNative {
    let poller = bound.start_poller();
    // Announced only once serving is about to start, and never fatal: a phone
    // can still pair by address.
    let announced = bound.announcer.as_ref().and_then(|announcer| {
        let announcement = bound.announcement()?;
        match announcer.announce(&announcement) {
            Ok(announced) => {
                tracing::info!(
                    address = %announcement.address,
                    "native pairing announced on the local network"
                );
                Some(announced)
            }
            Err(error) => {
                tracing::warn!(%error, "native pairing not announced on the local network");
                None
            }
        }
    });
    let BoundNative {
        gateway,
        listener,
        peers,
        audit,
        relay,
        #[cfg(unix)]
        tunnel,
        ..
    } = bound;
    #[cfg(unix)]
    let tunnel = tunnel.map(|(ingress, prepared)| {
        let (stop, stopped) = watch::channel(false);
        RunningTunnel {
            ingress: tokio::spawn(ingress.serve(stopped.clone())),
            supervisor: super::tunnel::start(prepared, stopped),
            stop,
        }
    });
    #[cfg(not(unix))]
    let tunnel = None;
    let relay = relay.map(|relay| {
        let (stop, stopped) = watch::channel(false);
        (stop, tokio::spawn(relay.serve(Backoff::default(), stopped)))
    });
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
        relay,
        poller,
        peers,
        audit,
        announced,
        tunnel,
    }
}

impl RunningNative {
    /// Ask the listener to stop admitting; it wakes its peers at once. Called
    /// before the cleanup owner waits on anything else (design row S12).
    pub(super) fn signal_stop(&mut self) {
        // A phone stops finding a gateway that is about to stop answering.
        if let Some(announced) = self.announced.take() {
            announced.withdraw();
        }
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some((stop, _)) = &self.relay {
            stop.send_replace(true);
        }
        if let Some(tunnel) = &self.tunnel {
            tunnel.stop.send_replace(true);
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
        // waited on: they hold sockets to peers and those peers' turns.
        self.poller.join().await;
        // Then the owner's peer commands: none takes the turn from now (it
        // is refused, with its outcome, before any effect), and those already
        // between their intent and their outcome are waited for, so no
        // effect is left without its outcome when the audit closes.
        self.peers.close(PREEMPT).await;
        // Then the peer audit: closed, so a command from now is refused
        // before its intent, and what was queued, the poller's last records
        // and the commands' outcomes included, written within one bound on
        // the deadline clock.
        if !self.audit.drained(AUDIT_DEADLINE).await {
            tracing::error!("peer gateway audit not drained before shutdown");
        }
        // The relay registration and its accept legs, which hold relay
        // permits the listener's drain waits for.
        if let Some((_, relay)) = self.relay.take() {
            if let Err(error) = relay.await {
                tracing::error!(?error, "relay registration task fault");
            }
        }
        // The tunnel's bridges, which hold tunnel permits, and its
        // supervisor, which leaves the provider's program for the next run.
        if let Some(tunnel) = self.tunnel.take() {
            for task in std::iter::once(tunnel.ingress).chain(tunnel.supervisor) {
                if let Err(error) = task.await {
                    tracing::error!(?error, "tunnel task fault");
                }
            }
        }
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

/// The address discovery may publish. A named bind is that address. A wildcard
/// bind is the configured `advertised` host on the bound port. Both paths ask
/// `OwnerPhoneRequest::validate_endpoint` before disclosure. Wildcards additionally
/// require a non-loopback advertised host of the listen family; a concrete
/// loopback endpoint remains usable by local clients.
/// Callers pass the configured port: a port of zero here is unpublished, and
/// `bind` does not substitute the ephemeral port.
fn advertised_endpoint(bound: SocketAddr, advertised: Option<IpAddr>) -> Option<SocketAddr> {
    let endpoint = if bound.ip().is_unspecified() {
        let ip = advertised
            .filter(|ip| routable_advertised_host(*ip) && ip.is_ipv4() == bound.is_ipv4())?;
        SocketAddr::new(ip, bound.port())
    } else {
        bound
    };
    OwnerPhoneRequest::validate_endpoint(endpoint).ok()?;
    Some(endpoint)
}

#[cfg(test)]
#[path = "../../tests/composition/native_pairing.rs"]
mod tests;
