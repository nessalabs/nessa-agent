//! The SSH environment: a conversation's agent runs on a host this gateway
//! reaches with `ssh <host> nessa env serve`, under a lease the host admits.
//!
//! ```text
//! open(lease, grant, binding)
//!   ──▶ link (one per host: connect, hello, this build?)
//!   ──▶ binding.on_host(LeaseHost) ──▶ the same binding, starting its harness there
//!   ──▶ link.grant(lease, agent) ──▶ EnvironmentLease { provider, SshHold }
//! binding opens a session ──▶ LeaseHost::start ──▶ Start{lease, channel}
//!   harness stdin/stdout ⇄ Input / Output frames
//!   close ──▶ SshControl::cleanup ──▶ (after its input) Stop ──▶ Stopped{cleanup}
//! LiveLease::lost ──▶ the lease's watch, taken at its grant
//! LiveLease::close ──▶ SshHold::end ──▶ End ──▶ Ended{cleanup}
//!   connection lost? ──▶ a new connection ──▶ Account ──▶ what the host recorded
//! account(lease) ──▶ Account ──▶ Accounted{cleanup}
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
    connector::LeaseConnector,
    link::{HostLink, StartError},
};
use crate::conversation::application::{
    Environment, EnvironmentDeclaration, EnvironmentFuture, EnvironmentLease, LeaseHold,
    LeaseRelease,
};
use nessa_protocol::lease::{Cleanup, KEEPALIVE_INTERVAL};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{
        AgentProvider, CloseOutcome, HarnessCleanupFuture, HarnessControl, HarnessHost,
        HarnessLaunch, HarnessProcess,
    },
};
use nessa_sdk::domain::agent_execution::leases::{
    EnvironmentRef, LeaseCleanup, LeaseEndCause, LeaseId, LeaseRefusal, LeaseTerms, LeaseWork,
    SandboxProfiles, SshDestination,
};
use std::{collections::BTreeMap, path::Path, sync::Arc, time::Duration};
use tokio::sync::{watch, Mutex};

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
}

impl Default for SshTimings {
    fn default() -> Self {
        Self {
            connect: Duration::from_secs(30),
            answer: Duration::from_secs(15),
            keepalive: KEEPALIVE_INTERVAL,
        }
    }
}

/// One SSH host, as an environment.
pub(crate) struct SshEnvironment {
    inner: Arc<Inner>,
}

struct Inner {
    host: SshDestination,
    connector: Arc<dyn LeaseConnector>,
    audit: Arc<dyn EnvironmentAudit>,
    timings: SshTimings,
    /// The one connection to the host, opened when first needed and opened
    /// again once lost. Held while connecting, so one is opened at a time.
    link: Mutex<Option<Arc<HostLink>>>,
}

impl SshEnvironment {
    pub(crate) fn new(
        host: SshDestination,
        connector: Arc<dyn LeaseConnector>,
        audit: Arc<dyn EnvironmentAudit>,
        timings: SshTimings,
    ) -> Self {
        Self {
            inner: Arc::new(Inner {
                host,
                connector,
                audit,
                timings,
                link: Mutex::new(None),
            }),
        }
    }
}

impl Inner {
    async fn link(&self) -> Result<Arc<HostLink>, LeaseRefusal> {
        let mut held = self.link.lock().await;
        if let Some(link) = held.as_ref().filter(|link| link.is_open()) {
            return Ok(link.clone());
        }
        *held = None;
        let link = HostLink::open(
            &self.host,
            self.connector.as_ref(),
            self.audit.clone(),
            self.timings.connect,
            self.timings.keepalive,
        )
        .await?;
        *held = Some(link.clone());
        Ok(link)
    }

    /// What the host recorded of `lease`, asked on the connection there is
    /// now: the evidence for a lease whose own connection was lost.
    async fn account(&self, lease: &str) -> Option<LeaseCleanup> {
        let link = self.link().await.ok()?;
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
            environment: EnvironmentRef::Ssh(self.inner.host.clone()),
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
            let link = self.inner.link().await?;
            let host = Arc::new(LeaseHost {
                link: link.clone(),
                lease: lease.as_str().into(),
            });
            let provider = binding.on_host(host).map_err(|error| {
                tracing::warn!(agent = work.agent(), %error, "this agent's binding cannot start its harness on a host");
                LeaseRefusal::AgentUnavailable
            })?;
            let gone = link
                .grant(lease.as_str(), work.agent(), self.inner.timings.answer)
                .await?;
            Ok(EnvironmentLease {
                provider,
                hold: Arc::new(SshHold {
                    inner: self.inner.clone(),
                    link,
                    lease: lease.as_str().into(),
                    gone,
                }),
            })
        })
    }

    fn account<'a>(&'a self, lease: &'a LeaseId) -> EnvironmentFuture<'a, Option<LeaseCleanup>> {
        Box::pin(self.inner.account(lease.as_str()))
    }
}

/// The gateway's side of one lease on a host.
struct SshHold {
    inner: Arc<Inner>,
    link: Arc<HostLink>,
    lease: String,
    /// Closed once the lease is no longer routed on its connection: taken
    /// at its grant, so a loss before anyone asks is still seen.
    gone: watch::Receiver<()>,
}

impl LeaseHold for SshHold {
    fn lost(&self) -> Option<EnvironmentFuture<'static, ()>> {
        let mut gone = self.gone.clone();
        Some(Box::pin(async move {
            // Only the route's end is waited for: it is never sent to, and
            // a route already gone answers at once.
            while gone.changed().await.is_ok() {}
        }))
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
            match evidence {
                Some(cleanup) => LeaseRelease::Released(cleanup),
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
    }
}

/// The host as a binding sees it, for one lease.
struct LeaseHost {
    link: Arc<HostLink>,
    lease: String,
}

impl HarnessHost for LeaseHost {
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
                link: self.link.clone(),
                lease: self.lease.clone(),
                channel: started.channel,
            }),
        })
    }
}

/// How the binding stops its harness on the host.
struct SshControl {
    link: Arc<HostLink>,
    lease: String,
    channel: u32,
}

impl HarnessControl for SshControl {
    fn cleanup(&mut self, grace: Duration, kill_timeout: Duration) -> HarnessCleanupFuture<'_> {
        Box::pin(async move {
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
