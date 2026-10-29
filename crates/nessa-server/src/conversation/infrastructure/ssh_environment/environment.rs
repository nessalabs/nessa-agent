//! The SSH environment: a conversation's agent runs on a host this gateway
//! reaches with `ssh <host>`, running the copy of this build installed there
//! (`env serve`), under a lease the host admits.
//!
//! ```text
//! open(lease, grant, binding)
//!   ──▶ link (one per host: connect, hello, this build?)
//!         the stream ended with nothing said ──▶ installer.ensure(host, lease)
//!           installed, or present already ──▶ connect once more
//!         (an account of a lost lease connects only: it never installs)
//!   ──▶ binding.on_host(LeaseHost) ──▶ the same binding, starting its harness there
//!   ──▶ link.grant(lease, agent, sandbox) ──▶ EnvironmentLease { provider, SshHold,
//!                                              the terms the host granted }
//! binding opens a session ──▶ LeaseHost::start ──▶ Start{lease, channel}
//!   harness stdin/stdout ⇄ Input / Output frames
//!   close ──▶ SshControl::cleanup ──▶ (after its input) Stop ──▶ Stopped{cleanup}
//! LiveLease::lost ──▶ the lease's watch, taken at its grant
//! LiveLease::close ──▶ SshHold::end ──▶ End ──▶ Ended{cleanup}
//!   connection lost? ──▶ a new connection ──▶ Account ──▶ what the host recorded
//! account(lease) ──▶ Account ──▶ Accounted{cleanup}
//! Published{lease, artifact, request, file} ──▶ LeaseHold::artifacts ──▶ ArtifactOffer
//!   its bytes ──▶ SftpBytes, over an artifact channel, only when pulled; on a
//!   paired connection, FrameBytes, over the lease frames themselves
//!   its answer ──▶ Collected{lease, artifact, outcome}
//! LeaseHold::publish(ask) ──▶ Publish{lease, request, path} ──▶ offered on the
//!   same offers as above, naming `request` | NotPublished{reason}
//! LeaseHold::fetch(ask) ──▶ Fetch{lease, request, destination, file}
//!   ──▶ FetchAccepted{part} ──▶ its bytes written to `part` over sftp, read
//!   from the held file range by range ──▶ Delivered ──▶ Fetched{outcome}
//!   | FetchRefused{reason}; given up before Delivered ──▶ Undelivered
//! LeaseHold::publish / fetch on a connection that carries no files (a
//!   paired one) ──▶ None, before any frame
//! a command: grant ──▶ link.grant_command ──▶ SshCommand, the terms the host granted,
//!   and the files it publishes
//!   SshCommand::publications ──▶ ArtifactOffer, while it runs
//!   SshCommand::run ──▶ Run ──▶ Ran{end, output, cleanup}
//!   stop said, or the hold dropped ──▶ End (not waited for) ──▶ Ran{Stopped} ──▶ Ended
//!   no Ran within its timeout and COMMAND_STOP_WAIT ──▶ unanswered
//! ```
//!
//! Arrows are calls and frames, in order. The binding — the agent protocol,
//! its permission requests, its events — stays on the gateway exactly as for
//! a local child; only the harness process runs on the host, and the host
//! owns its executable, its credentials and its process tree. So events,
//! approvals and Stop behave as they do here, and what reaches the gateway is
//! fenced by the lease as any provider's events are.
use super::{
    audit::EnvironmentAudit,
    connector::{ArtifactTransport, LeaseConnection, LeaseConnector},
    install::HostInstaller,
    link::{
        AdmissionRefused, CommandAnswer, Granted, HostLink, Liveness, Opening, Published,
        StartError,
    },
    peer::ServingCredentials,
    transfer::{FrameBytes, SftpBytes},
};
use crate::conversation::application::{
    ArtifactAnswer, ArtifactBytes, ArtifactOffer, CommandEnvironment, CommandHold, CommandResult,
    Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease, FetchAsk,
    FetchFailure, FetchPlaced, GrantedCommand, GrantedTerms, LeaseHold, LeaseRelease, PublishAsk,
    PublishFailure, WorkerLoad,
};
use crate::conversation::application::{BudgetPlace, OrganizationBudgets};
use nessa_auth::application::pairing::DeviceConnectionProof;
use nessa_protocol::lease::{
    self, Cleanup, CommandEnd, GrantRefusal, COMMAND_STOP_WAIT, KEEPALIVE_INTERVAL, SILENCE_LIMIT,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{
        AgentProvider, CloseOutcome, HarnessCleanupFuture, HarnessControl, HarnessHost,
        HarnessLaunch, HarnessProcess,
    },
};
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandRefusal, CommandWork, EnvironmentRef, LeaseCleanup, LeaseEndCause, LeaseId,
    LeaseRefusal, LeaseTerms, LeaseWork, PeerEnvironment, SandboxProfiles, SshDestination,
};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::{mpsc, watch, Mutex};

