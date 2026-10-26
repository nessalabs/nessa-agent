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
//!   ──▶ redial(dial, Composed::serve as the gateway the pin names) until
//!   SIGINT or SIGTERM ──▶ finish
//! with --relay, each ask and each dial that does not connect directly goes
//!   through the relay (enrolling does not: nothing knows the gateway's key
//!   before it)
//! with --tunnel, each dial that does not connect directly goes through the
//!   gateway's web tunnel too (asks and enrolling do not: the tunnel admits
//!   only a key that already holds a credential)
//! with --enroll-code-file, before all of it: enroll_once(the file) ──▶
//!   enrolled with its code when nothing is paired here, nothing sent when
//!   paired with the gateway at ADDRESS, refused when paired with another
//! with --drain-deadline, the first SIGINT or SIGTERM drains instead: the
//!   connection being served ends every lease with its cleanup and closes,
//!   nothing is dialed again ──▶ finish; stop_past_deadline closes it at the
//!   deadline, and a second signal exits at once
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
        application::{
            self, enroll_once, redial, stop_past_deadline, Backoff, EnrolledOnce, NotApproved,
            NotEnrolled,
        },
        domain::Peer,
        infrastructure::{dial, Approved, FileEnrollmentCode, FilePairingEnvironment},
    },
    timer::TokioTimer,
};
use nessa_auth::adapters::pairing::{ManualCode, NativeIdentity};
use nessa_protocol::{
    clock::Clock,
    relay::{host_port, RelayAddress},
    tunnel::TunnelAddress,
};
use std::{
    future::Future,
    io::{self, Write},
    path::Path,
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
    let enrolled = application::enroll(&pairing, code).await;
    pairing.shutdown().await;
    enrolled.map_err(RunError::Agent)?;
    writeln!(
        io::stdout(),
        "Paired. Approve this machine on the gateway, then run: nessa env dial {address}"
    )?;
    Ok(())
}

/// `nessa env dial ADDRESS [--relay RELAY] [--drain-deadline DURATION]
/// [--enroll-code-file PATH]`: with `relay`, the gateway is asked for there
/// whenever a direct dial does not connect (enrolling still needs the direct
/// path, since only a pairing knows the gateway's key); with `enroll_code_file`, enrolled first when nothing is paired
/// here ([`enroll_once`]); then, once the owner has
/// approved this machine, serve the gateway at `address` over connections
/// this machine dials, until SIGINT or SIGTERM. With `drain_deadline`, that
/// signal drains: every lease ends with its cleanup on both sides before
/// this returns, or the connection is closed once the deadline passes, as a
/// container's stop grace needs.
///
/// The name is looked up on every connection, not here: started at boot,
/// before names resolve, it keeps trying.
///
/// # Errors
/// The address is not `HOST:PORT`, this machine is not paired (or, with a
/// code file, is paired with another gateway, or could not enroll), this
/// host cannot serve, or its pairing cannot dial.
pub(super) async fn dial_in(
    address: &str,
    relay: Option<&RelayAddress>,
    tunnel: Option<&TunnelAddress>,
    drain_deadline: Option<Duration>,
    enroll_code_file: Option<&Path>,
) -> Result<(), RunError> {
    check_address(address)?;
    // Before any signal is taken: a stop while enrolling has nothing to
    // drain, and the process's default ends it.
    if let Some(code_file) = enroll_code_file {
        let pairing = open(address)?;
        let enrolled = enroll_once(&pairing, &FileEnrollmentCode(code_file.into())).await;
        pairing.shutdown().await;
        let enrolled = enrolled.map_err(|refused| {
            RunError::Agent(match refused {
                NotEnrolled::PairedElsewhere(recorded) => format!(
                    "this machine is already paired with {}: run nessa env dial without \
                     --enroll-code-file to serve it, or remove {} to pair it with {address}",
                    recorded.map_or_else(
                        || "a gateway it did not record".to_owned(),
                        |recorded| format!("the gateway at {recorded}")
                    ),
                    pairing.directory().display(),
                ),
                NotEnrolled::Failed(reason) => reason,
            })
        })?;
        match enrolled {
            EnrolledOnce::Now => {
                tracing::info!(%address, "enrolled; waiting for the owner's approval");
            }
            EnrolledOnce::Before => tracing::info!(%address, "already enrolled here"),
        }
    }
    let stop = Arc::new(watch::channel(false).0);
    let stopping = stop.subscribe();
    let (drain, draining) = watch::channel(false);
    let deadline = drain_deadline.map(|deadline| {
        let (stop, draining) = (stop.clone(), draining.clone());
        tokio::spawn(async move {
            stop_past_deadline(draining, deadline, &TokioTimer::new(), &stop).await;
        })
    });
    // Without a deadline nothing drains: the signal stops, as before.
    let drain = drain_deadline.is_some().then_some(drain);
    let signals = tokio::spawn(on_signal(stop, drain));
    let served =
        approved_then_served(address, relay, tunnel, enroll_code_file, stopping, draining).await;
    signals.abort();
    if let Some(deadline) = deadline {
        deadline.abort();
    }
    served
}

