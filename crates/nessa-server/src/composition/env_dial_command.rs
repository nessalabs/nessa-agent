//! `nessa env enroll` and `nessa env dial`: a machine the gateway cannot
//! reach pairs with it as an environment, then keeps itself dialed in and
//! serves the leases that come over its own connection (issue #706, part 1c).
//!
//! ```text
//! env enroll ADDRESS CODE ──▶ <data>/environment/pairing (FilePairingState)
//!   ──▶ NativeEnrollmentClient (PeerEnvironment) ──▶ the gateway's listener
//!   ──▶ pending until the owner approves this machine's key there
//! env dial ADDRESS ──▶ compose, as env serve does (lock, ledger, launcher)
//!   ──▶ ask the pairing's status until approved (the credential is saved)
//!   ──▶ redial(dial, Composed::serve) until SIGINT or SIGTERM ──▶ finish
//! ```
//!
//! Arrows are construction, then calls in order. The credential the
//! gateway issues is never sent: each connection proves this machine's key
//! in TLS, and the gateway asks its registry who that key is.
use super::env_serve_command::{compose, environment_directory, Composed};
use crate::{
    app::dependencies::RuntimeDependencies,
    core::RunError,
    device_pairing::infrastructure::TlsStream,
    env_serve::{
        application::{redial, Backoff},
        infrastructure::dial,
    },
};
use nessa_auth::{
    adapters::pairing::{FilePairingState, ManualCode, NativeIdentity, OsEntropy},
    application::pairing::ClientPendingStore,
    domain::pairing::ConsentClass,
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_protocol::{clock::Clock, pairing::wire::NativePairingStatus};
use std::{
    future::Future,
    io::{self, Write},
    net::{SocketAddr, TcpStream, ToSocketAddrs},
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::sync::watch;

/// The pairing's directory, beneath `<data>/environment`.
const PAIRING: &str = "pairing";
/// How long a TCP connect to the gateway may take when pairing.
const CONNECT: Duration = Duration::from_secs(5);
/// How often `env dial` asks whether the owner has approved this machine.
const APPROVAL_POLL: Duration = Duration::from_secs(5);

/// This machine's pairing as an environment: its key, the gateway's pin,
/// and, once approved, its credential.
pub(super) struct EnvironmentPairing {
    client: NativeEnrollmentClient,
    store: Arc<FilePairingState>,
    directory: PathBuf,
}

impl EnvironmentPairing {
    /// The pairing kept beneath `environment`, created empty when absent.
    pub(super) fn open(environment: &Path, clock: Arc<dyn Clock>) -> Result<Self, RunError> {
        let root = environment.canonicalize().map_err(storage)?;
        nessa_local_storage::create_directory_beneath(&root, Path::new(PAIRING))
            .map_err(storage)?;
        let store = Arc::new(
            FilePairingState::open(&root, Path::new(PAIRING))
                .map_err(|error| RunError::Agent(format!("{error:?}")))?,
        );
        let client =
            NativeEnrollmentClient::enrolling(ConsentClass::PeerEnvironment, store.clone(), clock);
        Ok(Self {
            client,
            store,
            directory: root.join(PAIRING),
        })
    }

    /// Pair with the gateway on `stream` using the owner's `code`.
    pub(super) async fn enroll(
        &self,
        stream: TcpStream,
        code: ManualCode,
    ) -> Result<NativePairingStatus, RunError> {
        self.client
            .enroll(stream, code, OsEntropy)
            .await
            .map_err(|error| self.enroll_error(error))
    }

    /// This machine's key and the gateway's pin, once the owner has approved
    /// the pairing: until then, ask its status every `poll`, connecting
    /// with `connect`.
    ///
    /// # Errors
    /// Nothing was ever paired here, or the pairing ended without approval.
    /// A gateway that cannot be reached is asked again.
    pub(super) async fn approved<Connect, Connected>(
        &self,
        connect: Connect,
        poll: Duration,
    ) -> Result<(Arc<NativeIdentity>, [u8; 44]), RunError>
    where
        Connect: Fn() -> Connected,
        Connected: Future<Output = io::Result<TcpStream>>,
    {
        let mut said = false;
        let mut active = false;
        loop {
            if let Some(saved) = self
                .store
                .load_credential()
                .map_err(|error| RunError::Agent(format!("{error:?}")))?
            {
                let (key, pin, _) = saved.into_enrollment().into_parts();
                let identity = NativeIdentity::restore(key)
                    .map_err(|error| RunError::Agent(format!("{error:?}")))?;
                return Ok((Arc::new(identity), pin));
            }
            match connect().await {
                Err(error) => tracing::warn!(%error, "the gateway could not be reached"),
                // Active saves the credential, which the next turn reads.
                Ok(stream) => match self.client.status(stream, None).await {
                    Ok(NativePairingStatus::Unclaimed { .. }) => {
                        return Err(RunError::Agent(format!(
                            "the pairing ended without being approved; remove {} and \
                             enroll again with a new code",
                            self.directory.display()
                        )))
                    }
                    // Read at once, unless it was not saved the last time.
                    Ok(NativePairingStatus::Active { .. }) if !active => {
                        active = true;
                        continue;
                    }
                    Ok(_) if !said => {
                        said = true;
                        tracing::info!("waiting for the owner to approve this machine");
                    }
                    Ok(_) => {}
                    Err(NativeClientError::NoPending) => {
                        return Err(RunError::Agent(
                            "this machine is not paired: run nessa env enroll first".into(),
                        ))
                    }
                    Err(error) => tracing::warn!(?error, "the pairing's status was not read"),
                },
            }
            tokio::time::sleep(poll).await;
        }
    }

    /// Let go of the pairing client's worker.
    pub(super) async fn shutdown(&self) {
        self.client.shutdown().await;
    }

    fn enroll_error(&self, error: NativeClientError) -> RunError {
        RunError::Agent(match error {
            NativeClientError::Enrolled => "this machine is already paired".into(),
            NativeClientError::PendingExists => format!(
                "this machine has a pairing waiting for approval: run nessa env dial, or \
                 remove {} to start over",
                self.directory.display()
            ),
            NativeClientError::OtherEnrollee => {
                "that code does not pair an environment: ask for an environment invitation".into()
            }
            NativeClientError::Refused => {
                "the gateway refused: the code may be wrong, used or expired".into()
            }
            NativeClientError::Crypto(_) => "the code is not the one the gateway gave".into(),
            other => format!("pairing failed: {other:?}"),
        })
    }
}

/// `nessa env enroll ADDRESS CODE`.
///
/// # Errors
/// The address or code is not one, the gateway could not be reached, or it
/// did not pair this machine.
pub(super) async fn enroll(address: &str, code: &str) -> Result<(), RunError> {
    let code = ManualCode::parse(code.as_bytes())
        .map_err(|_| RunError::Agent("that is not a pairing code".into()))?;
    let gateway = resolve(address)?;
    let pairing = EnvironmentPairing::open(&environment_directory()?, clock())?;
    let enrolled = match TcpStream::connect_timeout(&gateway, CONNECT) {
        Ok(stream) => pairing.enroll(stream, code).await,
        Err(error) => Err(RunError::Agent(format!("{address}: {error}"))),
    };
    pairing.shutdown().await;
    enrolled?;
    writeln!(
        io::stdout(),
        "Paired. Approve this machine on the gateway, then run: nessa env dial {address}"
    )?;
    Ok(())
}

/// `nessa env dial ADDRESS`: serve the gateway at `address` over
/// connections this machine dials, until SIGINT or SIGTERM.
///
/// # Errors
/// This host cannot serve, is not paired, or its pairing cannot dial.
pub(super) async fn dial_in(address: &str) -> Result<(), RunError> {
    resolve(address)?;
    let composed = compose().await.map_err(|(_, failure)| failure)?;
    let (stop, stopping) = watch::channel(false);
    let signals = tokio::spawn(stop_on_signal(stop));
    let served = serve_dialing(&composed, address, stopping).await;
    signals.abort();
    composed.finish().await;
    served
}

async fn serve_dialing(
    composed: &Composed,
    address: &str,
    stopping: watch::Receiver<bool>,
) -> Result<(), RunError> {
    let pairing = EnvironmentPairing::open(&environment_directory()?, clock())?;
    let connect = || {
        let address = address.to_owned();
        async move {
            tokio::task::spawn_blocking(move || {
                let gateway =
                    resolve(&address).map_err(|error| io::Error::other(error.to_string()))?;
                TcpStream::connect_timeout(&gateway, CONNECT)
            })
            .await
            .map_err(io::Error::other)?
        }
    };
    let approved = tokio::select! {
        approved = pairing.approved(connect, APPROVAL_POLL) => approved,
        _ = stopped(stopping.clone()) => {
            pairing.shutdown().await;
            return Ok(());
        }
    };
    pairing.shutdown().await;
    let (identity, pin) = approved?;
    tracing::info!(%address, "approved; serving the gateway");
    redial(
        || {
            let identity = identity.clone();
            async move {
                // Resolved on every dial: the gateway's address may move.
                let gateway = tokio::net::lookup_host(address)
                    .await?
                    .next()
                    .ok_or_else(|| io::Error::new(io::ErrorKind::NotFound, "no address"))?;
                dial(gateway, identity, pin, clock()).await
            }
        },
        |stream, stopping| serve_until_stopped(composed, stream, stopping),
        Backoff::default(),
        stopping,
    )
    .await
    .map_err(|error| RunError::Agent(format!("{address}: {error}")))
}

/// Serve one dialed connection; closing it once stop is asked, so its
/// leases end as they do when a gateway goes away.
async fn serve_until_stopped(
    composed: &Composed,
    stream: TlsStream,
    stopping: watch::Receiver<bool>,
) {
    let closer = stream.closer();
    let closing = tokio::spawn(async move {
        stopped(stopping).await;
        closer.close();
    });
    let (input, output) = tokio::io::split(stream);
    composed.serve(input, output).await;
    closing.abort();
}

async fn stopped(mut stopping: watch::Receiver<bool>) {
    if stopping.wait_for(|stop| *stop).await.is_err() {
        std::future::pending::<()>().await;
    }
}

async fn stop_on_signal(stop: watch::Sender<bool>) {
    use tokio::signal::unix::{signal, SignalKind};
    let (Ok(mut interrupt), Ok(mut terminate)) = (
        signal(SignalKind::interrupt()),
        signal(SignalKind::terminate()),
    ) else {
        tracing::error!("signals cannot stop nessa env dial; it stops when killed");
        return std::future::pending().await;
    };
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    let _ = stop.send(true);
}

fn resolve(address: &str) -> Result<SocketAddr, RunError> {
    address
        .to_socket_addrs()
        .map_err(|error| RunError::Agent(format!("{address}: {error}")))?
        .next()
        .ok_or_else(|| RunError::Agent(format!("{address}: no address")))
}

fn clock() -> Arc<dyn Clock> {
    RuntimeDependencies::default().clock
}

fn storage(error: io::Error) -> RunError {
    RunError::Agent(error.to_string())
}
