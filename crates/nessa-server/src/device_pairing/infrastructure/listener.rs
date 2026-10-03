//! One bound enrollment listener feeding the connection owner.
//!
//! The listening socket sits behind [`EnrollmentAccept`], so tests can produce
//! accept failures. What a failure means is decided by `accept_step`, design
//! row P67: a failure of one connection is skipped, descriptor exhaustion
//! pauses with a bounded backoff, and anything else is a failure of the
//! listening socket, which ends the run.
use super::worker::worker_fault;
use super::{
    connection::NativeConnectionCompletion, NativeConnectionFailure, NativeEnrollmentConnections,
};
use nessa_auth::adapters::pairing::{CryptoRng, RngCore};
use std::{
    future::Future,
    io::{Error as IoError, ErrorKind, Result as IoResult},
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};
use tokio::{
    net::TcpListener,
    task::{JoinError, JoinSet},
};

/// Pause after the first descriptor-exhaustion failure; it doubles each time.
const FIRST_PAUSE: Duration = Duration::from_millis(50);
/// Longest pause after repeated descriptor-exhaustion failures.
const LONGEST_PAUSE: Duration = Duration::from_secs(1);

/// What one accept on the listening socket produced.
#[derive(Debug)]
pub enum Accepted {
    /// A connection ready for a worker.
    Connection(TcpStream),
    /// `accept` itself failed. Whether that ends the run depends on the error.
    AcceptFailed(IoError),
    /// A connection was accepted but could not be prepared for a worker. Only
    /// that connection is lost.
    ConnectionUnusable(IoError),
}

/// The listening socket. `accept` must be cancel-safe: the listener drops the
/// future when stop wins, and no connection may be lost by that.
pub trait EnrollmentAccept: Send {
    /// Wait for the next connection or failure.
    fn accept(&mut self) -> impl Future<Output = Accepted> + Send;
}

/// The real listening socket, bound by composition.
pub struct TcpEnrollmentAccept(TcpListener);
impl TcpEnrollmentAccept {
    /// Wrap a socket composition has already bound.
    pub fn new(socket: TcpListener) -> Self {
        Self(socket)
    }
    /// The bound address. It carries no product authority.
    pub fn local_address(&self) -> IoResult<SocketAddr> {
        self.0.local_addr()
    }
}
impl EnrollmentAccept for TcpEnrollmentAccept {
    /// Tokio's `accept` is cancel-safe; the conversion runs only after it.
    async fn accept(&mut self) -> Accepted {
        match self.0.accept().await {
            Ok((stream, _)) => match stream.into_std() {
                Ok(stream) => Accepted::Connection(stream),
                Err(error) => Accepted::ConnectionUnusable(error),
            },
            Err(error) => Accepted::AcceptFailed(error),
        }
    }
}

/// What the listener does after a failed `accept`.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum AcceptStep {
    /// Only that connection failed; accept the next one.
    Skip,
    /// The process or system is out of file descriptors; wait, then accept.
    Pause,
    /// The listening socket itself failed.
    Stop,
}
fn accept_step(error: &IoError) -> AcceptStep {
    if out_of_descriptors(error) {
        return AcceptStep::Pause;
    }
    match error.kind() {
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted => {
            AcceptStep::Skip
        }
        _ => AcceptStep::Stop,
    }
}
/// EMFILE and ENFILE have no `ErrorKind` of their own, so the OS code decides.
fn out_of_descriptors(error: &IoError) -> bool {
    #[cfg(unix)]
    {
        matches!(error.raw_os_error(), Some(libc::EMFILE | libc::ENFILE))
    }
    #[cfg(windows)]
    {
        /// WSAEMFILE: too many open sockets.
        const WSAEMFILE: i32 = 10024;
        error.raw_os_error() == Some(WSAEMFILE)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = error;
        false
    }
}