/// How long reaching a host and its answers may take.
#[derive(Clone, Copy, Debug)]
pub(crate) struct SshTimings {
    /// Connecting and the hello: past it, the host is unreachable. Longer
    /// than a host's own wait for the previous serving process to finish.
    pub(crate) connect: Duration,
    /// A grant's or an account's answer.
    pub(crate) answer: Duration,
    /// Most the connection is left without a frame to the host: past it a
    /// keepalive is sent, which is how the host tells this gateway is there.
    pub(crate) keepalive: Duration,
    /// Most the connection is believed without a frame from the host, whose
    /// own keepalives say it is there: past it, the connection is lost.
    pub(crate) silence: Duration,
}

impl Default for SshTimings {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            answer: Duration::from_secs(15),
            keepalive: KEEPALIVE_INTERVAL,
            silence: SILENCE_LIMIT,
        }
    }
}

/// One host, as an environment: reached over SSH, or a paired environment
/// that dials in ([`super::peer`]). Only how its connection is had differs.
pub(crate) struct SshEnvironment {
    inner: Arc<Inner>,
}

struct Inner {
    reach: Reach,
    audit: Arc<dyn EnvironmentAudit>,
    timings: SshTimings,
    /// The one SSH connection to the host, opened when first needed and
    /// opened again once lost, held while connecting, so one is opened at a
    /// time. A paired environment's is [`Reach::Peer`]'s `serving`.
    link: Mutex<Option<Arc<HostLink>>>,
}

/// How a host's connection is had.
enum Reach {
    Workspace {
        connector: Arc<dyn super::connector::WorkspaceConnector>,
    },
    /// This gateway runs `ssh` to it.
    Ssh {
        host: SshDestination,
        connector: Arc<dyn LeaseConnector>,
        /// Puts this build on the host when none is there.
        installer: Arc<HostInstaller>,
    },
    /// It dials this gateway. Its connection is used only while the key
    /// that connection proved still holds a credential to serve, asked
    /// again before each lease.
    Peer {
        peer: PeerEnvironment,
        credentials: Arc<dyn ServingCredentials>,
        /// Its current connection and what that connection proved, kept
        /// together. Never held across an await, so whether it is connected
        /// is read without waiting.
        serving: std::sync::Mutex<Option<Served>>,
        /// [`super::PeerEnvironments`]' count of changes to which of them
        /// are connected, moved here as this one's connection is admitted,
        /// ends, or is revoked.
        connections: watch::Sender<u64>,
        /// The organization budget every paired environment of this gateway
        /// shares: a place is taken before a lease or a command is asked.
        budgets: OrganizationBudgets,
    },
}

/// A paired environment's current connection, with the proof it was opened
/// with.
#[derive(Clone)]
struct Served {
    link: Arc<HostLink>,
    proof: Arc<DeviceConnectionProof>,
}

/// Say that which paired environments are connected has changed. Whoever
/// waits on `connections` wakes once for however many changes came since
/// it last looked, and reads the list again.
fn moved(connections: &watch::Sender<u64>) {
    connections.send_modify(|count| *count = count.wrapping_add(1));
}

/// The paired environment's current connection, if it holds one.
fn served(serving: &std::sync::Mutex<Option<Served>>) -> Option<Served> {
    serving
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
        .clone()
}

impl SshEnvironment {
    /// No opening call, command or lease retains this environment's inner owner.
    /// The retaining workspace cache also checks that nobody borrows its wrapper.
    pub(crate) fn unowned(&self) -> bool {
        Arc::strong_count(&self.inner) == 1
    }

    pub(crate) fn new(
        host: SshDestination,
        connector: Arc<dyn LeaseConnector>,
        audit: Arc<dyn EnvironmentAudit>,
        installer: Arc<HostInstaller>,
        timings: SshTimings,
    ) -> Self {
        Self::reached(
            Reach::Ssh {
                host,
                connector,
                installer,
            },
            audit,
            timings,
        )
    }

    pub(crate) fn workspace(
        connector: Arc<dyn super::connector::WorkspaceConnector>,
        audit: Arc<dyn EnvironmentAudit>,
    ) -> Self {
        Self::reached(
            Reach::Workspace { connector },
            audit,
            SshTimings {
                connect: Duration::from_secs(120),
                ..SshTimings::default()
            },
        )
    }

    /// The paired environment `peer`, with no connection until it dials in,
    /// moving `connections` each time it is connected or no longer.
    pub(super) fn peer(
        peer: PeerEnvironment,
        credentials: Arc<dyn ServingCredentials>,
        audit: Arc<dyn EnvironmentAudit>,
        timings: SshTimings,
        connections: watch::Sender<u64>,
        budgets: OrganizationBudgets,
    ) -> Self {
        Self::reached(
            Reach::Peer {
                peer,
                credentials,
                serving: std::sync::Mutex::new(None),
                connections,
                budgets,
            },
            audit,
            timings,
        )
    }

