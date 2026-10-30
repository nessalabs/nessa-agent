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
//!               every keepalive interval: ServingCredentials::serves(proof)?
//!                 no ──▶ HostLink::revoke
//! admitted, ended, or HostLink::revoke ──▶ connections moved
//!   ──▶ every PeerEnvironments::connections receiver wakes once
//! open / grant / account ──▶ that environment's current connection
//!   ──▶ ServingCredentials::serves(the proof it was opened with)?
//!         no ──▶ HostLink::revoke, the lease refused unreachable
//! HostLink::revoke ──▶ no lease admitted ──▶ Revoked queued to the host first
//!   ──▶ then each lease ending as revoked (so its End goes after Revoked)
//!   ──▶ each End answered ──▶ the connection closed
//! ```
//!
//! Arrows are calls, in order. A paired environment is one key: its name is
//! `peer:` and the key in hex ([`PeerEnvironment`]), whichever connection it
//! last made. Whether that key may serve is the registry's and the policy's
//! answer, never a copy kept here: it is asked when the connection is
//! admitted, again before every lease on it, and again every keepalive
//! interval while it is open, so a credential revoked, expired or whose
//! membership was disabled ends an idle connection too, and the leases
//! running on it as revoked, on both sides. Only the connection differs
//! from an SSH host's; the leases on it, their frames and their audit are
//! the same [`SshEnvironment`]'s. A file the host publishes is read over
//! the lease frames themselves ([`ArtifactTransport::Frames`]), since a
//! paired connection carries nothing beside them.
//! Nothing is fetched into a paired host: with no file channel, the agent's
//! fetch is refused unavailable before the host is asked.
//!
//! Which environments are connected is read from their connections, never
//! kept here as a list; what is kept is a count of the changes to it
//! ([`PeerEnvironments::connections`]), moved where each change is made, so
//! the agent catalogue's subscribers (`agents.subscribe`) read the list
//! again when it may differ. It is its own change source: the conversation
//! catalogue's head does not move for it.
use super::{
    audit::EnvironmentAudit,
    connector::{ArtifactTransport, LeaseConnection},
    environment::{SshEnvironment, SshTimings},
};
use crate::conversation::application::{
    Environment, EnvironmentFuture, OrganizationBudgets, PairedEnvironments, WorkerLoad,
};
use nessa_auth::application::{pairing::DeviceConnectionProof, ports::AccessError};
use nessa_protocol::lease::MachineReport;
use nessa_sdk::domain::agent_execution::leases::PeerEnvironment;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex, PoisonError},
};
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::watch,
};

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

/// Told when a paired environment's connection ends, with the last report
/// its machine sent on it. Implemented in composition.
pub(crate) trait Departures: Send + Sync {
    fn left(&self, peer: PeerEnvironment, machine: MachineReport);
}

/// One paired environment as [`PeerEnvironments::workers`] reads it.
pub(crate) struct PeerWorker {
    pub(crate) peer: PeerEnvironment,
    pub(crate) load: Option<WorkerLoad>,
    pub(crate) machine: Option<MachineReport>,
}

/// This gateway's paired environments, one per key, made when it first
/// dials in.
pub(crate) struct PeerEnvironments {
    credentials: Arc<dyn ServingCredentials>,
    audit: Arc<dyn EnvironmentAudit>,
    timings: SshTimings,
    /// The organization budget all of them share.
    budgets: OrganizationBudgets,
    peers: Mutex<BTreeMap<PeerEnvironment, Arc<SshEnvironment>>>,
    /// How many times which of them are connected has changed. Each
    /// environment moves it as its connection is admitted, ends, or is
    /// revoked.
    connections: watch::Sender<u64>,
    departures: Option<Arc<dyn Departures>>,
}

impl PeerEnvironments {
    pub(crate) fn new(
        credentials: Arc<dyn ServingCredentials>,
        audit: Arc<dyn EnvironmentAudit>,
        timings: SshTimings,
        budgets: OrganizationBudgets,
    ) -> Self {
        Self {
            credentials,
            audit,
            timings,
            budgets,
            peers: Mutex::new(BTreeMap::new()),
            connections: watch::Sender::new(0),
            departures: None,
        }
    }

    /// Tell `departures` each time a connection that reported its machine
    /// ends.
    pub(crate) fn with_departures(mut self, departures: Arc<dyn Departures>) -> Self {
        self.departures = Some(departures);
        self
    }

    /// Wakes each time which paired environments are connected may have
    /// changed: one connected, one's connection ended, or one was revoked.
    /// A receiver that has fallen behind wakes once, not once per change,
    /// and reads [`PairedEnvironments::connected`] for the list as it is
    /// then. The count is a wake-up and nothing more: it is not a position
    /// anything resumes from.
    pub(crate) fn connections(&self) -> watch::Receiver<u64> {
        self.connections.subscribe()
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
        let peer = PeerEnvironment::new(*proof.key().bytes());
        let environment = self.peer(peer);
        let connection = LeaseConnection {
            mcp_relay: None,
            from_environment,
            to_environment,
            keep,
            artifacts: ArtifactTransport::Frames,
        };
        environment.serve(proof, connection).await;
        if let (Some(departures), Some(machine)) = (&self.departures, environment.machine()) {
            departures.left(peer, machine);
        }
    }

    /// Every paired environment that has dialed in since this gateway
    /// started, as it stands now: what its connection carries (`None` with
    /// none that serves leases) and the latest report its machine sent.
    pub(crate) fn workers(&self) -> Vec<PeerWorker> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .map(|(peer, environment)| PeerWorker {
                peer: *peer,
                load: environment.load(),
                machine: environment.machine(),
            })
            .collect()
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
                    self.connections.clone(),
                    self.budgets.clone(),
                ))
            })
            .clone()
    }
}

impl PairedEnvironments for PeerEnvironments {
    /// The one that has dialed in under `peer`'s key; for a key that never
    /// has, one with no connection, kept nowhere, so a name an agent makes
    /// up adds nothing here.
    fn environment(&self, peer: &PeerEnvironment) -> Arc<dyn Environment> {
        let known = self
            .peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(peer)
            .cloned();
        match known {
            Some(environment) => environment,
            None => Arc::new(SshEnvironment::peer(
                *peer,
                self.credentials.clone(),
                self.audit.clone(),
                self.timings,
                self.connections.clone(),
                self.budgets.clone(),
            )),
        }
    }

    fn serving(&self) -> Vec<(PeerEnvironment, WorkerLoad)> {
        self.peers
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .iter()
            .filter_map(|(peer, environment)| environment.load().map(|load| (*peer, load)))
            .collect()
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
