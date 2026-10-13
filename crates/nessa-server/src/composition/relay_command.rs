//! `nessa relay serve --listen HOST:PORT`: run a relay until stopped
//! (issue #266, `docs/design/auth/relay.md`).
//!
//! ```text
//! bind HOST:PORT ──▶ Relay (TokioTimer, OsRelayEntropy, Ed25519Proofs, default limits)
//!   ──▶ serve_tcp until SIGINT or SIGTERM (Ctrl-C elsewhere) ──▶ every connection closed
//! ```
//!
//! Arrows are construction, then calls in order. The relay keeps nothing on
//! disk and reads no configuration: everything it holds ends with it.
use crate::{
    core::RunError,
    relay::{
        application::{Relay, RelayLimits},
        infrastructure::{serve_tcp, Ed25519Proofs, OsRelayEntropy},
    },
    timer::{Timer, TokioTimer},
};
use std::{
    io::{self, Write},
    sync::Arc,
};
use tokio::net::TcpListener;

/// `nessa relay serve --listen HOST:PORT`.
///
/// # Errors
/// The address cannot be listened on.
pub(super) async fn execute(listen: &str) -> Result<(), RunError> {
    let listener = TcpListener::bind(listen)
        .await
        .map_err(|error| RunError::Agent(format!("{listen}: cannot listen: {error}")))?;
    let address = listener.local_addr()?;
    let timer: Arc<dyn Timer> = Arc::new(TokioTimer::new());
    let relay = Relay::new(
        timer.clone(),
        Arc::new(OsRelayEntropy),
        Arc::new(Ed25519Proofs),
        RelayLimits::default(),
    );
    // Best effort: a closed or broken stderr must not stop a bound relay.
    let _ = writeln!(io::stderr(), "Relay listening on {address}");
    serve_tcp(listener, relay, timer, stopped()).await;
    Ok(())
}

/// Resolves on the first SIGINT or SIGTERM, or on Ctrl-C where there are
/// no Unix signals. A relay that cannot hear them stops only when killed.
async fn stopped() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let (Ok(mut interrupt), Ok(mut terminate)) = (
            signal(SignalKind::interrupt()),
            signal(SignalKind::terminate()),
        ) else {
            tracing::error!("signals cannot stop nessa relay serve; it stops when killed");
            return std::future::pending().await;
        };
        tokio::select! {
            _ = interrupt.recv() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    if tokio::signal::ctrl_c().await.is_err() {
        tracing::error!("Ctrl-C cannot stop nessa relay serve; it stops when killed");
        std::future::pending::<()>().await;
    }
}
