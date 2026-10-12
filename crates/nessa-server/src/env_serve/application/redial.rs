//! How a paired environment stays dialed in (issue #706, part 1c): dial its
//! gateway, serve that connection until it ends, and dial again, waiting
//! longer after each attempt that fails.
//!
//! ```text
//! dial ──▶ connected ──▶ serve until the connection ends ──▶ wait ──▶ dial
//!      ──▶ refused, unreachable, cut ──▶ wait (doubling, capped) ──▶ dial
//!      ──▶ InvalidInput (this side's own setup) ──▶ stop: dialing again
//!          cannot change it
//! stop asked ──▶ the connection being served is closed, its leases end as
//!               they do when a gateway goes away, and dialing stops
//! ```
//!
//! Arrows are steps, in order. A refusal is redacted (see `dial.rs`), so a
//! revoked key is dialed again like a full gateway: the wait caps how often.
use super::timer::Timer;
use std::{future::Future, io, time::Duration};
use tokio::sync::watch;

/// How long to wait between dials.
#[derive(Debug, Clone, Copy)]
pub(crate) struct Backoff {
    /// After the first attempt that fails, and after a connection that was
    /// served for at least [`Backoff::most`].
    pub(crate) first: Duration,
    /// The longest wait; each further failure doubles the wait up to it.
    pub(crate) most: Duration,
}

impl Default for Backoff {
    fn default() -> Self {
        Self {
            first: Duration::from_secs(1),
            most: Duration::from_secs(60),
        }
    }
}

/// Dial with `connect` and serve each connection with `serve` until `stop`
/// says true, or until `connect` fails with [`io::ErrorKind::InvalidInput`].
/// The waits between dials, and how long a connection was served, are
/// measured on `timer`.
///
/// `serve` is given `stop` too: it closes its connection once stop is asked
/// and returns when serving it has ended. A connection served for less than
/// [`Backoff::most`] counts as a failure for the wait, so a gateway that
/// takes each connection and drops it is not dialed in a loop.
///
/// # Errors
/// The `InvalidInput` error that stopped it; `Ok` when stop was asked.
pub(crate) async fn redial<C, Connect, Connected, Serve, Served>(
    mut connect: Connect,
    mut serve: Serve,
    backoff: Backoff,
    timer: &dyn Timer,
    mut stop: watch::Receiver<bool>,
) -> io::Result<()>
where
    Connect: FnMut() -> Connected,
    Connected: Future<Output = io::Result<C>>,
    Serve: FnMut(C, watch::Receiver<bool>) -> Served,
    Served: Future<Output = ()>,
{
    let mut wait = backoff.first;
    loop {
        if *stop.borrow_and_update() {
            return Ok(());
        }
        let dialed = tokio::select! {
            dialed = connect() => dialed,
            _ = stopped(&mut stop) => return Ok(()),
        };
        match dialed {
            Ok(connection) => {
                let since = timer.now();
                serve(connection, stop.clone()).await;
                if timer.now().since(since) >= backoff.most {
                    wait = backoff.first;
                }
                tracing::info!(?wait, "the gateway connection ended; dialing again");
            }
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                tracing::error!(%error, "this environment cannot dial its gateway");
                return Err(error);
            }
            Err(error) => tracing::warn!(%error, ?wait, "dialing the gateway failed"),
        }
        tokio::select! {
            () = timer.sleep(wait) => {}
            _ = stopped(&mut stop) => return Ok(()),
        }
        wait = (wait * 2).min(backoff.most);
    }
}

/// Resolves once stop is asked; never, once nothing can ask it any more.
pub(super) async fn stopped(stop: &mut watch::Receiver<bool>) {
    if stop.wait_for(|stop| *stop).await.is_err() {
        std::future::pending::<()>().await;
    }
}

#[cfg(test)]
#[path = "../../../tests/env_serve/redial.rs"]
mod tests;
