//! One bound enrollment listener consumes the existing physical connection owner.
use super::worker::worker_fault;
use super::{
    connection::NativeConnectionCompletion, NativeConnectionFailure, NativeEnrollmentConnections,
};
use nessa_auth::adapters::pairing::{CryptoRng, RngCore};
use std::{
    future::Future,
    io::{ErrorKind, Result as IoResult},
    net::SocketAddr,
    sync::Arc,
};
use tokio::{
    net::TcpListener,
    task::{JoinError, JoinSet},
};

/// Accepted native listener; identity restoration precedes its injected binding.
/// Dropping the listener excludes new admission; accepted physical closures still
/// retain their original runtime and permit until actual completion/unwind.
pub struct NativeEnrollmentListener {
    socket: Option<TcpListener>,
    connections: Arc<NativeEnrollmentConnections>,
}
impl NativeEnrollmentListener {
    /// Compose one already bound socket and its canonical connection owner.
    /// The composition root restores/reconciles the private identity before bind.
    pub fn new(socket: TcpListener, connections: Arc<NativeEnrollmentConnections>) -> Self {
        Self {
            socket: Some(socket),
            connections,
        }
    }
    /// Actual bound enrollment endpoint; exposes no protected product authority.
    pub fn local_address(&self) -> IoResult<SocketAddr> {
        self.socket
            .as_ref()
            .expect("owned listener socket")
            .local_addr()
    }
    /// Accept until `stop` completes or an accept fails (design row P67), then
    /// stop admission, collect every admitted worker's result and wait for drain.
    /// An accept failure is passed to `failed` before the drain starts. A failed
    /// peer is logged and does not end service for others
    /// (`native_create_claim_approve_and_reopen_status` serves an abandoned
    /// connection, then a full enrollment, on one listener). Result handles hold
    /// the connection permit, so the task set is bounded by connection capacity.
    pub async fn run<R, E, S, F>(mut self, entropy: E, stop: S, failed: F) -> IoResult<()>
    where
        R: RngCore + CryptoRng + Send + 'static,
        E: Fn() -> R,
        S: Future<Output = ()>,
        F: FnOnce(ErrorKind),
    {
        let mut tasks = JoinSet::new();
        tokio::pin!(stop);
        let outcome = loop {
            tokio::select! {
                biased;
                () = &mut stop => break Ok(()),
                Some(result) = tasks.join_next(), if !tasks.is_empty() => observe(result),
                accepted = self.socket.as_ref().expect("owned listener socket").accept() => {
                    let (stream, _) = match accepted {
                        Ok(accepted) => accepted,
                        Err(error) => break Err(error),
                    };
                    let stream = match stream.into_std() {
                        Ok(stream) => stream,
                        Err(error) => break Err(error),
                    };
                    match self.connections.start(stream, entropy()) {
                        Ok(task) => { tasks.spawn(task.complete()); }
                        Err(error) if error.failure == NativeConnectionFailure::Capacity => {},
                        Err(error) => tracing::debug!(?error, "Native enrollment dispatch refused"),
                    }
                }
            }
        };
        drop(self.socket.take());
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
impl Drop for NativeEnrollmentListener {
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