    fn reached(reach: Reach, audit: Arc<dyn EnvironmentAudit>, timings: SshTimings) -> Self {
        Self {
            inner: Arc::new(Inner {
                reach,
                audit,
                timings,
                link: Mutex::new(None),
            }),
        }
    }

    /// Serve this paired environment's leases over `connection`, which
    /// proved `proof`, in place of any connection it had: that one is
    /// closed, and its leases are lost. Answers once the new connection
    /// ends, or at once when its hello is refused. While it lasts, whether
    /// the key still serves is asked again every keepalive interval, and a
    /// key that no longer does has its connection revoked. The connected
    /// list changes when the connection is admitted and again when it ends,
    /// and each is said on `connections` once the change is made, so whoever
    /// it wakes reads the list as it is after it.
    pub(super) async fn serve(&self, proof: DeviceConnectionProof, connection: LeaseConnection) {
        let Reach::Peer {
            serving,
            credentials,
            connections,
            ..
        } = &self.inner.reach
        else {
            return;
        };
        let opened = HostLink::over(
            &self.inner.name(),
            connection,
            self.inner.audit.clone(),
            self.inner.timings.connect,
            self.inner.liveness(),
        )
        .await;
        let Ok(link) = opened else {
            return;
        };
        let proof = Arc::new(proof);
        let replaced = serving
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .replace(Served {
                link: link.clone(),
                proof: proof.clone(),
            });
        if let Some(replaced) = replaced {
            replaced.link.close();
        }
        moved(connections);
        let ended = link.ended();
        let revoked = self.inner.until_revoked(credentials.as_ref(), &proof);
        let drains = link.drains();
        tokio::pin!(ended, revoked, drains);
        let mut draining = false;
        loop {
            tokio::select! {
                () = &mut ended => break,
                () = &mut revoked => {
                    self.inner.revoke(&link);
                    link.ended().await;
                    break;
                }
                // Draining, it is no longer offered new work: whoever lists
                // where work can go reads that list again.
                () = &mut drains, if !draining => {
                    draining = true;
                    moved(connections);
                }
            }
        }
        moved(connections);
    }

    /// Revoke the connection it holds now, as a paired environment's is
    /// revoked: for tests of what a revocation does to leases on it.
    #[cfg(test)]
    pub(super) async fn revoke_connection(&self) {
        let held = self.inner.link.lock().await.clone();
        if let Some(link) = held {
            self.inner.revoke(&link);
        }
    }

    /// Whether a connection to it serves leases now.
    pub(super) fn is_connected(&self) -> bool {
        self.reachable() == Some(true)
    }

    /// What a paired environment's connection carries now, read off that
    /// connection: `None` with none that serves leases.
    pub(super) fn load(&self) -> Option<WorkerLoad> {
        let Reach::Peer { serving, .. } = &self.inner.reach else {
            return None;
        };
        let link = served(serving)
            .map(|served| served.link)
            .filter(|link| link.is_serving())?;
        Some(WorkerLoad {
            leases: link.agent_leases(),
            draining: link.is_draining(),
        })
    }
}

/// Which organization budget a piece of work takes a place in.
#[derive(Clone, Copy, Debug)]
enum Work {
    Lease,
    Command,
}

/// Whether opening a connection may install this build on the host first.
#[derive(Clone, Copy, PartialEq, Eq)]
enum Install<'a> {
    /// For this lease: a host with no copy of this build gets one, and the
    /// install's records name the lease, so they join its issuance.
    IfAbsent(&'a str),
    /// For an account of a lease already held: a copy installed now could
    /// answer only that it holds nothing, so none is installed.
    Never,
}

impl Inner {
    /// Its name in the log and the audit.
    fn name(&self) -> String {
        self.environment().name()
    }

    fn environment(&self) -> EnvironmentRef {
        match &self.reach {
            Reach::Ssh { host, .. } => EnvironmentRef::Ssh(host.clone()),
            Reach::Workspace { .. } => EnvironmentRef::Boat,
            Reach::Peer { peer, .. } => EnvironmentRef::Peer(*peer),
        }
    }

    /// A place in the organization budget for one more `kind` of work, on
    /// a paired environment; `Ok(None)` elsewhere, where nothing is
    /// counted. With every place the budget allows taken, the work is
    /// refused busy before the host is asked.
    fn budget_place(&self, kind: Work) -> Result<Option<BudgetPlace>, LeaseRefusal> {
        let Reach::Peer { budgets, .. } = &self.reach else {
            return Ok(None);
        };
        let place = match kind {
            Work::Lease => budgets.lease(),
            Work::Command => budgets.command(),
        };
        match place {
            Some(place) => Ok(Some(place)),
            None => {
                let (leases, commands) = budgets.running();
                tracing::warn!(
                    host = self.name(),
                    ?kind,
                    leases,
                    commands,
                    "the organization budget is spent: refused busy before the worker is asked"
                );
                Err(LeaseRefusal::EnvironmentBusy)
            }
        }
    }

