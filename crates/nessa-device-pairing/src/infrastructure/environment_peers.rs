//! The environment phase of a native connection: after a first
//! `openEnvironment` envelope, a paired environment's `nessa env serve`
//! serves this gateway's leases over the same TLS connection, as the byte
//! stream [`TlsStream`] carries.
//!
//! ```text
//! openEnvironment ──▶ EnvironmentPeers::admits(proof)? ──▶ an environment permit
//!   ──▶ serving ──▶ EnvironmentPeers::serve(the stream) until it ends
//! ```
//! Arrows are steps, in order. The listener decides only who may take a
//! permit; what the environment may serve, and for how long, is the composed
//! side's, which asks again before every lease.
use super::tls_stream::TlsStream;
use nessa_auth::application::{pairing::DeviceConnectionProof, ports::AccessError};
use std::{future::Future, pin::Pin};

/// The composed side of paired environments that dial in.
pub trait EnvironmentPeers: Send + Sync {
    /// Whether a connection whose TLS handshake proved `proof` may take an
    /// environment permit: its key holds a credential to serve this gateway
    /// now. Asked before the permit is taken, so keys that may not serve
    /// cannot hold the pool.
    fn admits<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
    ) -> Pin<Box<dyn Future<Output = Result<bool, AccessError>> + Send + 'a>>;
    /// Serve one environment's connection until it ends. The connection has
    /// moved to the environment pool and returned its connection permit; its
    /// environment permit is held by the caller for the whole of the
    /// returned future.
    fn serve(&self, stream: TlsStream) -> Pin<Box<dyn Future<Output = ()> + Send>>;
}