/// Composed only once approved, so an unpaired machine says so at once and
/// `env serve` is not locked out while the owner decides.
async fn approved_then_served(
    address: &str,
    relay: Option<&RelayAddress>,
    tunnel: Option<&TunnelAddress>,
    enroll_code_file: Option<&Path>,
    stopping: watch::Receiver<bool>,
    draining: watch::Receiver<bool>,
) -> Result<(), RunError> {
    // A drain asked before anything is served has nothing to drain: it
    // stops the wait for approval as stop does.
    // Each resolves only once asked: a sender gone (no drain without a
    // deadline) never asks.
    let (waiting, drained) = (stopping.clone(), draining.clone());
    let (asked, asking) = watch::channel(false);
    let either = tokio::spawn(async move {
        tokio::select! {
            () = stopped(waiting) => {}
            () = stopped(drained) => {}
        }
        let _ = asked.send(true);
    });
    let approved = approval(address, relay, enroll_code_file, &asking).await;
    let Some((identity, pin)) = approved? else {
        either.abort();
        return Ok(());
    };
    tracing::info!(%address, "approved; serving the gateway");
    // Composing can wait on the serve lock; a stop or drain asked then
    // still has nothing to drain, so it ends the wait. The wait holds
    // nothing until it returns, so dropping it leaves nothing behind.
    let composed = tokio::select! {
        biased;
        () = stopped(asking) => None,
        composed = compose() => Some(composed),
    };
    either.abort();
    let Some(composed) = composed else {
        return Ok(());
    };
    let composed = composed.map_err(|(_, failure)| failure)?;
    let served = serve_dialing(
        &composed, address, relay, tunnel, identity, pin, stopping, draining,
    )
    .await;
    composed.finish().await;
    served
}

/// This machine's key and the gateway's pin, or `None` when stop was asked
/// first. The pairing, and its directory's lock, is let go either way.
async fn approval(
    address: &str,
    relay: Option<&RelayAddress>,
    enroll_code_file: Option<&Path>,
    stopping: &watch::Receiver<bool>,
) -> Result<Option<Approved>, RunError> {
    // Asked through the relay too: the pending pairing knows the gateway's
    // key, as enrolling, before it, does not.
    let pairing = open(address)?.through(relay.cloned());
    let approved = application::approval(
        &pairing,
        APPROVAL_POLL,
        &TokioTimer::new(),
        stopping.clone(),
    )
    .await;
    pairing.shutdown().await;
    approved.map_err(|refused| {
        RunError::Agent(match refused {
            NotApproved::NotPaired => {
                "this machine is not paired: run nessa env enroll first".into()
            }
            // Its pending record is gone: the next start enrolls again, so
            // a container needs a new code in its file, not a new command.
            NotApproved::Ended => format!(
                "the gateway ended this pairing without approving it (denied, cancelled or \
                 expired): {}",
                enroll_code_file.map_or_else(
                    || "run nessa env enroll with a new code".to_owned(),
                    |file| format!("put a new code in {}", file.display())
                )
            ),
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
    relay: Option<&RelayAddress>,
    tunnel: Option<&TunnelAddress>,
    identity: Arc<NativeIdentity>,
    pin: [u8; 44],
    stopping: watch::Receiver<bool>,
    draining: watch::Receiver<bool>,
) -> Result<(), RunError> {
    let drain = draining.clone();
    redial(
        || {
            let identity = identity.clone();
            // Resolved on every dial, the gateway's address may move; each
            // address is tried, as a TCP connect by name does, and then the
            // relay and the tunnel, when there are.
            dial(
                address.to_owned(),
                relay.cloned(),
                tunnel.cloned(),
                identity,
                pin,
                clock(),
            )
        },
        |stream, stopping| {
            // Each connection was dialed to the gateway holding this pin: its
            // grants are that gateway's.
            let drain = drain.clone();
            serve_until_stopped(stream, stopping, |input, output| {
                composed.serve(Peer::Gateway { pin }, input, output, drain)
            })
        },
        Backoff::default(),
        &TokioTimer::new(),
        stopping,
        draining,
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

/// The first SIGINT or SIGTERM asks a drain when there is one to ask, and
/// stop otherwise; a second, while either is under way, exits at once.
async fn on_signal(stop: Arc<watch::Sender<bool>>, drain: Option<watch::Sender<bool>>) {
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
    match &drain {
        Some(drain) => {
            tracing::info!("asked to stop: draining every lease first");
            drain.send_replace(true);
        }
        None => {
            stop.send_replace(true);
        }
    }
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    tracing::warn!("asked again while stopping: exiting now");
    std::process::exit(130);
}

/// `address` has the form `HOST:PORT`, the shape every dial checks, with a
/// port that is not 0; the name is not looked up.
fn check_address(address: &str) -> Result<(), RunError> {
    if host_port(address).is_some_and(|port| port != 0) {
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