    fn liveness(&self) -> Liveness {
        Liveness {
            keepalive: self.timings.keepalive,
            silence: self.timings.silence,
        }
    }

    async fn link(&self, install: Install<'_>) -> Result<Arc<HostLink>, LeaseRefusal> {
        if let Reach::Workspace { connector } = &self.reach {
            let mut held = self.link.lock().await;
            if let Some(link) = held.as_ref().filter(|link| link.is_open()) {
                return Ok(link.clone());
            }
            let connection = tokio::time::timeout(
                self.timings.connect,
                connector.connect(install != Install::Never),
            )
            .await
            .map_err(|_| LeaseRefusal::EnvironmentUnreachable)??;
            let link = HostLink::over(
                &self.name(),
                connection,
                self.audit.clone(),
                self.timings.connect,
                self.liveness(),
            )
            .await
            .map_err(|error| match error {
                Opening::Refused(reason) => reason,
                Opening::NotServed => LeaseRefusal::EnvironmentUnreachable,
            })?;
            *held = Some(link.clone());
            return Ok(link);
        }
        let (host, connector, installer) = match &self.reach {
            Reach::Workspace { .. } => unreachable!("workspace connection handled above"),
            Reach::Ssh {
                host,
                connector,
                installer,
            } => (host, connector, installer),
            Reach::Peer {
                credentials,
                serving,
                ..
            } => return self.dialed_in(credentials.as_ref(), serving).await,
        };
        let mut held = self.link.lock().await;
        if let Some(link) = held.as_ref().filter(|link| link.is_open()) {
            return Ok(link.clone());
        }
        *held = None;
        let link = match self.open(host, connector.as_ref(), installer).await {
            Err(Opening::NotServed) => match install {
                // Present already: another gateway of this build may have
                // just put it there, so it is asked once more either way.
                Install::IfAbsent(lease) => {
                    installer.ensure(host, lease, self.audit.as_ref()).await?;
                    self.open(host, connector.as_ref(), installer).await
                }
                Install::Never => Err(Opening::NotServed),
            },
            opened => opened,
        };
        let link = link.map_err(|opening| match opening {
            Opening::Refused(refusal) => refusal,
            Opening::NotServed => LeaseRefusal::EnvironmentUnreachable,
        })?;
        *held = Some(link.clone());
        Ok(link)
    }

    async fn open(
        &self,
        host: &SshDestination,
        connector: &dyn LeaseConnector,
        installer: &HostInstaller,
    ) -> Result<Arc<HostLink>, Opening> {
        HostLink::open(
            host,
            installer.serve_command().await?,
            connector,
            self.audit.clone(),
            self.timings.connect,
            self.liveness(),
        )
        .await
    }

    /// A paired environment's current connection, once the key it proved is
    /// asked again whether it still serves: one that no longer does is
    /// closed. Nothing is reached here: with no connection open, the host is
    /// unreachable until it dials in.
    ///
    /// The answer is about the key, and every connection in this slot proved
    /// the same key, so the connection returned is the one held once the ask
    /// returns: a reconnect while it was pending is served, not refused for
    /// the connection it replaced.
    async fn dialed_in(
        &self,
        credentials: &dyn ServingCredentials,
        serving: &std::sync::Mutex<Option<Served>>,
    ) -> Result<Arc<HostLink>, LeaseRefusal> {
        let Served { link, proof } = served(serving)
            .filter(|served| served.link.is_serving())
            .ok_or(LeaseRefusal::EnvironmentUnreachable)?;
        match credentials.serves(&proof).await {
            Ok(true) => served(serving)
                .map(|current| current.link)
                .filter(|current| current.is_serving())
                .ok_or(LeaseRefusal::EnvironmentUnreachable),
            Ok(false) => {
                self.revoke(&link);
                if let Some(current) = served(serving) {
                    self.revoke(&current.link);
                }
                Err(LeaseRefusal::EnvironmentUnreachable)
            }
            Err(error) => {
                tracing::warn!(host = self.name(), %error, "whether a paired environment still serves could not be read");
                Err(LeaseRefusal::EnvironmentUnreachable)
            }
        }
    }

    /// Answers once the key `proof` carries no longer serves, asked again
    /// every keepalive interval: the admission's answer held only for its
    /// moment, and a credential revoked, expired or whose membership was
    /// disabled since then says nothing to this connection by itself. An
    /// answer that cannot be read is not a revocation: the next one is
    /// asked, and the next lease asks for itself.
    async fn until_revoked(
        &self,
        credentials: &dyn ServingCredentials,
        proof: &DeviceConnectionProof,
    ) {
        loop {
            tokio::time::sleep(self.timings.keepalive).await;
            match credentials.serves(proof).await {
                Ok(true) => {}
                Ok(false) => return,
                Err(error) => {
                    tracing::warn!(host = self.name(), %error, "whether a paired environment still serves could not be read");
                }
            }
        }
    }

