//! One bound enrollment listener feeding the connection owner.
//!
//! The listening socket sits behind [`EnrollmentAccept`], so tests can produce
//! accept failures. What a failure means is decided by `accept_step`, design
//! row P67: only the closed set of errors that mean the listening socket is
//! invalid ends the run; a failure of one connection is skipped; anything else,
//! including an error the listener does not recognise, pauses with a bounded
//! backoff and then accepts again.
use super::{
    connection::NativeConnectionCompletion, NativeConnectionFailure, NativeEnrollmentConnections,
};
use nessa_auth::adapters::pairing::{CryptoRng, RngCore};
use nessa_protocol::pairing::socket::worker_fault;
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

/// Pause after the first failure that is not skipped; it doubles each time.
const FIRST_PAUSE: Duration = Duration::from_millis(50);
/// Longest pause after repeated failures.
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
    /// Only that connection failed; accept the next one at once.
    Skip,
    /// Resource exhaustion or an unrecognised error; wait, then accept.
    Pause,
    /// The listening socket itself is invalid.
    Stop,
}
/// Stop is the closed set; skip names the per-connection errors; everything
/// else, recognised or not, pauses. An error with no OS code is unrecognised:
/// only the OS can say the listening socket is invalid.
fn accept_step(error: &IoError) -> AcceptStep {
    let code = error.raw_os_error();
    if code.is_some_and(|code| LISTENER_INVALID.contains(&code)) {
        return AcceptStep::Stop;
    }
    if matches!(
        error.kind(),
        ErrorKind::ConnectionAborted | ErrorKind::ConnectionReset | ErrorKind::Interrupted
    ) || code.is_some_and(|code| CONNECTION_FAILED.contains(&code))
    {
        return AcceptStep::Skip;
    }
    AcceptStep::Pause
}
/// OS codes meaning the listening socket itself is invalid.
#[cfg(unix)]
const LISTENER_INVALID: [i32; 4] = [libc::EBADF, libc::EINVAL, libc::ENOTSOCK, libc::EFAULT];
/// WSAEBADF, WSAEINVAL, WSAENOTSOCK, WSAEFAULT.
#[cfg(windows)]
const LISTENER_INVALID: [i32; 4] = [10009, 10022, 10038, 10014];
#[cfg(not(any(unix, windows)))]
const LISTENER_INVALID: [i32; 0] = [];
/// Network errors of the new socket that accept(2) passes back, which its
/// manual says to retry like EAGAIN. Windows reports per-connection failures
/// through the error kinds above; anything else there pauses.
#[cfg(target_os = "linux")]
const CONNECTION_FAILED: [i32; 10] = [
    libc::ENETDOWN,
    libc::EPROTO,
    libc::ENOPROTOOPT,
    libc::EHOSTDOWN,
    libc::ENONET,
    libc::EHOSTUNREACH,
    libc::EOPNOTSUPP,
    libc::ENETUNREACH,
    libc::EPERM,
    libc::ETIMEDOUT,
];
#[cfg(all(unix, not(target_os = "linux")))]
const CONNECTION_FAILED: [i32; 9] = [
    libc::ENETDOWN,
    libc::EPROTO,
    libc::ENOPROTOOPT,
    libc::EHOSTDOWN,
    libc::EHOSTUNREACH,
    libc::EOPNOTSUPP,
    libc::ENETUNREACH,
    libc::EPERM,
    libc::ETIMEDOUT,
];
#[cfg(not(unix))]
const CONNECTION_FAILED: [i32; 0] = [];

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
    /// A failure of one connection is logged at debug and skipped
    /// (`native_listener_skips_connection_failures_and_backs_off_on_exhaustion`).
    /// Resource exhaustion and unrecognised errors are logged at warn and pause
    /// for 50 ms, doubling to at most 1 s; the pause resets when an accept
    /// yields a connection, and a stop during a pause ends the run at once
    /// (`native_listener_stop_ends_a_pause`). Only an error meaning the listening
    /// socket is invalid ends the run: `failed` receives its kind while admitted
    /// workers are still running, before the drain, and the error is returned
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
                    tracing::warn!(?wait, kind = ?error.kind(), "Native enrollment accept failed; pausing");
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