/// Bound native enrollment listener. Composition restores the gateway identity
/// before binding. Dropping the listener, or the `run` future, stops admission;
/// workers already admitted keep their permit until they end.
pub struct NativeEnrollmentListener<A: EnrollmentAccept = TcpEnrollmentAccept> {
    /// Taken only by `run`, which consumes the listener.
    socket: Option<A>,
    connections: Arc<NativeEnrollmentConnections>,
}
impl<A: EnrollmentAccept> NativeEnrollmentListener<A> {
    /// Compose one listening socket and its connection owner.
    pub fn new(socket: A, connections: Arc<NativeEnrollmentConnections>) -> Self {
        Self {
            socket: Some(socket),
            connections,
        }
    }
    /// Accept until `stop` completes or the listening socket fails, then stop
    /// admission, collect every admitted worker's result and wait for drain.
    ///
    /// A failure of one connection is logged at debug and skipped. Descriptor
    /// exhaustion is logged at warn and pauses for 50 ms, doubling to at most
    /// 1 s, until an accept succeeds; stop wins during a pause
    /// (`native_listener_skips_connection_failures_and_backs_off_on_exhaustion`).
    /// Any other accept failure ends the run: `failed` receives its kind before
    /// the drain starts and the error is returned
    /// (`native_listener_stops_on_a_listening_socket_failure`). A failed peer
    /// does not end service for others. Result handles hold the connection
    /// permit, so the task set is bounded by connection capacity.
    pub async fn run<R, E, S, F>(mut self, entropy: E, stop: S, failed: F) -> IoResult<()>
    where
        R: RngCore + CryptoRng + Send + 'static,
        E: Fn() -> R,
        S: Future<Output = ()>,
        F: FnOnce(ErrorKind),
    {
        let mut socket = self
            .socket
            .take()
            .expect("socket is present until run consumes the listener");
        let mut tasks = JoinSet::new();
        let mut pause: Option<Duration> = None;
        tokio::pin!(stop);
        let outcome = loop {
            let accepted = tokio::select! {
                biased;
                () = &mut stop => break Ok(()),
                Some(result) = tasks.join_next(), if !tasks.is_empty() => {
                    observe(result);
                    continue;
                }
                accepted = socket.accept() => accepted,
            };
            let error = match accepted {
                Accepted::Connection(stream) => {
                    pause = None;
                    match self.connections.start(stream, entropy()) {
                        Ok(task) => {
                            tasks.spawn(task.complete());
                        }
                        Err(error) if error.failure == NativeConnectionFailure::Capacity => {}
                        Err(error) => {
                            tracing::debug!(?error, "Native enrollment dispatch refused")
                        }
                    }
                    continue;
                }
                Accepted::ConnectionUnusable(error) => {
                    tracing::debug!(kind = ?error.kind(), "Native enrollment connection unusable");
                    continue;
                }
                Accepted::AcceptFailed(error) => error,
            };
            match accept_step(&error) {
                AcceptStep::Skip => {
                    tracing::debug!(kind = ?error.kind(), "Native enrollment accept skipped");
                }
                AcceptStep::Pause => {
                    let wait = pause.map_or(FIRST_PAUSE, |last| (last * 2).min(LONGEST_PAUSE));
                    pause = Some(wait);
                    tracing::warn!(?wait, "Native enrollment accept is out of file descriptors");
                    tokio::select! {
                        biased;
                        () = &mut stop => break Ok(()),
                        () = tokio::time::sleep(wait) => {}
                    }
                }
                AcceptStep::Stop => break Err(error),
            }
        };
        drop(socket);
        self.connections.close();
        finish(outcome, failed, &mut tasks, &self.connections).await
    }
}
// One converged serving outcome reports before the first physical-drain await.
// Notification is not completion: the original error and peers remain owned.
async fn finish(
    outcome: IoResult<()>,
    failed: impl FnOnce(ErrorKind),
    tasks: &mut JoinSet<NativeConnectionCompletion>,
    connections: &NativeEnrollmentConnections,
) -> IoResult<()> {
    if let Err(error) = &outcome {
        failed(error.kind());
    }
    while let Some(result) = tasks.join_next().await {
        observe(result);
    }
    connections.shutdown().await;
    outcome
}
impl<A: EnrollmentAccept> Drop for NativeEnrollmentListener<A> {
    fn drop(&mut self) {
        self.connections.close();
    }
}
fn observe(result: Result<NativeConnectionCompletion, JoinError>) {
    match result.map(|completion| completion.into_result()) {
        Ok(Ok(())) => {}
        Ok(Err(error)) => tracing::debug!(?error, "Native enrollment peer refused"),
        Err(error) => {
            tracing::debug!(fault = ?worker_fault(error), "Native enrollment observer fault")
        }
    }
}