    /// Revoke a paired environment's connection: its leases end as revoked
    /// on both sides, and it closes once they are let go, or past the
    /// silence bound, the longest a connection is believed without a frame.
    /// A revoked connection serves no more leases, so it leaves the
    /// connected list now, and that is said once it has.
    fn revoke(&self, link: &Arc<HostLink>) {
        let revoked = link.revoke(self.timings.silence);
        if let (true, Reach::Peer { connections, .. }) = (revoked, &self.reach) {
            moved(connections);
        }
    }

    /// What the host recorded of `lease`, asked on the connection there is
    /// now: the evidence for a lease whose own connection was lost.
    async fn account(&self, lease: &str) -> Option<LeaseCleanup> {
        let link = self.link(Install::Never).await.ok()?;
        let cleanup = tokio::time::timeout(self.timings.answer, link.account(lease))
            .await
            .ok()??;
        evidence(cleanup)
    }
}

/// The host's cleanup as lease evidence: uncertain is none.
fn evidence(cleanup: Cleanup) -> Option<LeaseCleanup> {
    match cleanup {
        Cleanup::Confirmed { forced } => Some(LeaseCleanup::Confirmed { forced }),
        Cleanup::NotHeld => Some(LeaseCleanup::NotHeld),
        Cleanup::Uncertain => None,
    }
}

impl Environment for SshEnvironment {
    fn declaration(&self) -> EnvironmentDeclaration {
        EnvironmentDeclaration {
            environment: self.inner.environment(),
            sandbox: SandboxProfiles::HARNESS_DEFAULT,
        }
    }

    fn open<'a>(
        &'a self,
        lease: &'a LeaseId,
        grant: &'a LeaseTerms,
        binding: Arc<dyn AgentProvider>,
    ) -> EnvironmentFuture<'a, Result<EnvironmentLease, LeaseRefusal>> {
        Box::pin(async move {
            let LeaseWork::Agent(work) = &grant.work;
            let place = self.inner.budget_place(Work::Lease)?;
            let link = self.inner.link(Install::IfAbsent(lease.as_str())).await?;
            let released = Arc::new(std::sync::OnceLock::new());
            let host: Arc<dyn HarnessHost> = Arc::new(LeaseHost {
                released: released.clone(),
                link: link.clone(),
                lease: lease.as_str().into(),
            });
            let provider = binding.on_host(host.clone()).map_err(|error| {
                tracing::warn!(agent = work.agent(), %error, "this agent's binding cannot start its harness on a host");
                LeaseRefusal::AgentUnavailable
            })?;
            let Granted {
                terms,
                gone,
                ending,
                published,
            } = link
                .grant(
                    lease.as_str(),
                    work.agent(),
                    grant.sandbox.into(),
                    place,
                    self.inner.timings.answer,
                )
                .await?;
            let workspace = Some(link.workspace().to_string_lossy().into_owned());
            Ok(EnvironmentLease {
                provider,
                workspace,
                granted: granted(terms),
                hold: Arc::new(SshHold {
                    released,
                    host,
                    inner: self.inner.clone(),
                    link,
                    lease: lease.as_str().into(),
                    gone,
                    ending,
                    published: std::sync::Mutex::new(Some(published)),
                }),
            })
        })
    }

    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(self.inner.account(lease.as_str()))
    }

    fn commands(&self) -> Option<&dyn CommandEnvironment> {
        Some(self)
    }
}

impl CommandEnvironment for SshEnvironment {
    fn reachable(&self) -> Option<bool> {
        if let Reach::Peer { serving, .. } = &self.inner.reach {
            return served(serving)
                .is_some_and(|served| served.link.is_serving())
                .then_some(true);
        }
        let held = self.inner.link.try_lock().ok()?;
        held.as_ref()
            .is_some_and(|link| link.is_open())
            .then_some(true)
    }

    fn grant<'a>(
        &'a self,
        lease: &'a LeaseId,
        command: &'a CommandWork,
    ) -> EnvironmentFuture<'a, Result<GrantedCommand, CommandRefusal>> {
        Box::pin(async move {
            let place = self
                .inner
                .budget_place(Work::Command)
                .map_err(CommandRefusal::Environment)?;
            let link = self
                .inner
                .link(Install::IfAbsent(lease.as_str()))
                .await
                .map_err(CommandRefusal::Environment)?;
            let admitted = link
                .grant_command(
                    lease.as_str(),
                    command
                        .argv()
                        .iter()
                        .map(|argument| argument.to_string())
                        .collect(),
                    command.cwd().map(Into::into),
                    command.timeout_ms(),
                    place,
                    self.inner.timings.answer,
                )
                .await
                .map_err(|refused| command_refusal(&self.inner.name(), refused))?;
            // The terms as the host answered them: the service checks them
            // and runs the hold with the timeout it checked.
            Ok(GrantedCommand {
                hold: Box::new(SshCommand {
                    inner: self.inner.clone(),
                    link,
                    lease: lease.as_str().into(),
                    published: std::sync::Mutex::new(Some(admitted.published)),
                    gone: admitted.gone,
                }),
                granted: granted(admitted.terms),
            })
        })
    }
}

