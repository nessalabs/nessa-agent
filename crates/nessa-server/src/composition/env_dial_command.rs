//! `nessa env enroll` and `nessa env dial`: a machine the gateway cannot
//! reach pairs with it as an environment, then keeps itself dialed in and
//! serves the leases that come over its own connection (issue #706, part 1c).
//!
//! ```text
//! env enroll ADDRESS CODE ──▶ FilePairingEnvironment (environment/pairing)
//!   ──▶ the gateway's listener ──▶ pending until the owner approves this
//!   machine's key there
//! env dial ADDRESS ──▶ approval(FilePairingEnvironment) until approved (the
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
        application::{self, redial, Backoff, NotApproved},
        infrastructure::{dial, Approved, FilePairingEnvironment},
    },
};
use nessa_auth::adapters::pairing::{ManualCode, NativeIdentity};
use nessa_protocol::clock::Clock;
use std::{
    future::Future,
    io::{self, Write},
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{ReadHalf, WriteHalf},
    sync::watch,
};

/// How often `env dial` asks whether the owner has approved this machine.
const APPROVAL_POLL: Duration = Duration::from_secs(5);

/// `nessa env enroll ADDRESS CODE`.
///
/// # Errors
/// The address or code is not one, the gateway could not be reached, or it
/// did not pair this machine.
pub(super) async fn enroll(address: &str, code: &str) -> Result<(), RunError> {
    let code = ManualCode::parse(code.as_bytes())
        .map_err(|_| RunError::Agent("that is not a pairing code".into()))?;
    check_address(address)?;
    let pairing = open(address)?;
    let enrolled = pairing.enroll(code).await;
    pairing.shutdown().await;
    enrolled.map_err(RunError::Agent)?;
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
) -> Result<Option<Approved>, RunError> {
    let pairing = open(address)?;
    let approved = application::approval(&pairing, APPROVAL_POLL, stopping.clone()).await;
    pairing.shutdown().await;
    approved.map_err(|refused| {
        RunError::Agent(match refused {
            NotApproved::NotPaired => {
                "this machine is not paired: run nessa env enroll first".into()
            }
            NotApproved::Ended => "the gateway ended this pairing without approving it \
                                   (denied, cancelled or expired): run nessa env enroll with \
                                   a new code"
                .into(),
            NotApproved::Unusable(reason) => reason,
        })
    })
}

fn open(address: &str) -> Result<FilePairingEnvironment, RunError> {
    FilePairingEnvironment::open(&environment_directory()?, address, clock())
        .map_err(RunError::Agent)
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

fn clock() -> Arc<dyn Clock> {
    RuntimeDependencies::default().clock
}
