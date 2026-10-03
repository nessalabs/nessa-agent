//! Native device pairing, mounted when `config.json` names a native listen
//! address: private state and the gateway key before anything is bound, one
//! connection owner per gateway, and a stop that joins the gateway's drain.
//!
//! ```text
//! prepare: native-pairing/ --> FilePairingState --> restore_gateway_identity
//!                          --> GatewayPairing::open --> (PreparedNative, PairingOwnerCommands)
//! bind:    PreparedNative --> TcpEnrollmentAccept --> the one NativeEnrollmentConnections
//! start:   BoundNative --> listener task (failed --> watch) --> RunningNative
//! stop:    signal_stop --> join: listener drain, then GatewayPairing::shutdown
//! ```
//! Arrows are construction and ownership handoffs, in order. Design rows
//! S1–S14 in `docs/design/auth/device-pairing.md` ("Owner routes and mounting").
use super::runtime_config::NativeConfig;
use crate::app::dependencies::RuntimeDependencies;
use crate::core::{NativeFailure, NativeShutdownFailure, RunError};
use crate::device_pairing::infrastructure::{
    restore_gateway_identity, GatewayPairing, InvitationEntropy, NativeEnrollmentConnections,
    NativeEnrollmentListener, PairingOwnerCommands, PairingRuntimeDependencies,
    TcpEnrollmentAccept,
};
use nessa_auth::{
    adapters::{
        local::LocalCredentialStore,
        pairing::{FilePairingState, OsEntropy},
    },
    application::{
        pairing::PairingWorkerFault,
        ports::{Clock, PolicyEvaluator},
    },
    domain::Resource,
};
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

/// What startup needs from the rest of composition to prepare native pairing.
pub(super) struct NativeInputs {
    /// The namespace directory: the parent of `auth/`.
    pub namespace: PathBuf,
    pub registry: Arc<LocalCredentialStore>,
    pub policy: Arc<dyn PolicyEvaluator>,
    pub clock: Arc<dyn Clock>,
    /// The gateway resource every invitation is for, from the registry.
    pub gateway: Resource,
}

/// A gateway whose key is restored and whose enrollments are settled, not yet
/// bound. Not `Clone`: `bind` consumes it, so one opened gateway gets one
/// connection owner (design row S13).
pub(super) struct PreparedNative {
    gateway: Arc<GatewayPairing>,
    address: SocketAddr,
}

/// Prepare native pairing before either socket is bound (design rows S3–S6):
/// the private directory and state, then the gateway key (restored, or first
/// published through Auth's guarded publication), then `GatewayPairing::open`,
/// which settles this gateway's unfinished enrollments. The owner commands are
/// for the product socket; they give no way back to the runtime.
pub(super) async fn prepare(
    config: &NativeConfig,
    inputs: NativeInputs,
) -> Result<(PreparedNative, PairingOwnerCommands), RunError> {
    // The private-state owner wants a trusted absolute root; on Unix that is
    // the canonical spelling, as its own tests use.
    #[cfg(unix)]
    let root = inputs
        .namespace
        .canonicalize()
        .map_err(|error| RunError::Native(NativeFailure::Directory(error)))?;
    #[cfg(not(unix))]
    let root = inputs.namespace.clone();
    nessa_local_storage::create_directory_beneath(&root, Path::new(PRIVATE_DIRECTORY))
        .map_err(|error| RunError::Native(NativeFailure::Directory(error)))?;
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
    let gateway = Arc::new(
        GatewayPairing::open(PairingRuntimeDependencies {
            enrollments: registry.clone(),
            access: registry,
            policy: inputs.policy,
            clock: inputs.clock,
            gateway: inputs.gateway,
            key_store: keys,
            identity,
        })
        .map_err(|error| RunError::Native(NativeFailure::Open(error)))?,
    );
    let commands = PairingOwnerCommands::new(
        gateway.clone(),
        Arc::new(|| Box::new(OsEntropy) as Box<dyn InvitationEntropy>),
    );
    Ok((
        PreparedNative {
            gateway,
            address: config.listen_address,
        },
        commands,
    ))
}

/// Native pairing with its socket bound and its one connection owner built.
pub(super) struct BoundNative {
    gateway: Arc<GatewayPairing>,
    listener: NativeEnrollmentListener,
    address: SocketAddr,
}

impl BoundNative {
    /// The address the listener is bound to.
    pub(super) fn local_address(&self) -> SocketAddr {
        self.address
    }
}

/// Bind the configured address and build this gateway's only connection owner
/// (design rows S10, S13). A bind failure leaves the key and enrollment history
/// as `prepare` found them.
pub(super) async fn bind(prepared: PreparedNative) -> Result<BoundNative, RunError> {
    let PreparedNative { gateway, address } = prepared;
    let bind_failed = |source| RunError::Native(NativeFailure::Bind { address, source });
    let socket = TcpEnrollmentAccept::new(TcpListener::bind(address).await.map_err(bind_failed)?);
    let bound = socket.local_address().map_err(bind_failed)?;
    let connections = Arc::new(NativeEnrollmentConnections::new(
        gateway.clone(),
        RuntimeDependencies::default().clock,
    ));
    Ok(BoundNative {
        gateway,
        listener: NativeEnrollmentListener::new(socket, connections),
        address: bound,
    })
}

/// The serving listener, and how to stop it.
pub(super) struct RunningNative {
    gateway: Arc<GatewayPairing>,
    stop: Option<oneshot::Sender<()>>,
    task: JoinHandle<IoResult<()>>,
}

/// Start serving. An accept failure that ends the listener is published on `failure`
/// before the listener drains (design rows P67, S14); the root stops the whole
/// gateway on it.
pub(super) fn start(
    bound: BoundNative,
    failure: watch::Sender<Option<ErrorKind>>,
) -> RunningNative {
    let BoundNative {
        gateway, listener, ..
    } = bound;
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
    }
}

impl RunningNative {
    /// Ask the listener to stop admitting; it wakes its peers at once. Called
    /// before the cleanup owner waits on anything else (design row S12).
    pub(super) fn signal_stop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }

    /// Wait for the listener to collect its peers and drain its connection
    /// owner, then close create admission and wait for registration and any
    /// admitted create. An accept failure was already published when it
    /// happened; here only a fault of the listener task is a failure.
    pub(super) async fn join(mut self) -> Result<(), NativeShutdownFailure> {
        self.signal_stop();
        let drained = match self.task.await {
            Ok(_) => Ok(()),
            Err(error) => Err(NativeShutdownFailure::ListenerFault(if error.is_panic() {
                PairingWorkerFault::Panic
            } else {
                PairingWorkerFault::Cancelled
            })),
        };
        self.gateway.shutdown().await;
        drained
    }
}

#[cfg(test)]
#[path = "../../tests/composition/native_pairing.rs"]
mod tests;