/// The terms a host granted, as the gateway's.
fn granted(terms: lease::GrantedTerms) -> GrantedTerms {
    GrantedTerms {
        sandbox: terms.sandbox.into(),
        timeout_ms: terms.timeout_ms,
        lease_ms: terms.lease_ms,
    }
}

/// A host's refusal of a command lease, as the gateway's.
fn command_refusal(host: &str, refused: AdmissionRefused) -> CommandRefusal {
    match refused {
        AdmissionRefused::Host(GrantRefusal::CommandsUnavailable) => {
            CommandRefusal::CommandsUnavailable
        }
        // The host's owner grants this gateway no commands there: the
        // environment's refusal, not this gateway's own policy.
        AdmissionRefused::Host(GrantRefusal::NotGranted) => {
            CommandRefusal::Environment(LeaseRefusal::EnvironmentNotGranted)
        }
        AdmissionRefused::Host(GrantRefusal::SandboxUnavailable) => {
            CommandRefusal::Environment(LeaseRefusal::SandboxUnavailable)
        }
        // The gateway checked it against the same bounds: a host that
        // disagrees has another reading of its workspace, so it is not run.
        AdmissionRefused::Host(GrantRefusal::InvalidCommand) => {
            tracing::warn!(host, "the host refused a command the gateway admitted");
            CommandRefusal::CommandDenied
        }
        AdmissionRefused::Host(
            GrantRefusal::AgentUnavailable
            | GrantRefusal::Duplicate
            | GrantRefusal::AuditUnavailable
            | GrantRefusal::Revoked
            | GrantRefusal::Draining,
        )
        | AdmissionRefused::Unreachable => {
            CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable)
        }
    }
}

/// The gateway's side of one command lease on a host. Dropped, it ends the
/// lease there unless an End is already queued: a command that finished
/// queues its own from its `Ran`, and the route stays until `Ended`.
struct SshCommand {
    inner: Arc<Inner>,
    link: Arc<HostLink>,
    lease: String,
    /// The files published under it, until [`CommandHold::publications`]
    /// takes them.
    published: std::sync::Mutex<Option<mpsc::Receiver<Published>>>,
    /// Closed once the lease is no longer routed on its connection.
    gone: watch::Receiver<()>,
}

impl CommandHold for SshCommand {
    fn publications(&mut self) -> Option<mpsc::Receiver<ArtifactOffer>> {
        let published = self
            .published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()?;
        Some(offer_artifacts(
            self.link.clone(),
            self.lease.clone(),
            published,
            self.gone.clone(),
        ))
    }

    fn run(
        self: Box<Self>,
        timeout_ms: u64,
        mut stop: watch::Receiver<Option<LeaseEndCause>>,
    ) -> EnvironmentFuture<'static, CommandResult> {
        Box::pin(async move {
            // The host answers within the command's timeout and what stopping
            // it takes; a stop asked shortens nothing the host already owes.
            // Stopped before it was run: it is never run, and its lease ends.
            if stop.borrow().is_some() {
                let ended =
                    tokio::time::timeout(self.inner.timings.answer, self.link.end(&self.lease))
                        .await
                        .ok()
                        .flatten();
                return CommandResult {
                    exit: CommandExit::NotStarted,
                    cleanup: ended.and_then(evidence),
                    ..CommandResult::unanswered()
                };
            }
            let bound =
                Duration::from_millis(timeout_ms) + COMMAND_STOP_WAIT + self.inner.timings.answer;
            let mut stopped = None;
            let answer = tokio::time::timeout(bound, async {
                let run = self.link.run(&self.lease);
                tokio::pin!(run);
                loop {
                    tokio::select! {
                        biased;
                        Ok(cause) = stop.wait_for(Option::is_some), if stopped.is_none() => {
                            stopped = *cause;
                            // Its End stops it; its Ran still answers.
                            self.link.abandon_lease(&self.lease);
                        }
                        answer = &mut run => break answer,
                    }
                }
            })
            .await
            .ok()
            .flatten();
            // The host stops a command itself once told it is revoked.
            let stopped = stopped.or(self.link.revoked().then_some(LeaseEndCause::Revoked));
            let mut result = match answer {
                Some(answer) => command_result(answer, stopped),
                None => CommandResult {
                    cleanup: self.inner.account(&self.lease).await,
                    ..CommandResult::unanswered()
                },
            };
            // A transition of its lease that went unrecorded (its
            // revocation, its connection's loss) settles nothing.
            if self.link.unaudited(&self.lease) {
                result.cleanup = None;
            }
            result
        })
    }
}

impl Drop for SshCommand {
    fn drop(&mut self) {
        self.link.abandon_lease(&self.lease);
        self.link.forget(&self.lease);
    }
}

