//! `nessa env enroll` and `nessa env dial`: a machine the gateway cannot
//! reach pairs with it as an environment, then keeps itself dialed in and
//! serves the leases that come over its own connection (issue #706, part 1c).
//!
//! ```text
//! env enroll ADDRESS CODE ──▶ <data>/environment/pairing (FilePairingState)
//!   ──▶ NativeEnrollmentClient (PeerEnvironment) ──▶ the gateway's listener
//!   ──▶ pending until the owner approves this machine's key there
//! env dial ADDRESS ──▶ ask the pairing's status until approved (the
//!   credential is saved) ──▶ let go of the pairing
//!   ──▶ compose, as env serve does (lock, ledger, launcher)
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
    application::pairing::{ClientPendingStore, PrivateStateError},
    domain::pairing::ConsentClass,
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_protocol::{clock::Clock, pairing::wire::NativePairingStatus};
use std::{
    future::Future,
    io::{self, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{ReadHalf, WriteHalf},
    sync::watch,
};

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
        let store =
            Arc::new(
                FilePairingState::open(&root, Path::new(PAIRING)).map_err(|error| match error {
                    PrivateStateError::Locked => RunError::Agent(
                        "another nessa env enroll or nessa env dial is using this machine's \
                     pairing"
                            .into(),
                    ),
                    other => RunError::Agent(format!("{other:?}")),
                })?,
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
        loop {
            let store = self.store.clone();
            // Pending first: a pending record only ever becomes a
            // credential, so a pairing that exists never reads as neither.
            let (pending, saved) = tokio::task::spawn_blocking(move || {
                Ok::<_, PrivateStateError>((store.load_pending()?, store.load_credential()?))
            })
            .await
            .map_err(|error| RunError::Agent(error.to_string()))?
            .map_err(|error| RunError::Agent(format!("the pairing was not read: {error:?}")))?;
            if saved.is_none() && pending.is_none() {
                return Err(not_paired());
            }
            if let Some(saved) = saved {
                let (key, pin, _) = saved.into_enrollment().into_parts();
                let identity = NativeIdentity::restore(key)
                    .map_err(|error| RunError::Agent(format!("{error:?}")))?;
                return Ok((Arc::new(identity), pin));
            }
            match connect().await {
                Err(error) => tracing::warn!(%error, "the gateway could not be reached"),
                Ok(stream) => match self.client.status(stream, None).await {
                    // Reading either status ends the enrollment here, so a
                    // new code enrolls again at once.
                    Ok(
                        NativePairingStatus::Unclaimed { .. }
                        | NativePairingStatus::Terminal { .. },
                    ) => {
                        return Err(RunError::Agent(
                            "the gateway ended this pairing without approving it (denied, \
                             cancelled or expired): run nessa env enroll with a new code"
                                .into(),
                        ))
                    }
                    // Active is read only once the credential is saved:
                    // the top of the loop returns it.
                    Ok(NativePairingStatus::Active { .. }) => continue,
                    Ok(_) if !said => {
                        said = true;
                        tracing::info!("waiting for the owner to approve this machine");
                    }
                    Ok(_) => {}
                    Err(NativeClientError::NoPending) => return Err(not_paired()),
                    // A pairing record this machine cannot use, or a
                    // gateway answering out of turn: no later ask changes it.
                    Err(
                        error @ (NativeClientError::Storage(
                            PrivateStateError::Corrupt | PrivateStateError::Conflict,
                        )
                        | NativeClientError::Crypto(_)
                        | NativeClientError::Wire(_)
                        | NativeClientError::Phase
                        | NativeClientError::OtherEnrollee),
                    ) => {
                        return Err(RunError::Agent(format!(
                            "the pairing's status cannot be read: {error:?}; remove {} and \
                             enroll again",
                            self.directory.display()
                        )))
                    }
                    // Anything else may clear: the gateway out of reach, busy
                    // or refusing, or a save that failed, which leaves the
                    // pending record for the next ask to deliver again.
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
            NativeClientError::Enrolled => format!(
                "this machine is already paired: run nessa env dial, or remove {} to pair \
                 again",
                self.directory.display()
            ),
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
    check_address(address)?;
    let pairing = EnvironmentPairing::open(&environment_directory()?, clock())?;
    let enrolled = match connect(address).await {
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

/// `nessa env dial ADDRESS`: once the owner has approved this machine,
/// serve the gateway at `address` over connections this machine dials,
/// until SIGINT or SIGTERM.
///
/// The name is looked up on every connection, not here: started at boot,
/// before names resolve, it keeps trying.
///
/// # Errors
/// The address is not `HOST:PORT`, this machine is not paired, this host
/// cannot serve, or its pairing cannot dial.
pub(super) async fn dial_in(address: &str) -> Result<(), RunError> {
    check_address(address)?;
    let (stop, stopping) = watch::channel(false);
    let signals = tokio::spawn(stop_on_signal(stop));
    let served = approved_then_served(address, stopping).await;
    signals.abort();
    served
}

/// Composed only once approved, so an unpaired machine says so at once and
/// `env serve` is not locked out while the owner decides.
async fn approved_then_served(
    address: &str,
    stopping: watch::Receiver<bool>,
) -> Result<(), RunError> {
    let Some((identity, pin)) = approval(address, &stopping).await? else {
        return Ok(());
    };
    tracing::info!(%address, "approved; serving the gateway");
    let composed = compose().await.map_err(|(_, failure)| failure)?;
    let served = serve_dialing(&composed, address, identity, pin, stopping).await;
    composed.finish().await;
    served
}

/// This machine's key and the gateway's pin, or `None` when stop was asked
/// first. The pairing, and its directory's lock, is let go either way.
async fn approval(
    address: &str,
    stopping: &watch::Receiver<bool>,
) -> Result<Option<(Arc<NativeIdentity>, [u8; 44])>, RunError> {
    let pairing = EnvironmentPairing::open(&environment_directory()?, clock())?;
    let approved = tokio::select! {
        approved = pairing.approved(|| connect(address), APPROVAL_POLL) => Some(approved),
        () = stopped(stopping.clone()) => None,
    };
    pairing.shutdown().await;
    approved.transpose()
}

async fn serve_dialing(
    composed: &Composed,
    address: &str,
    identity: Arc<NativeIdentity>,
    pin: [u8; 44],
    stopping: watch::Receiver<bool>,
) -> Result<(), RunError> {
    redial(
        || {
            let identity = identity.clone();
            async move {
                // Resolved on every dial, the gateway's address may move;
                // each address is tried, as a TCP connect by name does.
                let mut failed = io::Error::new(io::ErrorKind::NotFound, "no address");
                for gateway in tokio::net::lookup_host(address).await? {
                    match dial(gateway, identity.clone(), pin, clock()).await {
                        Ok(stream) => return Ok(stream),
                        Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                            return Err(error)
                        }
                        Err(error) => failed = error,
                    }
                }
                Err(failed)
            }
        },
        |stream, stopping| {
            serve_until_stopped(stream, stopping, |input, output| {
                composed.serve(input, output)
            })
        },
        Backoff::default(),
        stopping,
    )
    .await
    .map_err(|error| RunError::Agent(format!("{address}: {error}")))
}

/// Serve one dialed connection with `serve`, closing it once stop is asked
/// so its leases end as they do when a gateway goes away.
pub(super) async fn serve_until_stopped<Serve, Served>(
    stream: TlsStream,
    stopping: watch::Receiver<bool>,
    serve: Serve,
) where
    Serve: FnOnce(ReadHalf<TlsStream>, WriteHalf<TlsStream>) -> Served,
    Served: Future<Output = ()>,
{
    let closer = stream.closer();
    let closing = tokio::spawn(async move {
        stopped(stopping).await;
        closer.close();
    });
    let (input, output) = tokio::io::split(stream);
    serve(input, output).await;
    closing.abort();
}

async fn stopped(mut stopping: watch::Receiver<bool>) {
    if stopping.wait_for(|stop| *stop).await.is_err() {
        std::future::pending::<()>().await;
    }
}

/// The first SIGINT or SIGTERM asks stop; a second, while stopping drains,
/// exits at once.
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
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    tracing::warn!("asked again while stopping: exiting now");
    std::process::exit(130);
}

/// `address` has the form `HOST:PORT`; the name is not looked up.
fn check_address(address: &str) -> Result<(), RunError> {
    let fits = address.rsplit_once(':').is_some_and(|(host, port)| {
        !host.is_empty() && port.parse::<u16>().is_ok_and(|port| port != 0)
    });
    if fits {
        Ok(())
    } else {
        Err(RunError::Agent(format!(
            "{address}: give the gateway as HOST:PORT"
        )))
    }
}

/// A blocking stream to the gateway for the pairing client, connected
/// without blocking the runtime.
async fn connect(address: &str) -> io::Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??
        .into_std()?;
    stream.set_nonblocking(false)?;
    Ok(stream)
}

fn not_paired() -> RunError {
    RunError::Agent("this machine is not paired: run nessa env enroll first".into())
}

fn clock() -> Arc<dyn Clock> {
    RuntimeDependencies::default().clock
}

fn storage(error: io::Error) -> RunError {
    RunError::Agent(error.to_string())
}
