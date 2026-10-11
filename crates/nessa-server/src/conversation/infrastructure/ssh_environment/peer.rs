//! Paired environments (issue #706): machines whose `nessa env serve` dials
//! this gateway's native listener, proves the key it paired with, and serves
//! leases over that connection, so the gateway reaches a machine it could
//! not dial.
//!
//! ```text
//! native listener: openEnvironment ──▶ ServingCredentials::serves(proof)?
//!   ──▶ PeerEnvironments::serve(proof, stream)
//!         ──▶ the key's environment ──▶ HostLink::over(peer:<key>, stream)
//!               hello, this build? ──▶ replaces the connection it had
//!               (that one closed: its leases lost) ──▶ held until it ends
//! open / grant / account ──▶ that environment's current connection
//!   ──▶ ServingCredentials::serves(the proof it was opened with)?
//!         no ──▶ the connection closed, the lease refused unreachable
//! ```
//!
//! Arrows are calls, in order. A paired environment is one key: its name is
//! `peer:` and the key in hex ([`PeerEnvironment`]), whichever connection it
//! last made. Whether that key may serve is the registry's and the policy's
//! answer, never a copy kept here: it is asked when the connection is
//! admitted and again before every lease on it. Only the connection differs
//! from an SSH host's; the leases on it, their frames and their audit are
//! the same [`SshEnvironment`]'s. A file the host publishes is not fetched
//! over a paired connection yet: it has no artifact channel, so its
//! collection is refused.
use super::{
    audit::EnvironmentAudit,
    connector::{ArtifactChannel, ArtifactChannels, LeaseConnection},
    environment::{SshEnvironment, SshTimings},
};
use crate::conversation::application::{Environment, EnvironmentFuture, PairedEnvironments};
use nessa_auth::application::{pairing::DeviceConnectionProof, ports::AccessError};
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use std::{
    collections::BTreeMap,
    io,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::io::{AsyncRead, AsyncWrite};

/// Whether a key holds a credential to serve this gateway's leases now.
/// Implemented over the registry and the policy in composition.
pub(crate) trait ServingCredentials: Send + Sync {
    /// Whether the key `proof` carries holds an active credential whose
    /// `environment.serve` this gateway's policy allows now.
    fn serves<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
    ) -> EnvironmentFuture<'a, Result<bool, AccessError>>;
}

/// This gateway's paired environments, one per key, made when first named.
pub(crate) struct PeerEnvironments {
    credentials: Arc<dyn ServingCredentials>,
    audit: Arc<dyn EnvironmentAudit>,
    timings: SshTimings,
    peers: Mutex<BTreeMap<PeerEnvironment, Arc<SshEnvironment>>>,
}

impl PeerEnvironments {
    pub(crate) fn new(
        credentials: Arc<dyn ServingCredentials>,
        audit: Arc<dyn EnvironmentAudit>,
        timings: SshTimings,
    ) -> Self {
        Self {
            credentials,
            audit,
            timings,
            peers: Mutex::new(BTreeMap::new()),
        }
    }

    /// Whether the key `proof` carries may serve now: asked before its
    /// connection is admitted.
    pub(crate) async fn admits(&self, proof: &DeviceConnectionProof) -> Result<bool, AccessError> {
        self.credentials.serves(proof).await
    }

    /// Serve the leases of the paired environment `proof`'s key names over
    /// one connection it dialed, until that connection ends. It replaces the
    /// connection that environment had.
    pub(crate) async fn serve(
        &self,
        proof: DeviceConnectionProof,
        from_environment: Box<dyn AsyncRead + Send + Unpin>,
        to_environment: Box<dyn AsyncWrite + Send + Unpin>,
        keep: Box<dyn Send>,
    ) {
        let environment = self.peer(PeerEnvironment::new(*proof.key().bytes()));
        let connection = LeaseConnection {
            from_environment,
            to_environment,
            keep,
            artifacts: Arc::new(NoArtifactChannels),
        };
        environment.serve(proof, connection).await;
    }

    fn peer(&self, peer: PeerEnvironment) -> Arc<SshEnvironment> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .entry(peer)
            .or_insert_with(|| {
                Arc::new(SshEnvironment::peer(
                    peer,
                    self.credentials.clone(),
                    self.audit.clone(),
                    self.timings,
                ))
            })
            .clone()
    }
}

impl PairedEnvironments for PeerEnvironments {
    fn environment(&self, peer: &PeerEnvironment) -> Arc<dyn Environment> {
        self.peer(*peer)
    }

    fn connected(&self) -> Vec<PeerEnvironment> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter(|(_, environment)| environment.is_connected())
            .map(|(peer, _)| *peer)
            .collect()
    }
}

/// A paired connection carries no artifact channel yet.
struct NoArtifactChannels;

impl ArtifactChannels for NoArtifactChannels {
    fn open(&self) -> io::Result<ArtifactChannel> {
        Err(io::Error::new(
            io::ErrorKind::Unsupported,
            "a paired environment's files are not fetched yet",
        ))
    }
}