/// What a host's `Ran` says, as the command's result. A stop the gateway
/// asked is the cause it was asked for.
fn command_result(answer: CommandAnswer, stopped: Option<LeaseEndCause>) -> CommandResult {
    let exit = match answer.end {
        CommandEnd::Exited { code } => CommandExit::Exited { code },
        CommandEnd::Signalled { signal } => CommandExit::Signalled { signal },
        CommandEnd::TimedOut => CommandExit::TimedOut,
        CommandEnd::Stopped => CommandExit::Stopped {
            cause: stopped.unwrap_or(LeaseEndCause::Lost),
        },
        // The host stopping is a system stop, as a drained lease is.
        CommandEnd::Drained => CommandExit::Stopped {
            cause: LeaseEndCause::Stopped,
        },
        CommandEnd::NotStarted => CommandExit::NotStarted,
        CommandEnd::Unknown => CommandExit::Unanswered,
    };
    CommandResult {
        exit,
        stdout: answer.stdout,
        stderr: answer.stderr,
        dropped_bytes: answer.dropped_bytes,
        cleanup: evidence(answer.cleanup),
        published: Vec::new(),
    }
}

/// Offer each file `published` carries as an [`ArtifactOffer`], until the
/// lease is no longer routed. The same pump a harness lease and a command
/// lease use.
fn offer_artifacts(
    link: Arc<HostLink>,
    lease: String,
    mut published: mpsc::Receiver<Published>,
    gone: watch::Receiver<()>,
) -> mpsc::Receiver<ArtifactOffer> {
    let (offers, taken) = mpsc::channel(1);
    tokio::spawn(async move {
        while let Some(Published {
            artifact,
            file,
            request,
            unanswered,
        }) = published.recv().await
        {
            let answering = link.clone();
            let answered = lease.clone();
            let bytes: Box<dyn ArtifactBytes> = match link.artifact_transport() {
                ArtifactTransport::Channels(channels) => Box::new(SftpBytes::new(
                    channels,
                    file.path.clone(),
                    file.size,
                    gone.clone(),
                )),
                ArtifactTransport::Frames => Box::new(FrameBytes::new(
                    link.clone(),
                    lease.clone(),
                    artifact,
                    file.size,
                )),
            };
            let offer = ArtifactOffer {
                bytes,
                file,
                answer: ArtifactAnswer::new(move |outcome| {
                    answering.collected(&answered, artifact, outcome, unanswered)
                }),
                request,
            };
            if offers.send(offer).await.is_err() {
                break;
            }
        }
    });
    taken
}

/// The gateway's side of one lease on a host.
struct SshHold {
    /// Actual audited End/account receipt, shared only with this lease's controls.
    released: Arc<std::sync::OnceLock<CloseOutcome>>,
    host: Arc<dyn HarnessHost>,
    inner: Arc<Inner>,
    link: Arc<HostLink>,
    lease: String,
    /// Closed once the lease is no longer routed on its connection: taken
    /// at its grant, so a loss before anyone asks is still seen.
    gone: watch::Receiver<()>,
    /// Why it is ending unasked, once known: the host said so, or this
    /// gateway revoked the host.
    ending: watch::Receiver<Option<LeaseEndCause>>,
    /// The files published under it, until [`LeaseHold::artifacts`] takes
    /// them.
    published: std::sync::Mutex<Option<mpsc::Receiver<Published>>>,
}

impl LeaseHold for SshHold {
    fn model_provider(
        &self,
        binding: Arc<dyn AgentProvider>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        binding.on_host(self.host.clone())
    }

    fn ends(&self) -> Option<EnvironmentFuture<'static, LeaseEndCause>> {
        let mut ending = self.ending.clone();
        Some(Box::pin(async move {
            // The host saying it is ending the lease, this gateway revoking
            // the host, or the route's end; a route already gone answers at
            // once. A cause known before the route went is still what
            // ended it.
            let _ = ending.wait_for(Option::is_some).await;
            let known = *ending.borrow();
            known.unwrap_or(LeaseEndCause::Lost)
        }))
    }

    fn artifacts(&self) -> Option<mpsc::Receiver<ArtifactOffer>> {
        let published = self
            .published
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .take()?;
        Some(offer_artifacts(
            self.link.clone(),
            self.lease.clone(),
            published,
            self.gone.clone(),
        ))
    }

    /// Asked over the lease's own connection; its file is offered on the
    /// receiver [`Self::artifacts`] handed out, with every other.
    fn publish(
        &self,
        ask: PublishAsk,
    ) -> Option<EnvironmentFuture<'static, Result<(), PublishFailure>>> {
        let link = self.link.clone();
        let lease = self.lease.clone();
        Some(Box::pin(async move {
            link.publish(&lease, ask.request, ask.path, ask.media_type)
                .await
        }))
    }

    /// Asked over the lease's own connection; its bytes go over an artifact
    /// channel of that connection, never its control channel. `None` on a
    /// connection that carries no files, as a paired one.
    fn fetch(
        &self,
        ask: FetchAsk,
    ) -> Option<EnvironmentFuture<'static, Result<FetchPlaced, FetchFailure>>> {
        // Bytes that could never be written are never asked for.
        if !self.link.writes_files() {
            return None;
        }
        let link = self.link.clone();
        let lease = self.lease.clone();
        let gone = self.gone.clone();
        Some(Box::pin(async move { link.fetch(&lease, ask, gone).await }))
    }

    fn end(&self, _cause: LeaseEndCause) -> EnvironmentFuture<'_, LeaseRelease> {
        Box::pin(async move {
            let answered = match self.link.is_open() {
                true => self.link.end(&self.lease).await.map(evidence),
                false => None,
            };
            let evidence = match answered {
                Some(evidence) => evidence,
                // The connection was lost: the host ended the lease as lost
                // and recorded what that took, which a new connection asks.
                None => self.inner.account(&self.lease).await,
            };
            if !self.link.is_open() {
                // Its routes are let go only once the loss was recorded, or
                // known unrecorded.
                let mut gone = self.gone.clone();
                while gone.changed().await.is_ok() {}
            }
            // A transition of it that went unrecorded settles nothing.
            if self.link.unaudited(&self.lease) {
                tracing::warn!(
                    lease = self.lease,
                    "a transition of this lease went unrecorded; its end has no evidence"
                );
                return LeaseRelease::Unanswered;
            }
            match evidence {
                Some(cleanup) => {
                    let physical = match cleanup {
                        LeaseCleanup::Confirmed { forced } => CloseOutcome { forced },
                        LeaseCleanup::NotHeld => CloseOutcome { forced: false },
                    };
                    let _ = self.released.set(physical);
                    LeaseRelease::Released(cleanup)
                }
                None => LeaseRelease::Unanswered,
            }
        })
    }
}

impl Drop for SshHold {
    /// Ends the lease on the host unless its End is already queued or it is
    /// no longer held there.
    fn drop(&mut self) {
        self.link.abandon_lease(&self.lease);
        self.link.forget(&self.lease);
    }
}

/// The host as a binding sees it, for one lease.
struct LeaseHost {
    released: Arc<std::sync::OnceLock<CloseOutcome>>,
    link: Arc<HostLink>,
    lease: String,
}

impl HarnessHost for LeaseHost {
    fn mcp_relay(
        &self,
    ) -> Option<&dyn nessa_sdk::application::agent_execution::providers::HarnessMcpRelay> {
        self.link.mcp_relay()
    }

    fn workspace(&self) -> &Path {
        self.link.workspace()
    }

    fn start(&self, launch: HarnessLaunch) -> Result<HarnessProcess, AgentError> {
        let mut environment = BTreeMap::new();
        for (key, value) in launch.environment {
            match (key.into_string(), value.into_string()) {
                (Ok(key), Ok(value)) => {
                    environment.insert(key, value);
                }
                _ => {
                    return Err(AgentError::InvalidInput(
                        "a launch variable is not UTF-8".into(),
                    ))
                }
            }
        }
        let started = self
            .link
            .start(&self.lease, environment)
            .map_err(|error| match error {
                StartError::Closed => AgentError::Closed,
                StartError::Busy => AgentError::Backpressure,
                StartError::TooLarge => {
                    AgentError::InvalidInput("the harness's launch variables are too large".into())
                }
            })?;
        Ok(HarnessProcess {
            input: Box::new(started.input),
            output: Box::new(started.output),
            control: Box::new(SshControl {
                released: self.released.clone(),
                link: self.link.clone(),
                lease: self.lease.clone(),
                channel: started.channel,
            }),
        })
    }
}

/// How the binding stops its harness on the host.
struct SshControl {
    released: Arc<std::sync::OnceLock<CloseOutcome>>,
    link: Arc<HostLink>,
    lease: String,
    channel: u32,
}

impl HarnessControl for SshControl {
    fn cleanup(&mut self, grace: Duration, kill_timeout: Duration) -> HarnessCleanupFuture<'_> {
        Box::pin(async move {
            // End may have been acknowledged on a replacement connection. Join
            // that same lease owner's real receipt even if this control's old
            // connection is now gone; the SDK still releases its own use guard.
            if let Some(outcome) = self.released.get() {
                return Ok(*outcome);
            }
            match self
                .link
                .stop(&self.lease, self.channel, grace, kill_timeout)
                .await
            {
                Some(Cleanup::Confirmed { forced }) => Ok(CloseOutcome { forced }),
                Some(Cleanup::NotHeld) => Ok(CloseOutcome { forced: false }),
                Some(Cleanup::Uncertain) | None => Err(AgentError::CleanupUncertain),
            }
        })
    }
}

impl Drop for SshControl {
    /// Asks the host to stop the harness unless a stop was already asked of
    /// it: its Stop is sent once, whoever asks.
    fn drop(&mut self) {
        self.link.abandon(&self.lease, self.channel);
    }
}
