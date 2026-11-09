//! One connection to a host's `nessa env serve`, shared by every lease this
//! gateway holds there, and the routing of its frames to them.
//!
//! ```text
//! open: connect ──▶ first frame ──▶ read_hello ──▶ protocol == LEASE_PROTOCOL?
//!          the stream ended first ──▶ NotServed: nothing installed there yet?
//!          no ──▶ VersionRefused, nothing sent ──▶ EnvironmentVersionMismatch
//!          empty workspace ──▶ Unavailable{busy | notConfigured} ──▶ refused
//!          yes ──▶ writer task (frames out), demux task (frames in)
//! writer: frames out, in the order queued; a Keepalive when idle `keepalive`
//! demux: nothing read for `silence`, keepalives included ──▶ end of stream
//! demux: frame ──▶ lease route? ──▶ grant / channel output / stopped / ended
//!                   published ──▶ the lease's offers ──▶ (taken) Collected
//!                     no room, or nobody takes them ──▶ Collected refused
//!                     naming a request ──▶ that publish's waiter: offered, or refused
//!                   not published ──▶ that publish's waiter: the host's reason
//!                   fetch accepted / refused / fetched ──▶ that fetch's waiter
//!                   ending ──▶ the lease's `ending` watch: the host is ending it
//!                   draining, or a grant refused draining ──▶ the link is marked
//!                     draining: the pool sends it no new lease (is_draining)
//!                   none ──▶ FrameDropped, never applied elsewhere (row L9)
//! end of stream ──▶ every route dropped: waiters unanswered, outputs ended,
//!                   each lease's `gone` closed ──▶ ConnectionLost
//! revoke(drain) ──▶ no lease admitted any more ──▶ Revoked{leases} recorded
//!   ──▶ Revoked to the host ──▶ each harness lease's ending cause: revoked
//!   ──▶ every lease let go (each End answered), or `drain` ──▶ close
//! publish(lease, request) ──▶ waiter by request ──▶ Publish ──▶ Published | NotPublished
//! fetch(lease, ask) ──▶ waiter by request ──▶ Fetch
//!   ──▶ FetchRefused | Fetched (already there)
//!   ──▶ FetchAccepted{path} ──▶ write_part over an artifact channel
//!         written ──▶ Delivered ──▶ Fetched{outcome}
//!         not written ──▶ Undelivered (unanswered)
//!   the call gone after Fetch, before the last word or Delivered ──▶ Undelivered
//! read_artifact(lease, artifact, offset) ──▶ waiter by artifact and offset
//!   ──▶ ReadArtifact ──▶ ArtifactRead | ArtifactNotRead (a paired connection's
//!   file bytes, which have no artifact channel beside it)
//! a command lease: grant_command ──▶ GrantCommand ──▶ Granted{terms} | Refused
//!                  run ──▶ Run ──▶ Ran (routed to its waiter). Its End is
//!                  asked then, after every file already offered is answered,
//!                  unless one was already asked; the route stays until Ended
//! a harness: binding's input ──▶ its pump ──▶ Input… InputClosed ──▶ Stop
//!            (cleanup, let go, or output overflow ask its pump for the Stop;
//!             a Stopped the host sent unasked, for its input overflow, ends
//!             the pump, so the binding's writes fail)
//! ```
//!
//! Arrows are steps and frames, in order. A lease's frames are routed only
//! while the gateway holds that lease on this connection: from its grant
//! until the host's `Ended`, a refusal, or the connection's end. Every wait
//! here is bounded by its caller or by a deadline of its own, and no lock is
//! held across one.
//!
//! Each harness's frames have one sender, its input pump, so they reach the
//! host in the order its binding gave them: a stop, however it is asked,
//! goes after the input written before it, never overtaking it. A lease's
//! End goes after every frame its harnesses' pumps still send: asking for it
//! closes each pump not asked to stop, and it is queued only once every pump
//! under the lease is done, so it never overtakes a Stop already asked. A
//! stop or an end asked by a binding or hold that let go, or by an overflow,
//! is always delivered — queued behind what is already waiting, on a task of
//! its own — never dropped for want of room.
//!
//! The gateway's audit of a transition (an output overflow, a lost
//! connection) is part of it: when the record cannot be written, the
//! harness is still stopped and the lease still ended, but neither is
//! settled as confirmed — the harness's stop answers uncertain and the
//! lease's end has no evidence, so it is Interrupted. A dropped frame's
//! record failing is logged only: it records no transition of any lease.
use super::audit::{EnvironmentAudit, EnvironmentEvent};
use super::connector::{ArtifactTransport, LeaseConnection, LeaseConnector};
use super::transfer::{write_part, WriteFailure};
use crate::conversation::application::{
    ArtifactAnswering, ArtifactChannels, BudgetPlace, FetchAsk, FetchFailure, FetchPlaced,
    PublishFailure,
};
use crate::env::LEASE_PROTOCOL;
use crate::env_serve::application::FrameStream;
use crate::env_serve::install::Platform;
use futures_util::FutureExt;
use nessa_protocol::build_update::{self, FromEnvironment::UpdateWanted};
use nessa_protocol::lease::{
    decode, encode, read_hello, stop_steps, ArtifactReadRefusal, Cleanup, Collection,
    CollectionRefusal, CommandEnd, Data, EndingCause, FetchOutcome, FetchRefusal, FromEnvironment,
    GrantRefusal, GrantedTerms, MachineReport, PublishRefusal, Sandbox, StagedArtifact,
    ToEnvironment, Unavailability, COMMAND_STOP_WAIT, MAX_ARTIFACTS_IN_FLIGHT, MAX_DATA_BYTES,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseEndCause, LeaseRefusal, SshDestination};
use std::{
    collections::{hash_map::Entry, BTreeMap, HashMap, HashSet},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    sync::{mpsc, oneshot, watch, OwnedRwLockReadGuard, RwLock},
};

/// Most frames queued for the host; a harness's input waits for room.
const TO_HOST_QUEUE: usize = 256;
/// Most output chunks queued for one harness's binding; past it the
/// harness's output is ended and the harness stopped.
const OUTPUT_QUEUE: usize = 128;
/// The in-memory pipe between a binding and its harness's frames.
const PIPE_BYTES: usize = 64 * 1024;
/// Most drops one connection records; more are counted in the log.
const MAX_RECORDED_DROPS: u32 = 64;
/// How long a stop waits beyond every step the host takes
/// ([`stop_steps`]) for its answer: the host recording it, and the frames'
/// way there and back.
const STOP_MARGIN: Duration = Duration::from_secs(5);
/// Each forced step's budget for a harness stopped without its binding's
/// own budgets: one let go, or one whose output overflowed.
const ABANDON_KILL_MS: u64 = 5_000;

/// A stop for one harness, sent by its input pump after its input.
#[derive(Clone, Copy)]
struct StopRequest {
    grace_ms: u64,
    kill_ms: u64,
}

impl StopRequest {
    /// The stop of a harness nobody waits on any more.
    const ABANDONED: Self = Self {
        grace_ms: 0,
        kill_ms: ABANDON_KILL_MS,
    };

    /// How long its answer is waited for.
    fn bound(self) -> Duration {
        stop_steps(
            Duration::from_millis(self.grace_ms),
            Duration::from_millis(self.kill_ms),
        ) + STOP_MARGIN
    }
}

/// One harness under a lease, as the demux routes to it.
struct ChannelRoute {
    /// Its binding's output; `None` once ended.
    output: Option<mpsc::Sender<Vec<u8>>>,
    /// Asks its input pump for the one Stop it sends; `None` once asked.
    stop: Option<oneshot::Sender<StopRequest>>,
    /// The stop asked of it, once asked.
    asked: Option<StopRequest>,
    /// Waiting for the host's `Stopped`.
    stopped: Option<oneshot::Sender<Cleanup>>,
    /// The host's `Stopped`, once it came: kept, so a stop asked after the
    /// host already stopped it (an output overflow's) is answered with it.
    cleanup: Option<Cleanup>,
    /// The host said it never started.
    start_failed: bool,
}

/// What a command lease's `Ran` said.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandAnswer {
    pub(crate) end: CommandEnd,
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    pub(crate) dropped_bytes: u64,
    pub(crate) cleanup: Cleanup,
}

/// Why a host did not admit a lease: its typed refusal, or no answer.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum AdmissionRefused {
    /// The host refused it.
    Host(GrantRefusal),
    /// The connection is lost, or the host did not answer in time.
    Unreachable,
}

/// A host's refusal of a harness's lease, as the lease's: each reason the
/// host gives that the gateway records as its own, and no answer, or one a
/// lease of an agent cannot meet, as unreachable.
fn lease_refusal(refused: AdmissionRefused) -> LeaseRefusal {
    match refused {
        AdmissionRefused::Host(GrantRefusal::AgentUnavailable) => LeaseRefusal::AgentUnavailable,
        AdmissionRefused::Host(GrantRefusal::NotGranted) => LeaseRefusal::EnvironmentNotGranted,
        AdmissionRefused::Host(GrantRefusal::SandboxUnavailable) => {
            LeaseRefusal::SandboxUnavailable
        }
        AdmissionRefused::Host(
            GrantRefusal::Duplicate
            | GrantRefusal::AuditUnavailable
            | GrantRefusal::CommandsUnavailable
            | GrantRefusal::InvalidCommand
            | GrantRefusal::Revoked
            | GrantRefusal::Draining,
        )
        | AdmissionRefused::Unreachable => LeaseRefusal::EnvironmentUnreachable,
    }
}

/// One lease held on this connection.
struct LeaseRoute {
    grant: Option<oneshot::Sender<Result<GrantedTerms, GrantRefusal>>>,
    /// A command lease's run, waiting for its `Ran`.
    ran: Option<oneshot::Sender<CommandAnswer>>,
    /// A command lease's. The host never ends it unasked (`Ending` is
    /// dropped). Its `Ran` asks its End when none was asked already. The
    /// route stays until the host's `Ended`, or, once that `End` is queued
    /// and `Ran` was said, for [`COMMAND_STOP_WAIT`] and then it is
    /// released: a connection that stays up does not keep its place.
    command: bool,
    ended: Option<oneshot::Sender<Cleanup>>,
    /// Its End was asked: no harness starts under it and none is asked to
    /// stop, as its `Ended` answers their stops.
    ending: bool,
    /// Its End is queued for the host, or will be once its pumps are done.
    end_sent: bool,
    /// Its End has been handed to the writer.
    end_queued: bool,
    /// Its `Ran` was handed to whoever ran it.
    ran_answered: bool,
    /// Held for reading by each of its harnesses' pumps while it runs; its
    /// End is queued only once it can be held for writing.
    pumps: Arc<RwLock<()>>,
    channels: HashMap<u32, ChannelRoute>,
    next_channel: u32,
    /// Where the files the host publishes under it are offered: `None`
    /// once whoever takes them let go, and for a command lease.
    published: Option<mpsc::Sender<Published>>,
    /// Why the lease is ending unasked, once known: the host said it is
    /// (`Ending`), or this gateway revoked the host. The first cause known
    /// is the one kept.
    ending_cause: watch::Sender<Option<LeaseEndCause>>,
    /// Each publish this gateway asked under it, by request, waiting for
    /// the host to offer the file or say why not. Dropped with the route,
    /// which answers each unanswered.
    publishes: HashMap<String, oneshot::Sender<Result<(), PublishFailure>>>,
    /// Each fetch this gateway asked under it, by the request it names on
    /// the wire, waiting for the host's answers. Dropped with the route,
    /// which ends each wait.
    fetches: HashMap<String, mpsc::Sender<FetchAnswer>>,
    /// Each read of a published file this gateway asked under it, by
    /// artifact and offset, waiting for the host's bytes or why not.
    /// Dropped with the route, which ends each wait.
    reads: HashMap<(u32, u64), oneshot::Sender<ArtifactReadAnswer>>,
    /// Dropped with the route: the lease is no longer routed here.
    gone: watch::Sender<()>,
    /// Its place in the organization budget, on a paired environment:
    /// given back with the route, once the host answered its end or the
    /// connection was lost, as until then the host may still hold it.
    _place: Option<BudgetPlace>,
}

/// What the host answered one read of a published file: its bytes, or why
/// not.
pub(crate) type ArtifactReadAnswer = Result<Vec<u8>, ArtifactReadRefusal>;

#[derive(Default)]
struct Routes {
    /// The connection ended: nothing more is routed or asked.
    closed: bool,
    /// The host no longer holds a credential to serve this gateway: no
    /// lease is admitted any more, and the connection closes once the
    /// leases on it are let go.
    revoked: bool,
    leases: HashMap<String, LeaseRoute>,
    accounts: HashMap<String, Vec<oneshot::Sender<Cleanup>>>,
    drops: u32,
    /// Leases a transition of which could not be recorded in the audit:
    /// none of their outcomes is settled as confirmed.
    unaudited: HashSet<String>,
    /// How many fetches this connection has asked: each names itself on
    /// the wire by its call and this count, so no two asks share a name.
    fetches_asked: u64,
    /// The host said it is stopping (`Draining`), or refused a lease as
    /// draining: it admits nothing new on this connection.
    draining: Drains,
    /// What the host last said its machine is and how busy
    /// (`FromEnvironment::Machine`); `None` until it says.
    machine: Reports,
}

/// The host's latest machine report, for whoever waits to learn it.
struct Reports(watch::Sender<Option<MachineReport>>);

impl Default for Reports {
    fn default() -> Self {
        Self(watch::channel(None).0)
    }
}

/// Whether the host said it is stopping, for whoever waits to learn it.
struct Drains(watch::Sender<bool>);

impl Default for Drains {
    fn default() -> Self {
        Self(watch::channel(false).0)
    }
}

/// What the host said of a fetch, as the demux hands it on.
#[derive(Debug)]
enum FetchAnswer {
    /// Taken: its bytes go to this part.
    Accepted(String),
    /// Not taken.
    Refused(FetchRefusal),
    /// Its last word.
    Fetched(FetchOutcome),
}

/// A file the host published under a lease, as the demux hands it on.
pub(crate) struct Published {
    pub(crate) artifact: u32,
    pub(crate) file: StagedArtifact,
    /// The gateway's publish it answers; `None` for a harness's.
    pub(crate) request: Option<String>,
    /// Held, as a pump holds it, until its answer is queued: the lease's
    /// End waits for it, so a file answered before the End is told before it.
    pub(crate) unanswered: OwnedRwLockReadGuard<()>,
}

/// A command lease the host admitted: the terms it granted, the files
/// published under it, and the watch that closes when it is no longer routed.
pub(crate) struct AdmittedCommand {
    /// As the host answered them.
    pub(crate) terms: GrantedTerms,
    /// Closes once the lease is no longer routed here.
    pub(crate) gone: watch::Receiver<()>,
    /// Ends once the lease is no longer routed here.
    pub(crate) published: mpsc::Receiver<Published>,
}

/// What a granted lease is handed: the terms the host granted, its watch,
/// and the files published under it.
pub(crate) struct Granted {
    /// As the host answered them; the gateway checks them against what it
    /// asked.
    pub(crate) terms: GrantedTerms,
    /// Closes once the lease is no longer routed here.
    pub(crate) gone: watch::Receiver<()>,
    /// Why the lease is ending unasked, once known: the host said so
    /// (`Ending`), or this gateway revoked the host; closes with `gone`.
    pub(crate) ending: watch::Receiver<Option<LeaseEndCause>>,
    /// Ends once the lease is no longer routed here.
    pub(crate) published: mpsc::Receiver<Published>,
}

/// One open connection to a host's environment.
pub(crate) struct HostLink {
    mcp_relay: Option<Arc<dyn nessa_sdk::application::agent_execution::providers::HarnessMcpRelay>>,
    /// Names the host in the log and the audit: its SSH destination, or a
    /// paired environment's `peer:<key>`.
    host: String,
    workspace: PathBuf,
    frames: mpsc::Sender<ToEnvironment>,
    routes: Arc<Mutex<Routes>>,
    audit: Arc<dyn EnvironmentAudit>,
    keep: Mutex<Box<dyn Send>>,
    /// How a published file's bytes are read off the host.
    artifacts: ArtifactTransport,
    /// Closes once the demux ends: the connection is over.
    ended: watch::Receiver<()>,
}

/// How long the writer may be idle before it says the gateway is there.
pub(crate) type Keepalive = Duration;

/// How long a connection is kept, and believed, without a frame either way.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Liveness {
    /// Past it with nothing to send, the writer sends a keepalive.
    pub(crate) keepalive: Keepalive,
    /// Past it with nothing read, keepalives included, the host is gone and
    /// the connection ends, as if its stream had.
    pub(crate) silence: Duration,
}

/// What a started harness's binding is handed.
pub(crate) struct StartedChannel {
    pub(crate) channel: u32,
    pub(crate) input: DuplexStream,
    pub(crate) output: DuplexStream,
}

/// Why no connection was opened.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Opening {
    /// Refused, with its reason.
    Refused(LeaseRefusal),
    /// The stream ended before anything was said on it: `ssh` reached
    /// nothing there, or no copy of this build is installed where it looked.
    NotServed,
}

impl From<LeaseRefusal> for Opening {
    fn from(refusal: LeaseRefusal) -> Self {
        Self::Refused(refusal)
    }
}

impl HostLink {
    /// Connect to `host` by running `command` there (the serve command of
    /// this build) and read its hello within `deadline`.
    ///
    /// # Errors
    /// The typed refusal: unreachable, another build, or busy; or the stream
    /// ending with nothing said on it.
    pub(crate) async fn open(
        host: &SshDestination,
        command: String,
        connector: &dyn LeaseConnector,
        audit: Arc<dyn EnvironmentAudit>,
        deadline: Duration,
        liveness: Liveness,
    ) -> Result<Arc<Self>, Opening> {
        let connection = connector.connect(host, command).map_err(|error| {
            tracing::warn!(host = host.as_str(), %error, "ssh could not be started");
            LeaseRefusal::EnvironmentUnreachable
        })?;
        Self::over(host.as_str(), connection, audit, deadline, liveness).await
    }

    /// Serve leases over `connection`, already open to the host `host`
    /// names, once its hello within `deadline` says it is this build's.
    ///
    /// # Errors
    /// As [`Self::open`], but for starting the connection.
    pub(super) fn mcp_relay(
        &self,
    ) -> Option<&dyn nessa_sdk::application::agent_execution::providers::HarnessMcpRelay> {
        self.mcp_relay.as_deref()
    }

    pub(crate) async fn over(
        host: &str,
        connection: LeaseConnection,
        audit: Arc<dyn EnvironmentAudit>,
        deadline: Duration,
        liveness: Liveness,
    ) -> Result<Arc<Self>, Opening> {
        let mut stream = FrameStream::new(connection.from_environment);
        let mut to_environment = connection.to_environment;
        let first = match tokio::time::timeout(deadline, stream.next()).await {
            Ok(Ok(Some(body))) => body,
            Ok(Ok(None)) => {
                tracing::warn!(
                    host,
                    "the host's stream ended before its environment said anything"
                );
                return Err(Opening::NotServed);
            }
            Ok(Err(_)) | Err(_) => {
                tracing::warn!(host, "the host's environment did not answer");
                return Err(LeaseRefusal::EnvironmentUnreachable.into());
            }
        };
        // A paired host asking for this build instead of serving.
        if let (Some(updates), Some(UpdateWanted { os, arch, libc })) =
            (&connection.updates, build_update::from_environment(&first))
        {
            updates
                .send_build(host, Platform { os, arch, libc }, &mut to_environment)
                .await;
            return Err(Opening::NotServed);
        }
        let hello = match read_hello(&first) {
            Some(hello) if hello.protocol == LEASE_PROTOCOL => hello,
            other => {
                let (build, protocol) = other.map(|hello| (hello.build, hello.protocol)).unzip();
                let event = EnvironmentEvent::VersionRefused {
                    host: host.into(),
                    build,
                    protocol,
                };
                if let Err(error) = audit.record(&event) {
                    tracing::error!(%error, "an environment's version refusal could not be recorded");
                }
                tracing::warn!(host, "the host speaks another lease protocol");
                // A paired host is told which one, so it can ask for it.
                if let Some(updates) = &connection.updates {
                    updates.refuse_version(&mut to_environment).await;
                }
                return Err(LeaseRefusal::EnvironmentVersionMismatch.into());
            }
        };
        let workspace = PathBuf::from(&hello.workspace);
        if !workspace.is_absolute() {
            return Err(
                Self::unavailable(host, &mut stream, audit.as_ref(), deadline)
                    .await
                    .into(),
            );
        }
        let connected = EnvironmentEvent::Connected {
            host: host.into(),
            workspace: hello.workspace.clone(),
        };
        if let Err(error) = audit.record(&connected) {
            tracing::error!(%error, "an environment connection could not be recorded; it is not used");
            return Err(LeaseRefusal::EnvironmentUnreachable.into());
        }
        let (frames, outgoing) = mpsc::channel(TO_HOST_QUEUE);
        tokio::spawn(write_frames(to_environment, outgoing, liveness.keepalive));
        let routes = Arc::new(Mutex::new(Routes::default()));
        let (ending, ended) = watch::channel(());
        let link = Arc::new(Self {
            host: host.into(),
            workspace,
            frames,
            routes,
            audit,
            mcp_relay: connection.mcp_relay,
            keep: Mutex::new(connection.keep),
            artifacts: connection.artifacts,
            ended,
        });
        tokio::spawn(demux(
            Arc::downgrade(&link),
            link.shared(),
            stream,
            liveness.silence,
            ending,
        ));
        Ok(link)
    }

    /// The refusal a hello without a workspace precedes.
    async fn unavailable<R: AsyncRead + Unpin>(
        host: &str,
        stream: &mut FrameStream<R>,
        audit: &dyn EnvironmentAudit,
        deadline: Duration,
    ) -> LeaseRefusal {
        let reason = match tokio::time::timeout(deadline, stream.next()).await {
            Ok(Ok(Some(body))) => match decode::<FromEnvironment>(&body) {
                Ok(FromEnvironment::Unavailable { reason }) => Some(reason),
                _ => None,
            },
            _ => None,
        };
        let host_name = host.to_owned();
        let (event, refusal) = match reason {
            Some(Unavailability::Busy) => (
                EnvironmentEvent::Busy { host: host_name },
                LeaseRefusal::EnvironmentBusy,
            ),
            Some(Unavailability::NotConfigured) | None => (
                EnvironmentEvent::NotConfigured { host: host_name },
                LeaseRefusal::EnvironmentUnreachable,
            ),
        };
        if let Err(error) = audit.record(&event) {
            tracing::error!(%error, "an environment's refusal could not be recorded");
        }
        refusal
    }

    fn shared(&self) -> Shared {
        Shared {
            host: self.host.clone(),
            routes: self.routes.clone(),
            audit: self.audit.clone(),
            frames: self.frames.downgrade(),
        }
    }

    fn routes(&self) -> MutexGuard<'_, Routes> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host's workspace, from its hello.
    pub(crate) fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// How a published file's bytes are read off the host.
    pub(crate) fn artifact_transport(&self) -> ArtifactTransport {
        self.artifacts.clone()
    }

    /// Whether a file can be written to the host over this connection: only
    /// over artifact channels beside its lease frames, never over the
    /// frames themselves, so never on a paired connection.
    pub(crate) fn writes_files(&self) -> bool {
        self.file_channels().is_some()
    }

    /// The artifact channels a file is written to the host over; `None` on
    /// a connection that has none.
    fn file_channels(&self) -> Option<Arc<dyn ArtifactChannels>> {
        match &self.artifacts {
            ArtifactTransport::Channels(channels) => Some(channels.clone()),
            ArtifactTransport::Frames => None,
        }
    }

    /// Whether the connection still carries leases: those on it may still
    /// be asked to end, revoked or not.
    pub(crate) fn is_open(&self) -> bool {
        !self.routes().closed && !self.frames.is_closed()
    }

    /// Whether a new lease may be asked on it: open, and not revoked.
    pub(crate) fn is_serving(&self) -> bool {
        self.is_open() && !self.routes().revoked
    }

    /// Whether the host was revoked on this connection.
    pub(crate) fn revoked(&self) -> bool {
        self.routes().revoked
    }

    /// Whether the host said it is stopping on this connection: it admits
    /// no new lease here, though those it holds still end through it.
    pub(crate) fn is_draining(&self) -> bool {
        *self.routes().draining.0.borrow()
    }

    /// What the host last said its machine is and how busy, on this
    /// connection; `None` before it says, or when it never does.
    pub(crate) fn machine(&self) -> Option<MachineReport> {
        self.routes().machine.0.borrow().clone()
    }

    /// Wakes each time the host reports its machine on this connection.
    pub(crate) fn machines(&self) -> watch::Receiver<Option<MachineReport>> {
        self.routes().machine.0.subscribe()
    }

    /// Answers once the host said it is stopping on this connection: from
    /// then on it is no longer offered new work.
    pub(crate) async fn drains(&self) {
        let mut draining = self.routes().draining.0.subscribe();
        if draining.wait_for(|draining| *draining).await.is_err() {
            std::future::pending::<()>().await;
        }
    }

    /// How many leases of an agent this connection holds now: granted, or
    /// ending, until each is let go. Commands are not counted, nor a lease
    /// still waiting for its grant: the pool counts that one as reserved
    /// until it is answered, so counting it here too would count it twice.
    /// A grant left unanswered is counted, as the host may hold it.
    pub(crate) fn agent_leases(&self) -> usize {
        self.routes()
            .leases
            .values()
            .filter(|route| !route.command && route.grant.is_none())
            .count()
    }

    /// Whether no lease of any kind is on this connection: none asked, held,
    /// ending, nor a command waiting for its end.
    pub(crate) fn holds_nothing(&self) -> bool {
        self.routes().leases.is_empty()
    }

    /// The host no longer holds a credential to serve this gateway. No lease
    /// is admitted on this connection any more, nor a harness started or a
    /// file asked for (a command's `Run` still goes, and the host answers
    /// it as never started); the revocation is recorded with every lease the host
    /// granted on it, and a lease it grants after, on its own; the host is told (`Revoked`), and ends
    /// each lease itself with that cause; then each harness lease's ending
    /// cause becomes revoked, unless the host already said why it is ending
    /// it, so whoever watches it ends it as the system's stop; and the
    /// connection is closed once every lease on it is let go — each End
    /// answered with the host's cleanup — or once `drain` passes from now,
    /// whichever comes first. A revocation that cannot be recorded still
    /// ends and closes everything, but settles none of those leases as
    /// confirmed. Asked again, or of a closed connection, nothing more
    /// happens. Answers whether this call revoked it.
    pub(crate) fn revoke(self: &Arc<Self>, drain: Duration) -> bool {
        let (granted, carried) = {
            let mut routes = self.routes();
            if routes.closed || routes.revoked {
                return false;
            }
            routes.revoked = true;
            // A lease whose grant is still being answered is not one the
            // host granted: it is not listed. Granted after all, it is
            // recorded revoked on its own when its grant comes, and ends
            // as revoked as the others do; refused, its route goes.
            let mut granted: Vec<String> = routes
                .leases
                .iter()
                .filter(|(_, route)| route.grant.is_none())
                .map(|(lease, _)| lease.clone())
                .collect();
            granted.sort();
            let carried: Vec<String> = routes.leases.keys().cloned().collect();
            (granted, carried)
        };
        let leases = granted;
        tracing::warn!(
            host = self.host.as_str(),
            leases = leases.len(),
            "the host no longer holds a credential to serve this gateway; its leases end and its connection closes"
        );
        let recorded = match self.audit.record(&EnvironmentEvent::Revoked {
            host: self.host.clone(),
            leases: leases.clone(),
        }) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(%error, "a host's revocation could not be recorded; its leases end unconfirmed");
                false
            }
        };
        if !recorded {
            // A grant still being answered is included: granted after all,
            // its end settles nothing either. Only leases still routed are
            // added, under the same lock: one refused or let go meanwhile
            // was already forgotten, and is not brought back.
            let mut routes = self.routes();
            let still: Vec<String> = carried
                .into_iter()
                .filter(|lease| routes.leases.contains_key(lease))
                .collect();
            routes.unaudited.extend(still);
        }
        let link = self.clone();
        tokio::spawn(async move {
            let deadline = tokio::time::Instant::now() + drain;
            // The host is told before any lease is seen ending as revoked,
            // so an End asked for one reaches it after `Revoked`, and it
            // records the revocation as that end's cause. A host reading
            // nothing is waited on no longer than the bound.
            let told =
                tokio::time::timeout_at(deadline, link.frames.send(ToEnvironment::Revoked)).await;
            if !matches!(told, Ok(Ok(()))) {
                tracing::warn!(
                    host = link.host.as_str(),
                    "a revoked host could not be told; its connection closes"
                );
            }
            let let_go = async {
                for mut lease in link.end_as_revoked() {
                    while lease.changed().await.is_ok() {}
                }
            };
            if tokio::time::timeout_at(deadline, let_go).await.is_err() {
                tracing::warn!(
                    host = link.host.as_str(),
                    "a revoked host's leases were not all let go in time; its connection closes"
                );
            }
            link.close();
        });
        true
    }

    /// Each harness lease on this revoked connection ends as revoked,
    /// unless the host already said why it is ending it: whoever watches it
    /// ends it as the system's stop. Answers each lease's watch, which
    /// closes once it is let go. No lease is admitted once revoked, so these
    /// are every lease it carries still.
    fn end_as_revoked(&self) -> Vec<watch::Receiver<()>> {
        self.routes()
            .leases
            .values()
            .map(|route| {
                if !route.command {
                    route.ending_cause.send_if_modified(|cause| {
                        let unknown = cause.is_none();
                        if unknown {
                            *cause = Some(LeaseEndCause::Revoked);
                        }
                        unknown
                    });
                }
                route.gone.subscribe()
            })
            .collect()
    }

    /// End the connection from this side: what keeps it open is let go, so
    /// its stream ends and every lease on it is lost, as when the host goes.
    pub(crate) fn close(&self) {
        let mut keep = self.keep.lock().unwrap_or_else(PoisonError::into_inner);
        drop(std::mem::replace(&mut *keep, Box::new(())));
    }

    /// Answers once the connection has ended and its leases are let go.
    pub(crate) async fn ended(&self) {
        let mut ended = self.ended.clone();
        // Never sent to: only the demux's end, dropping the sender, answers.
        while ended.changed().await.is_ok() {}
    }

    /// Ask the host to admit `lease` for `agent` in `sandbox`, waiting at
    /// most `deadline`.
    /// Granted, it answers the lease's watch: it closes once the lease is no
    /// longer routed here — ended, or the connection lost — and, being the
    /// lease's own from its grant, says so even when that happened before
    /// anyone looked.
    ///
    /// # Errors
    /// The typed refusal: the host's own, as the lease's; a connection lost
    /// or silent is unreachable.
    pub(crate) async fn grant(
        &self,
        lease: &str,
        agent: &str,
        sandbox: Sandbox,
        place: Option<BudgetPlace>,
        deadline: Duration,
    ) -> Result<Granted, LeaseRefusal> {
        let (offers, published) = mpsc::channel(MAX_ARTIFACTS_IN_FLIGHT);
        let frame = ToEnvironment::Grant {
            lease: lease.into(),
            agent: agent.into(),
            sandbox,
        };
        let Admitted {
            gone,
            ending,
            terms,
        } = self
            .admit(lease, frame, Some(offers), false, place, deadline)
            .await
            .map_err(lease_refusal)?;
        Ok(Granted {
            terms,
            gone,
            ending,
            published,
        })
    }

    /// Ask the host to admit command lease `lease` to run `argv` once, as
    /// [`Self::grant`] asks for a harness's, waiting at most `deadline`.
    ///
    /// # Errors
    /// The host's refusal, or no answer.
    pub(crate) async fn grant_command(
        &self,
        lease: &str,
        argv: Vec<String>,
        cwd: Option<String>,
        timeout_ms: u64,
        place: Option<BudgetPlace>,
        deadline: Duration,
    ) -> Result<AdmittedCommand, AdmissionRefused> {
        let frame = ToEnvironment::GrantCommand {
            lease: lease.into(),
            argv,
            cwd,
            timeout_ms,
        };
        // The same publish channel a harness lease gets. The route stays a
        // command route, so an `Ending` is not taken for it.
        let (offers, published) = mpsc::channel(MAX_ARTIFACTS_IN_FLIGHT);
        let admitted = self
            .admit(lease, frame, Some(offers), true, place, deadline)
            .await?;
        Ok(AdmittedCommand {
            terms: admitted.terms,
            gone: admitted.gone,
            published,
        })
    }

    /// Run a granted command lease's command and wait for its `Ran`. The
    /// caller bounds the wait; `None` when the lease is not held here, or
    /// the connection was lost before it answered. Its `End`, asked while it
    /// runs, stops it, and its `Ran` still answers.
    pub(crate) async fn run(&self, lease: &str) -> Option<CommandAnswer> {
        let (answer, answered) = oneshot::channel();
        {
            let mut routes = self.routes();
            let route = routes
                .leases
                .get_mut(lease)
                .filter(|route| !route.ending && route.ran.is_none())?;
            route.ran = Some(answer);
        }
        self.frames
            .send(ToEnvironment::Run {
                lease: lease.into(),
            })
            .await
            .ok()?;
        answered.await.ok()
    }

    /// Route `lease`, offering what it publishes to `published`, and send
    /// `frame`, the request to admit it, and wait at most `deadline` for the
    /// host's answer: the lease's watch and the terms it granted.
    async fn admit(
        &self,
        lease: &str,
        frame: ToEnvironment,
        published: Option<mpsc::Sender<Published>>,
        command: bool,
        place: Option<BudgetPlace>,
        deadline: Duration,
    ) -> Result<Admitted, AdmissionRefused> {
        let (answer, answered) = oneshot::channel();
        let (gone, ending) = {
            let mut routes = self.routes();
            if routes.closed || routes.revoked || routes.leases.contains_key(lease) {
                return Err(AdmissionRefused::Unreachable);
            }
            let (gone_sender, gone) = watch::channel(());
            let (ending_sender, ending) = watch::channel(None);
            routes.leases.insert(
                lease.into(),
                LeaseRoute {
                    grant: Some(answer),
                    ran: None,
                    command,
                    ended: None,
                    ending: false,
                    end_sent: false,
                    end_queued: false,
                    ran_answered: false,
                    pumps: Arc::new(RwLock::new(())),
                    channels: HashMap::new(),
                    next_channel: 1,
                    published,
                    ending_cause: ending_sender,
                    publishes: HashMap::new(),
                    fetches: HashMap::new(),
                    reads: HashMap::new(),
                    gone: gone_sender,
                    _place: place,
                },
            );
            (gone, ending)
        };
        let sent = self.frames.send(frame).await;
        if sent.is_err() {
            self.drop_admission(lease);
            return Err(AdmissionRefused::Unreachable);
        }
        match tokio::time::timeout(deadline, answered).await {
            Ok(Ok(Ok(terms))) => Ok(Admitted {
                gone,
                ending,
                terms,
            }),
            refused => {
                if refused.is_err() {
                    // No answer in time: the host may still admit it, so it
                    // is ended there rather than held for nobody, and its
                    // route, with its budget place, waits for that end.
                    self.end_unanswered(lease);
                } else {
                    self.drop_admission(lease);
                }
                Err(match refused {
                    // A host told it is revoked refuses what it is asked
                    // after: that is the revocation, not its owner's grant.
                    Ok(Ok(Err(_))) if self.revoked() => AdmissionRefused::Unreachable,
                    Ok(Ok(Err(reason))) => AdmissionRefused::Host(reason),
                    _ => AdmissionRefused::Unreachable,
                })
            }
        }
    }

    /// End a lease whose admission went unanswered: no hold will take it,
    /// and its route stays until the host's `Ended`, or the connection's
    /// loss, says it is not held there.
    fn end_unanswered(&self, lease: &str) {
        let mut routes = self.routes();
        routes.unaudited.remove(lease);
        if let Some(route) = routes.leases.get_mut(lease) {
            route.grant = None;
            if !route.end_sent {
                route.end_sent = true;
                let pumps = begin_end(route);
                deliver(
                    &self.frames,
                    Some(pumps),
                    ToEnvironment::End {
                        lease: lease.into(),
                    },
                    None,
                );
            }
        }
    }

    /// Let go of a lease that was never admitted: its route, and whether
    /// its transitions were recorded, as no hold will ever forget it.
    fn drop_admission(&self, lease: &str) {
        let mut routes = self.routes();
        routes.leases.remove(lease);
        routes.unaudited.remove(lease);
    }

    /// Start a harness under `lease` with the binding's `environment`.
    /// Answers at once.
    ///
    /// # Errors
    /// The connection is lost, the lease not held, or the host's queue full.
    pub(crate) fn start(
        &self,
        lease: &str,
        environment: BTreeMap<String, String>,
    ) -> Result<StartedChannel, StartError> {
        let (output_sender, output_receiver) = mpsc::channel(OUTPUT_QUEUE);
        let (stop, stop_asked) = oneshot::channel();
        let channel = {
            let mut routes = self.routes();
            // Once revoked, the host ends every lease: nothing starts.
            if routes.closed || routes.revoked {
                return Err(StartError::Closed);
            }
            let Some(route) = routes.leases.get_mut(lease).filter(|route| !route.ending) else {
                return Err(StartError::Closed);
            };
            let running = route
                .pumps
                .clone()
                .try_read_owned()
                .map_err(|_| StartError::Closed)?;
            let channel = route.next_channel;
            route.next_channel = channel.checked_add(1).ok_or(StartError::Closed)?;
            route.channels.insert(
                channel,
                ChannelRoute {
                    output: Some(output_sender),
                    stop: Some(stop),
                    asked: None,
                    stopped: None,
                    cleanup: None,
                    start_failed: false,
                },
            );
            let start = ToEnvironment::Start {
                lease: lease.into(),
                channel,
                environment,
            };
            // Larger than a frame carries: refused here, before it is queued
            // for every lease's connection.
            if encode(&start).is_err() {
                route.channels.remove(&channel);
                return Err(StartError::TooLarge);
            }
            if let Err(error) = self.frames.try_send(start) {
                route.channels.remove(&channel);
                return Err(match error {
                    mpsc::error::TrySendError::Full(_) => StartError::Busy,
                    mpsc::error::TrySendError::Closed(_) => StartError::Closed,
                });
            }
            (channel, running)
        };
        let (channel, running) = channel;
        let (output, pump_out) = tokio::io::duplex(PIPE_BYTES);
        let (input, pump_in) = tokio::io::duplex(PIPE_BYTES);
        tokio::spawn(pump_output(output_receiver, pump_out));
        tokio::spawn(pump_input(
            pump_in,
            self.frames.clone(),
            lease.to_owned(),
            channel,
            stop_asked,
            running,
        ));
        Ok(StartedChannel {
            channel,
            input,
            output,
        })
    }

    /// Stop one harness and wait for the host's evidence: `None` when it
    /// cannot be had — the connection lost, or no answer in time. The Stop
    /// goes after the harness's input; a harness already asked to stop, or
    /// under a lease whose end is queued, is answered by that stop or that
    /// end; one under a lease the host already ended, by what the host
    /// recorded of it.
    pub(crate) async fn stop(
        &self,
        lease: &str,
        channel: u32,
        grace: Duration,
        kill: Duration,
    ) -> Option<Cleanup> {
        let request = StopRequest {
            grace_ms: millis(grace),
            kill_ms: millis(kill),
        };
        let waiting = {
            let mut routes = self.routes();
            if routes.closed {
                return None;
            }
            match routes.leases.get_mut(lease) {
                None => None,
                Some(route) => {
                    let ending = route.ending;
                    let route = route.channels.get_mut(&channel)?;
                    if route.start_failed {
                        return Some(Cleanup::NotHeld);
                    }
                    if let Some(cleanup) = route.cleanup {
                        return Some(cleanup);
                    }
                    let (answer, answered) = oneshot::channel();
                    route.stopped = Some(answer);
                    let mut bound = request.bound();
                    match (route.asked, route.stop.take()) {
                        // Asked already: its own Stop is waited for too.
                        (Some(asked), _) => bound = bound.max(asked.bound()),
                        (None, Some(stop)) if !ending => {
                            route.asked = Some(request);
                            // A pump that is gone has ended with the
                            // connection or the lease, which answer this.
                            let _ = stop.send(request);
                        }
                        (None, _) => {}
                    }
                    Some((answered, bound))
                }
            }
        };
        let answered = match waiting {
            Some((answered, bound)) => tokio::time::timeout(bound, answered).await.ok()?.ok(),
            // No longer held here: the host ended it, and its record of
            // that end covers every harness under it.
            None => tokio::time::timeout(request.bound(), self.account(lease))
                .await
                .ok()
                .flatten(),
        }?;
        // A transition of its lease that went unrecorded settles nothing.
        Some(match self.unaudited(lease) {
            true => Cleanup::Uncertain,
            false => answered,
        })
    }

    /// Whether a transition of `lease` could not be recorded in the audit.
    pub(crate) fn unaudited(&self, lease: &str) -> bool {
        self.routes().unaudited.contains(lease)
    }

    /// Forget whether `lease`'s transitions were recorded: its hold is gone.
    pub(crate) fn forget(&self, lease: &str) {
        self.routes().unaudited.remove(lease);
    }

    /// Stop a harness whose binding let go of it without a stop: asked, not
    /// waited for, and always sent, after its input.
    pub(crate) fn abandon(&self, lease: &str, channel: u32) {
        let mut routes = self.routes();
        let Some(route) = routes.leases.get_mut(lease).filter(|route| !route.ending) else {
            return;
        };
        if let Some(route) = route.channels.get_mut(&channel) {
            if let Some(stop) = route.stop.take() {
                route.asked = Some(StopRequest::ABANDONED);
                let _ = stop.send(StopRequest::ABANDONED);
            }
        }
    }

    /// End `lease` and wait for the host's evidence; `None` when it cannot be
    /// had. The caller bounds the wait. Its End goes after every frame its
    /// harnesses' pumps still send. Cancelled before its End is queued, it
    /// leaves the End to its hold's drop; once queued, nothing more is sent
    /// for it.
    pub(crate) async fn end(&self, lease: &str) -> Option<Cleanup> {
        let asked = {
            let mut routes = self.routes();
            if routes.closed {
                return None;
            }
            routes.leases.get_mut(lease).map(|route| {
                let (answer, answered) = oneshot::channel();
                route.ended = Some(answer);
                (answered, begin_end(route))
            })
        };
        // Not held on this connection: what the host recorded of it.
        let Some((answered, pumps)) = asked else {
            return self.account(lease).await;
        };
        // Every pump under it done, then room, then the End and its record
        // together: cancelled at either wait, nothing was sent and nothing
        // says it was.
        let _done = pumps.write_owned().await;
        let permit = self.frames.reserve().await.ok()?;
        {
            let mut routes = self.routes();
            let route = routes.leases.get_mut(lease)?;
            if !route.end_sent {
                route.end_sent = true;
                permit.send(ToEnvironment::End {
                    lease: lease.into(),
                });
            }
        }
        answered.await.ok()
    }

    /// End `lease` without waiting, unless its End is already queued: its
    /// hold was let go. Always sent, after every frame its pumps still send.
    pub(crate) fn abandon_lease(&self, lease: &str) {
        let pumps = {
            let mut routes = self.routes();
            let Some(route) = routes.leases.get_mut(lease).filter(|route| !route.end_sent) else {
                return;
            };
            route.end_sent = true;
            begin_end(route)
        };
        queue_end(
            self.frames.clone(),
            pumps,
            lease.into(),
            self.routes.clone(),
        );
    }

    /// Ask the host to publish the file at `path` under `lease` for
    /// `request`, and wait for it to be offered on the lease's offers, or
    /// for why not. A lease no longer held here, or one whose End is asked,
    /// is answered as ended; the connection's loss, or the lease's route
    /// ending, before the host answers, as unanswered, so the wait never
    /// outlives the route.
    pub(crate) async fn publish(
        &self,
        lease: &str,
        request: String,
        path: String,
        media_type: Option<String>,
    ) -> Result<(), PublishFailure> {
        let frame = ToEnvironment::Publish {
            lease: lease.into(),
            request: request.clone(),
            path,
            media_type,
        };
        // Larger than a frame carries: never queued, so never answered.
        if encode(&frame).is_err() {
            return Err(PublishFailure::NotPublished(PublishRefusal::Invalid));
        }
        let answered = {
            let mut routes = self.routes();
            if routes.closed {
                return Err(PublishFailure::Unanswered);
            }
            // Once revoked, the host is ending every lease on it.
            if routes.revoked {
                return Err(PublishFailure::NotPublished(PublishRefusal::LeaseEnded));
            }
            let Some(route) = routes
                .leases
                .get_mut(lease)
                .filter(|route| !route.ending && route.published.is_some())
            else {
                return Err(PublishFailure::NotPublished(PublishRefusal::LeaseEnded));
            };
            // One ask per request: the caller holds each until answered.
            if route.publishes.contains_key(&request) {
                return Err(PublishFailure::Unanswered);
            }
            let (answer, answered) = oneshot::channel();
            route.publishes.insert(request.clone(), answer);
            answered
        };
        if self.frames.send(frame).await.is_err() {
            if let Some(route) = self.routes().leases.get_mut(lease) {
                route.publishes.remove(&request);
            }
            return Err(PublishFailure::Unanswered);
        }
        answered.await.unwrap_or(Err(PublishFailure::Unanswered))
    }

    /// Ask the host to place `ask`'s file under `lease`, write its bytes to
    /// the part the host names over an artifact channel, and wait for what
    /// became of it. A lease no longer held here, or one whose End is
    /// asked, is answered as ended; the connection's loss, or the lease's
    /// route ending, before the host's last word, as unanswered once any
    /// byte may have been delivered, so the wait never outlives the route.
    /// Dropped while its bytes are written, the host is told they will not
    /// all come.
    pub(crate) async fn fetch(
        &self,
        lease: &str,
        ask: FetchAsk,
        gone: watch::Receiver<()>,
    ) -> Result<FetchPlaced, FetchFailure> {
        let FetchAsk {
            request,
            destination,
            file,
            source,
            delivered: said_delivered,
        } = ask;
        // Bytes that could never be written are never asked for.
        let Some(channels) = self.file_channels() else {
            return Err(FetchFailure::Refused(FetchRefusal::Unavailable));
        };
        let (request, mut answers) = {
            let mut routes = self.routes();
            if routes.closed {
                return Err(FetchFailure::Unanswered);
            }
            if !routes
                .leases
                .get(lease)
                .is_some_and(|route| !route.ending && route.published.is_some())
            {
                return Err(FetchFailure::Refused(FetchRefusal::LeaseEnded));
            }
            // Named on the wire by its call and the connection's count, so
            // an ask never shares a name with one before it, even one given
            // up whose host has not said its last word: what the host says
            // of each reaches only the ask that named it.
            let request = format!("{request}#{}", routes.fetches_asked);
            routes.fetches_asked += 1;
            let Some(route) = routes.leases.get_mut(lease) else {
                return Err(FetchFailure::Refused(FetchRefusal::LeaseEnded));
            };
            // The host says at most two things of one fetch.
            let (answer, answers) = mpsc::channel(2);
            route.fetches.insert(request.clone(), answer);
            (request, answers)
        };
        // From here, however the call ends, its wait is let go; and from
        // the moment its Fetch is queued until the host's last word on it,
        // or every byte is delivered, it is told they will not come. So a
        // call given up while an acceptance is on its way, or already
        // waiting unread, still lets the host's part go.
        let mut fetching = Fetching {
            routes: self.routes.clone(),
            frames: self.frames.clone(),
            lease: lease.to_owned(),
            request: request.clone(),
            undelivered: false,
        };
        let frame = ToEnvironment::Fetch {
            lease: lease.into(),
            request: request.clone(),
            destination,
            digest: file.digest().to_hex(),
            media_type: file.media_type().as_str().to_owned(),
            size: file.size(),
        };
        // Larger than a frame carries: never queued, so never answered.
        if encode(&frame).is_err() {
            return Err(FetchFailure::Refused(FetchRefusal::Invalid));
        }
        if self.frames.send(frame).await.is_err() {
            return Err(FetchFailure::Unanswered);
        }
        fetching.undelivered = true;
        let part = match answers.recv().await {
            Some(FetchAnswer::Accepted(part)) => part,
            last => {
                fetching.undelivered = false;
                return match last {
                    Some(FetchAnswer::Refused(reason)) => Err(FetchFailure::Refused(reason)),
                    Some(FetchAnswer::Fetched(outcome)) => fetched(outcome),
                    // The route ended with the lease or the connection:
                    // nothing of it is held there any more.
                    _ => Err(FetchFailure::Unanswered),
                };
            }
        };
        // The host's last word first: ready with a failed write, it is the
        // answer that holds.
        let written = tokio::select! {
            biased;
            // The host's last word while the bytes are written: the lease
            // ended there, and nothing is placed.
            answer = answers.recv() => {
                fetching.undelivered = false;
                return match answer {
                    Some(FetchAnswer::Fetched(outcome)) => fetched(outcome),
                    Some(FetchAnswer::Refused(reason)) => Err(FetchFailure::Refused(reason)),
                    _ => Err(FetchFailure::Refused(FetchRefusal::LeaseEnded)),
                };
            }
            written = write_part(channels.as_ref(), &part, file.size(), source, gone) => written,
        };
        if let Err(failure) = written {
            // Told on the way out: nothing more is written.
            return Err(match failure {
                WriteFailure::NotHeld => FetchFailure::NotHeld,
                WriteFailure::SourceUnavailable => FetchFailure::SourceUnavailable,
                WriteFailure::ChannelUnavailable => FetchFailure::ChannelUnavailable,
                WriteFailure::LeaseEnded => FetchFailure::Refused(FetchRefusal::LeaseEnded),
            });
        }
        let delivered = ToEnvironment::Delivered {
            lease: lease.into(),
            request,
        };
        // Room for it first, then marked, then queued with nothing awaited
        // between: marked exactly when the host may place it, so a call
        // given up while the queue is full keeps no bytes counted.
        // The host's last word while waiting for room is still its answer.
        let permit = tokio::select! {
            biased;
            answer = answers.recv() => {
                fetching.undelivered = false;
                return match answer {
                    Some(FetchAnswer::Fetched(outcome)) => fetched(outcome),
                    Some(FetchAnswer::Refused(reason)) => Err(FetchFailure::Refused(reason)),
                    _ => Err(FetchFailure::Refused(FetchRefusal::LeaseEnded)),
                };
            }
            permit = self.frames.reserve() => match permit {
                Ok(permit) => permit,
                Err(_) => return Err(FetchFailure::Unanswered),
            },
        };
        said_delivered.store(true, std::sync::atomic::Ordering::SeqCst);
        permit.send(delivered);
        fetching.undelivered = false;
        match answers.recv().await {
            Some(FetchAnswer::Fetched(outcome)) => fetched(outcome),
            Some(FetchAnswer::Refused(reason)) => Err(FetchFailure::Refused(reason)),
            // Delivered, and no word on it: placed or not, it is not known.
            _ => Err(FetchFailure::Unanswered),
        }
    }

    /// Tell the host what became of a file it published under `lease`,
    /// unless the lease is no longer held here: done once the answer is
    /// queued. `unanswered` is let go only then, so the lease's End, which
    /// waits for it, always goes after: the host never answers as ended a
    /// file this gateway kept.
    pub(crate) fn collected(
        &self,
        lease: &str,
        artifact: u32,
        outcome: Collection,
        unanswered: OwnedRwLockReadGuard<()>,
    ) -> ArtifactAnswering {
        let held = self.routes().leases.contains_key(lease);
        let frames = self.frames.clone();
        let frame = ToEnvironment::Collected {
            lease: lease.into(),
            artifact,
            outcome,
        };
        Box::pin(async move {
            if held {
                let _ = frames.send(frame).await;
            }
            drop(unanswered);
        })
    }

    /// Ask the host for `length` bytes of the file it published under
    /// `lease` as `artifact`, from `offset`, over this connection's own
    /// frames: where its answer arrives. `None` when it is not asked: the
    /// lease is not held here, the connection is gone, or that range is
    /// already asked. Whether the file may still be read is the host's to
    /// say: it answers only for a staged copy still waiting for its answer,
    /// so once revoked, which ends every lease there, a read is answered
    /// not staged. The answer's wait ends with the
    /// lease's route, so it never outlives the lease here. Only for a
    /// connection whose transport is [`ArtifactTransport::Frames`].
    pub(crate) async fn read_artifact(
        &self,
        lease: &str,
        artifact: u32,
        offset: u64,
        length: u32,
    ) -> Option<oneshot::Receiver<ArtifactReadAnswer>> {
        let answered = {
            let mut routes = self.routes();
            if routes.closed {
                return None;
            }
            let route = routes.leases.get_mut(lease)?;
            let Entry::Vacant(waiting) = route.reads.entry((artifact, offset)) else {
                return None;
            };
            let (answer, answered) = oneshot::channel();
            waiting.insert(answer);
            answered
        };
        let frame = ToEnvironment::ReadArtifact {
            lease: lease.into(),
            artifact,
            offset,
            length,
        };
        if self.frames.send(frame).await.is_err() {
            if let Some(route) = self.routes().leases.get_mut(lease) {
                route.reads.remove(&(artifact, offset));
            }
            return None;
        }
        Some(answered)
    }

    /// What the host recorded of `lease`; `None` when it cannot be had. The
    /// caller bounds the wait.
    pub(crate) async fn account(&self, lease: &str) -> Option<Cleanup> {
        let answered = {
            let mut routes = self.routes();
            if routes.closed {
                return None;
            }
            let (answer, answered) = oneshot::channel();
            routes
                .accounts
                .entry(lease.into())
                .or_default()
                .push(answer);
            answered
        };
        self.frames
            .send(ToEnvironment::Account {
                lease: lease.into(),
            })
            .await
            .ok()?;
        answered.await.ok()
    }
}

/// Why a harness could not be asked to start.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum StartError {
    /// The connection is lost or the lease no longer held.
    Closed,
    /// The host's queue is full.
    Busy,
    /// The launch's variables do not fit in one frame.
    TooLarge,
}

/// Mark `route`'s End asked: each harness not already asked to stop has its
/// pump closed without a Stop, as the End stops it. Answers what the End
/// waits on before it is queued.
fn begin_end(route: &mut LeaseRoute) -> Arc<RwLock<()>> {
    route.ending = true;
    for channel in route.channels.values_mut() {
        if channel.asked.is_none() {
            channel.stop = None;
        }
    }
    route.pumps.clone()
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// What the demux needs of the link without keeping it alive.
struct Shared {
    host: String,
    routes: Arc<Mutex<Routes>>,
    audit: Arc<dyn EnvironmentAudit>,
    /// The host's frames, for an answer the demux gives itself; weak, so
    /// the demux never keeps the connection's writer open.
    frames: mpsc::WeakSender<ToEnvironment>,
}

impl Shared {
    fn routes(&self) -> MutexGuard<'_, Routes> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// Whether `event` was recorded.
    fn record(&self, event: EnvironmentEvent) -> bool {
        match self.audit.record(&event) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(%error, ?event, "an environment event could not be recorded");
                false
            }
        }
    }

    fn dropped(&self, lease: Option<&str>, channel: Option<u32>, frame: &str) {
        tracing::warn!(
            host = self.host.as_str(),
            lease,
            channel,
            frame,
            "a lease frame naming nothing this gateway holds was dropped"
        );
        let record = {
            let mut routes = self.routes();
            routes.drops = routes.drops.saturating_add(1);
            routes.drops <= MAX_RECORDED_DROPS
        };
        if record {
            // Logged where it fails, and nothing more: a drop is no
            // transition of any lease.
            let _ = self.record(EnvironmentEvent::FrameDropped {
                host: self.host.as_str().into(),
                lease: lease.map(str::to_owned),
                channel,
                frame: frame.into(),
            });
        }
    }

    /// Route one frame from the host. Nothing here waits.
    fn route(&self, frame: FromEnvironment) {
        let name = frame_name(&frame);
        let lease = frame.lease().map(str::to_owned);
        let channel = frame_channel(&frame);
        let routed = {
            let mut routes = self.routes();
            route_frame(&mut routes, frame)
        };
        match routed {
            Routed::Done => {}
            Routed::Answer(frame, unanswered) => {
                if let Some(frames) = self.frames.upgrade() {
                    deliver(&frames, None, frame, unanswered);
                }
            }
            Routed::Dropped => self.dropped(lease.as_deref(), channel, name),
            Routed::Overflow { lease, channel } => {
                // The harness is stopped either way; unrecorded, its lease
                // settles nothing as confirmed. Set before the demux reads
                // the host's next frame, so before its Stopped is answered.
                let recorded = self.record(EnvironmentEvent::OutputOverflow {
                    host: self.host.as_str().into(),
                    lease: lease.clone(),
                    channel,
                });
                if !recorded {
                    self.routes().unaudited.insert(lease);
                }
            }
            Routed::EndAfterPumps { lease, pumps } => {
                if let Some(frames) = self.frames.upgrade() {
                    queue_end(frames, pumps, lease, self.routes.clone());
                }
            }
            Routed::ReleaseCommand { lease } => {
                let routes = self.routes.clone();
                tokio::spawn(async move {
                    release_command_route(routes, lease).await;
                });
            }
            Routed::GrantedRevoked(lease) => {
                // Granted after the revocation was recorded, so not named
                // in it: it is recorded on its own, as every lease the host
                // granted is, before the host's next frame (its end) is
                // read. Unrecorded, it settles nothing as confirmed, unless
                // it was let go meanwhile.
                let recorded = self.record(EnvironmentEvent::Revoked {
                    host: self.host.as_str().into(),
                    leases: vec![lease.clone()],
                });
                if !recorded {
                    let mut routes = self.routes();
                    if routes.leases.contains_key(&lease) {
                        routes.unaudited.insert(lease);
                    }
                }
            }
        }
    }
}

/// A lease the host admitted: its watches, and the terms it granted.
struct Admitted {
    gone: watch::Receiver<()>,
    ending: watch::Receiver<Option<LeaseEndCause>>,
    terms: GrantedTerms,
}

enum Routed {
    Done,
    /// Routed, and answered at once by this gateway; the fence it was
    /// taken under, if any, is held until the answer is queued.
    Answer(ToEnvironment, Option<OwnedRwLockReadGuard<()>>),
    Dropped,
    Overflow {
        lease: String,
        channel: u32,
    },
    /// The host granted a lease whose admission was still pending when its
    /// connection was revoked.
    GrantedRevoked(String),
    /// End this command lease once `pumps` can be held for writing, so
    /// after every file already offered is answered. The route stays until
    /// the host's `Ended`, or for [`COMMAND_STOP_WAIT`] after that End is
    /// queued.
    EndAfterPumps {
        lease: String,
        pumps: Arc<RwLock<()>>,
    },
    /// `Ran` was already said and End is already queued. Release the
    /// command route if `Ended` does not arrive within [`COMMAND_STOP_WAIT`].
    ReleaseCommand {
        lease: String,
    },
}

fn route_frame(routes: &mut Routes, frame: FromEnvironment) -> Routed {
    match frame {
        FromEnvironment::Hello { .. } | FromEnvironment::Unavailable { .. } => Routed::Dropped,
        // Only proof the host is still there: its arrival already reset the
        // demux's silence bound, and nothing waits on it.
        FromEnvironment::Keepalive => Routed::Done,
        // Shown, never acted on; one out of its bounds is not kept.
        FromEnvironment::Machine { report } if report.is_bounded() => {
            routes.machine.0.send_replace(Some(report));
            Routed::Done
        }
        FromEnvironment::Machine { .. } => Routed::Dropped,
        FromEnvironment::Draining => {
            routes.draining.0.send_replace(true);
            Routed::Done
        }
        FromEnvironment::Accounted { lease, cleanup } => match routes.accounts.remove(&lease) {
            Some(waiters) => {
                for waiter in waiters {
                    let _ = waiter.send(cleanup);
                }
                Routed::Done
            }
            None => Routed::Dropped,
        },
        FromEnvironment::Granted { lease, terms } => {
            match routes
                .leases
                .get_mut(&lease)
                .and_then(|route| route.grant.take())
            {
                Some(waiter) => {
                    let _ = waiter.send(Ok(terms));
                    if routes.revoked {
                        Routed::GrantedRevoked(lease)
                    } else {
                        Routed::Done
                    }
                }
                None => Routed::Dropped,
            }
        }
        FromEnvironment::Refused { lease, reason } => {
            match routes
                .leases
                .get_mut(&lease)
                .and_then(|route| route.grant.take())
            {
                Some(waiter) => {
                    routes.leases.remove(&lease);
                    if reason == GrantRefusal::Draining {
                        routes.draining.0.send_replace(true);
                    }
                    let _ = waiter.send(Err(reason));
                    Routed::Done
                }
                None => Routed::Dropped,
            }
        }
        FromEnvironment::Published {
            lease,
            artifact,
            request,
            file,
        } => {
            let Some(route) = routes.leases.get_mut(&lease) else {
                return Routed::Dropped;
            };
            // The publish it answers learns what came of the offer: taken
            // to be kept, or refused here before anyone could take it.
            let waiter = request
                .as_ref()
                .and_then(|request| route.publishes.remove(request));
            let offered = offer_published(route, artifact, file, request);
            if let Some(waiter) = waiter {
                let _ = waiter.send(
                    offered
                        .as_ref()
                        .copied()
                        .map_err(|(reason, _)| PublishFailure::Refused(*reason)),
                );
            }
            match offered {
                Ok(()) => Routed::Done,
                Err((reason, unanswered)) => Routed::Answer(
                    ToEnvironment::Collected {
                        lease,
                        artifact,
                        outcome: Collection::Refused { reason },
                    },
                    unanswered,
                ),
            }
        }
        FromEnvironment::ArtifactRead {
            lease,
            artifact,
            offset,
            data,
        } => answer_read(routes, &lease, artifact, offset, Ok(data.0)),
        FromEnvironment::ArtifactNotRead {
            lease,
            artifact,
            offset,
            reason,
        } => answer_read(routes, &lease, artifact, offset, Err(reason)),
        // The host is ending a lease unasked: said to whoever watches it,
        // who asks its End, which the host answers with what that took.
        // A command's lease is bounded by its timeout, never ended so. A
        // lease already ending as revoked keeps that cause.
        FromEnvironment::Ending { lease, cause } => match routes.leases.get(&lease) {
            Some(route) if !route.command => {
                let cause = match cause {
                    EndingCause::Expired => LeaseEndCause::Expired,
                    // The host stopping is a system stop, as the gateway
                    // retiring is: the lease ends with what that took.
                    EndingCause::Drained => LeaseEndCause::Stopped,
                };
                route.ending_cause.send_if_modified(|known| {
                    let unknown = known.is_none();
                    if unknown {
                        *known = Some(cause);
                    }
                    unknown
                });
                Routed::Done
            }
            _ => Routed::Dropped,
        },
        FromEnvironment::NotPublished {
            lease,
            request,
            reason,
        } => match routes
            .leases
            .get_mut(&lease)
            .and_then(|route| route.publishes.remove(&request))
        {
            Some(waiter) => {
                let _ = waiter.send(Err(PublishFailure::NotPublished(reason)));
                Routed::Done
            }
            None => Routed::Dropped,
        },
        FromEnvironment::FetchAccepted {
            lease,
            request,
            path,
        } => fetch_answer(routes, &lease, &request, FetchAnswer::Accepted(path), false),
        FromEnvironment::FetchRefused {
            lease,
            request,
            reason,
        } => fetch_answer(routes, &lease, &request, FetchAnswer::Refused(reason), true),
        FromEnvironment::Fetched {
            lease,
            request,
            outcome,
        } => fetch_answer(
            routes,
            &lease,
            &request,
            FetchAnswer::Fetched(outcome),
            true,
        ),
        FromEnvironment::Ended { lease, cleanup } => match routes.leases.remove(&lease) {
            Some(mut route) => {
                if let Some(waiter) = route.ended.take() {
                    let _ = waiter.send(cleanup);
                }
                // The host stopped every harness under the lease before it
                // ended it: a stop still waiting is answered by that end.
                for channel in route.channels.into_values() {
                    if let Some(waiter) = channel.stopped {
                        let _ = waiter.send(cleanup);
                    }
                }
                Routed::Done
            }
            None => Routed::Dropped,
        },
        FromEnvironment::Ran {
            lease,
            end,
            stdout: Data(stdout),
            stderr: Data(stderr),
            dropped_bytes,
            cleanup,
        } => {
            let Some(route) = routes.leases.get_mut(&lease) else {
                return Routed::Dropped;
            };
            let Some(waiter) = route.ran.take() else {
                return Routed::Dropped;
            };
            let _ = waiter.send(CommandAnswer {
                end,
                stdout,
                stderr,
                dropped_bytes,
                cleanup,
            });
            // Files already offered are still read. The stream itself ends
            // with `Ran`, so collecting them does not wait for `Ended`.
            route.published.take();
            route.ran_answered = true;
            // Its `Ran` is the command's finish. An End already asked is
            // answered by the `Ended` that follows. Otherwise End is asked
            // now, after every file already offered is answered. The route
            // stays through that copy. Once End is queued, `Ended` has the
            // stop wait; then the route is released, so a host that stays
            // up without answering it does not keep the place.
            if !route.end_sent && route.ended.is_none() {
                route.end_sent = true;
                let pumps = begin_end(route);
                return Routed::EndAfterPumps { lease, pumps };
            }
            if route.end_queued {
                return Routed::ReleaseCommand { lease };
            }
            Routed::Done
        }
        FromEnvironment::StartFailed { lease, channel, .. } => {
            match channel_route(routes, &lease, channel) {
                Some(route) => {
                    route.start_failed = true;
                    route.output = None;
                    Routed::Done
                }
                None => Routed::Dropped,
            }
        }
        FromEnvironment::Output {
            lease,
            channel,
            data: Data(bytes),
        } => {
            // Once its lease's End is queued, that End stops the harness.
            let ending = routes.leases.get(&lease).is_some_and(|route| route.ending);
            match channel_route(routes, &lease, channel) {
                Some(route) => match &route.output {
                    Some(output) => match output.try_send(bytes) {
                        Ok(()) => Routed::Done,
                        // The binding let go of its output: nothing reads it.
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            route.output = None;
                            Routed::Done
                        }
                        // A harness whose output is no longer read is stopped,
                        // never left running: its pump sends the Stop.
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            route.output = None;
                            if let Some(stop) = route.stop.take().filter(|_| !ending) {
                                route.asked = Some(StopRequest::ABANDONED);
                                let _ = stop.send(StopRequest::ABANDONED);
                            }
                            Routed::Overflow { lease, channel }
                        }
                    },
                    None => Routed::Dropped,
                },
                None => Routed::Dropped,
            }
        }
        FromEnvironment::OutputClosed { lease, channel } => {
            match channel_route(routes, &lease, channel) {
                Some(route) => {
                    route.output = None;
                    Routed::Done
                }
                None => Routed::Dropped,
            }
        }
        FromEnvironment::Stopped {
            lease,
            channel,
            cleanup,
        } => {
            let Some(route) = routes.leases.get_mut(&lease) else {
                return Routed::Dropped;
            };
            // Kept until the lease ends: a channel number is never used
            // again under it, and a later stop is answered from it.
            match route.channels.get_mut(&channel) {
                Some(channel) => {
                    // Stopped, asked or not (the host stops a harness that
                    // stopped reading its input): its output ends, and its
                    // pump, never to be asked for a Stop now, ends too, so
                    // its binding's writes fail rather than go nowhere.
                    channel.output = None;
                    channel.stop = None;
                    channel.cleanup = Some(cleanup);
                    if let Some(waiter) = channel.stopped.take() {
                        let _ = waiter.send(cleanup);
                    }
                    Routed::Done
                }
                None => Routed::Dropped,
            }
        }
    }
}

/// Offer a file the host published under `route` to whoever takes them,
/// or say why it is refused here, with the fence a refusal keeps until its
/// answer is queued.
fn offer_published(
    route: &mut LeaseRoute,
    artifact: u32,
    file: StagedArtifact,
    request: Option<String>,
) -> Result<(), (CollectionRefusal, Option<OwnedRwLockReadGuard<()>>)> {
    // Once its End is asked, nothing more is taken under it.
    if route.ending {
        return Err((CollectionRefusal::LeaseEnded, None));
    }
    let Ok(unanswered) = route.pumps.clone().try_read_owned() else {
        return Err((CollectionRefusal::LeaseEnded, None));
    };
    let offer = Published {
        artifact,
        file,
        request,
        unanswered,
    };
    // A refusal keeps the offer's fence until it is queued, so an End
    // asked meanwhile still goes after it.
    let sent = match route.published.as_ref() {
        Some(offers) => offers.try_send(offer).map_err(|error| match error {
            mpsc::error::TrySendError::Full(offer) => (true, offer.unanswered),
            mpsc::error::TrySendError::Closed(offer) => (false, offer.unanswered),
        }),
        None => Err((false, offer.unanswered)),
    };
    match sent {
        Ok(()) => Ok(()),
        // The host keeps at most as many in flight as there is room for:
        // one past it is not this gateway's to take.
        Err((true, unanswered)) => Err((CollectionRefusal::BudgetExceeded, Some(unanswered))),
        Err((false, unanswered)) => {
            route.published = None;
            Err((CollectionRefusal::ChannelUnavailable, Some(unanswered)))
        }
    }
}

/// Hand `answer` to the fetch `request` under `lease` that waits for it;
/// a `last` word lets the wait go. Dropped when nothing waits for it: a
/// call given up after its Fetch was queued already told the host
/// `Undelivered` ([`Fetching`]), so a late acceptance needs no answer.
fn fetch_answer(
    routes: &mut Routes,
    lease: &str,
    request: &str,
    answer: FetchAnswer,
    last: bool,
) -> Routed {
    let Some(route) = routes.leases.get_mut(lease) else {
        return Routed::Dropped;
    };
    let waiter = match last {
        true => route.fetches.remove(request),
        false => route.fetches.get(request).cloned(),
    };
    match waiter.map(|waiter| waiter.try_send(answer)) {
        Some(Ok(())) => Routed::Done,
        // Nobody waits for it, or the host said more than a fetch has.
        _ => Routed::Dropped,
    }
}

/// What the host's last word on a fetch says to its caller.
fn fetched(outcome: FetchOutcome) -> Result<FetchPlaced, FetchFailure> {
    match outcome {
        FetchOutcome::Placed { path } => Ok(FetchPlaced::Placed { path }),
        FetchOutcome::AlreadyPlaced { path } => Ok(FetchPlaced::AlreadyPlaced { path }),
        FetchOutcome::Refused { reason } => Err(FetchFailure::Refused(reason)),
    }
}

/// One fetch's wait on its route, let go however the fetch ends; while
/// `undelivered`, its drop tells the host no more bytes will come.
struct Fetching {
    routes: Arc<Mutex<Routes>>,
    frames: mpsc::Sender<ToEnvironment>,
    lease: String,
    request: String,
    /// Its Fetch is queued, the host has said nothing final of it, and not
    /// every byte is delivered: dropped now, the host is told `Undelivered`.
    undelivered: bool,
}

impl Drop for Fetching {
    fn drop(&mut self) {
        let mut routes = self.routes.lock().unwrap_or_else(PoisonError::into_inner);
        // Its lease's fence, taken under the same lock an End is asked
        // under: an End not yet asked then waits for the Undelivered to be
        // queued, so the host settles the fetch as given up, never as the
        // lease's end. One already asked goes first, as it was asked first.
        let held = routes.leases.get_mut(&self.lease).map(|route| {
            route.fetches.remove(&self.request);
            match route.ending {
                true => None,
                false => route.pumps.clone().try_read_owned().ok(),
            }
        });
        drop(routes);
        // A lease no longer held here has no part left to let go.
        if let Some(fence) = held.filter(|_| self.undelivered) {
            deliver(
                &self.frames,
                None,
                ToEnvironment::Undelivered {
                    lease: self.lease.clone(),
                    request: self.request.clone(),
                },
                fence,
            );
        }
    }
}

/// Hand a read's answer to whoever asked it; a read nobody asked is dropped.
/// One asked by bytes whose reader let go is answered to nobody.
fn answer_read(
    routes: &mut Routes,
    lease: &str,
    artifact: u32,
    offset: u64,
    answer: ArtifactReadAnswer,
) -> Routed {
    match routes
        .leases
        .get_mut(lease)
        .and_then(|route| route.reads.remove(&(artifact, offset)))
    {
        Some(waiter) => {
            let _ = waiter.send(answer);
            Routed::Done
        }
        None => Routed::Dropped,
    }
}

fn channel_route<'a>(
    routes: &'a mut Routes,
    lease: &str,
    channel: u32,
) -> Option<&'a mut ChannelRoute> {
    routes.leases.get_mut(lease)?.channels.get_mut(&channel)
}

fn frame_name(frame: &FromEnvironment) -> &'static str {
    match frame {
        FromEnvironment::Hello { .. } => "hello",
        FromEnvironment::Unavailable { .. } => "unavailable",
        FromEnvironment::Keepalive => "keepalive",
        FromEnvironment::Machine { .. } => "machine",
        FromEnvironment::Draining => "draining",
        FromEnvironment::Granted { .. } => "granted",
        FromEnvironment::Refused { .. } => "refused",
        FromEnvironment::StartFailed { .. } => "start_failed",
        FromEnvironment::Output { .. } => "output",
        FromEnvironment::OutputClosed { .. } => "output_closed",
        FromEnvironment::Stopped { .. } => "stopped",
        FromEnvironment::Ending { .. } => "ending",
        FromEnvironment::Ended { .. } => "ended",
        FromEnvironment::Accounted { .. } => "accounted",
        FromEnvironment::Published { .. } => "published",
        FromEnvironment::NotPublished { .. } => "not_published",
        FromEnvironment::FetchAccepted { .. } => "fetch_accepted",
        FromEnvironment::FetchRefused { .. } => "fetch_refused",
        FromEnvironment::Fetched { .. } => "fetched",
        FromEnvironment::ArtifactRead { .. } => "artifact_read",
        FromEnvironment::ArtifactNotRead { .. } => "artifact_not_read",
        FromEnvironment::Ran { .. } => "ran",
    }
}

fn frame_channel(frame: &FromEnvironment) -> Option<u32> {
    match frame {
        FromEnvironment::StartFailed { channel, .. }
        | FromEnvironment::Output { channel, .. }
        | FromEnvironment::OutputClosed { channel, .. }
        | FromEnvironment::Stopped { channel, .. } => Some(*channel),
        _ => None,
    }
}

/// Read the host's frames until its stream ends, then close every route.
async fn demux<R: AsyncRead + Unpin>(
    link: std::sync::Weak<HostLink>,
    shared: Shared,
    mut stream: FrameStream<R>,
    silence: Duration,
    _ending: watch::Sender<()>,
) {
    loop {
        let body = match tokio::time::timeout(silence, stream.next()).await {
            Ok(Ok(Some(body))) => body,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(host = shared.host.as_str(), %error, "the host's lease stream could not be read");
                break;
            }
            Err(_) => {
                tracing::warn!(
                    host = shared.host.as_str(),
                    "nothing was heard from the host for its silence limit"
                );
                break;
            }
        };
        match decode::<FromEnvironment>(&body) {
            Ok(frame) => shared.route(frame),
            Err(error) => {
                tracing::warn!(host = shared.host.as_str(), %error, "an unreadable lease frame");
                shared.dropped(None, None, "unreadable");
            }
        }
    }
    // Closed first, so nothing more is routed or asked; recorded next; and
    // only then are the routes, and so each lease's watch, let go: a lease
    // seen lost already knows whether its loss was recorded.
    let leases = {
        let mut routes = shared.routes();
        routes.closed = true;
        routes.leases.keys().cloned().collect::<Vec<String>>()
    };
    tracing::warn!(
        host = shared.host.as_str(),
        leases = leases.len(),
        "the connection to the host ended"
    );
    let recorded = shared.record(EnvironmentEvent::ConnectionLost {
        host: shared.host.as_str().into(),
        leases: leases.clone(),
    });
    {
        let mut routes = shared.routes();
        if !recorded {
            routes.unaudited.extend(leases);
        }
        routes.accounts.clear();
        routes.leases.clear();
    }
    // Ends the ssh process too, if it is somehow still running.
    if let Some(link) = link.upgrade() {
        link.close();
    }
}

async fn write_frames(
    mut output: Box<dyn AsyncWrite + Send + Unpin>,
    mut frames: mpsc::Receiver<ToEnvironment>,
    keepalive: Keepalive,
) {
    loop {
        // Idle for `keepalive`: the host is told the gateway is still there,
        // which a network gone silent cannot tell it.
        let frame = match tokio::time::timeout(keepalive, frames.recv()).await {
            Ok(Some(frame)) => frame,
            Ok(None) => break,
            Err(_) => ToEnvironment::Keepalive,
        };
        // A frame that cannot be encoded is that frame's failure alone; the
        // connection, which other leases share, goes on.
        let bytes = match encode(&frame) {
            Ok(bytes) => bytes,
            Err(error) => {
                tracing::error!(%error, lease = frame.lease(), "a lease frame could not be encoded; it is not sent");
                continue;
            }
        };
        if let Err(error) = write_all(&mut output, &bytes).await {
            tracing::warn!(%error, "a lease frame could not be written to the host");
            return;
        }
    }
    let _ = output.shutdown().await;
}

/// Queue End for `lease` once `pumps` can be held for writing. A command
/// whose `Ran` was already said is released if `Ended` does not arrive
/// within [`COMMAND_STOP_WAIT`] of that End being queued.
fn queue_end(
    frames: mpsc::Sender<ToEnvironment>,
    pumps: Arc<RwLock<()>>,
    lease: String,
    routes: Arc<Mutex<Routes>>,
) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        tracing::error!("a command's End asked outside a runtime is not sent");
        return;
    };
    runtime.spawn(async move {
        let _done = pumps.write_owned().await;
        if frames
            .send(ToEnvironment::End {
                lease: lease.clone(),
            })
            .await
            .is_err()
        {
            return;
        }
        let arm = {
            let mut routes = routes.lock().unwrap_or_else(PoisonError::into_inner);
            let Some(route) = routes.leases.get_mut(&lease) else {
                return;
            };
            route.end_queued = true;
            route.command && route.ran_answered
        };
        if arm {
            release_command_route(routes, lease).await;
        }
    });
}

/// Give back a finished command's route when its host does not answer
/// `Ended`. A route already removed, or one whose `Ran` was not said, stays
/// as it is.
async fn release_command_route(routes: Arc<Mutex<Routes>>, lease: String) {
    tokio::time::sleep(COMMAND_STOP_WAIT).await;
    let mut routes = routes.lock().unwrap_or_else(PoisonError::into_inner);
    if routes.leases.get(&lease).is_some_and(|route| {
        route.command && route.end_sent && route.end_queued && route.ran_answered
    }) {
        routes.leases.remove(&lease);
    }
}

/// Queue `frame` for the host even when there is no room just now: on a
/// task of its own, once every pump `after` holds is done, behind what is
/// already queued; done once it is queued or the connection is gone.
/// `held` is let go only then.
/// Outside a runtime, which a gateway never is, it is queued only if there
/// is room.
fn deliver(
    frames: &mpsc::Sender<ToEnvironment>,
    after: Option<Arc<RwLock<()>>>,
    frame: ToEnvironment,
    held: Option<OwnedRwLockReadGuard<()>>,
) {
    match tokio::runtime::Handle::try_current() {
        Ok(runtime) => {
            let frames = frames.clone();
            runtime.spawn(async move {
                let _done = match after {
                    Some(pumps) => Some(pumps.write_owned().await),
                    None => None,
                };
                let _ = frames.send(frame).await;
                drop(held);
            });
        }
        Err(_) => {
            if frames.try_send(frame).is_err() {
                tracing::error!(
                    "a lease frame asked outside a runtime found no room; it is not sent"
                );
            }
        }
    }
}

async fn write_all(
    output: &mut Box<dyn AsyncWrite + Send + Unpin>,
    bytes: &[u8],
) -> std::io::Result<()> {
    output.write_all(bytes).await?;
    output.flush().await
}

async fn pump_output(mut chunks: mpsc::Receiver<Vec<u8>>, mut output: DuplexStream) {
    while let Some(bytes) = chunks.recv().await {
        if output.write_all(&bytes).await.is_err() {
            return;
        }
    }
    let _ = output.shutdown().await;
}

/// The one sender of a harness's frames: its input as it arrives, its
/// input's end, and then, once asked, its Stop, after whatever input was
/// already written. Ends without a Stop when nobody can ask for one any
/// more: the lease ended, or the connection is gone.
async fn pump_input(
    mut input: DuplexStream,
    frames: mpsc::Sender<ToEnvironment>,
    lease: String,
    channel: u32,
    mut stop: oneshot::Receiver<StopRequest>,
    // Held until it is done: its lease's End waits for that.
    _running: OwnedRwLockReadGuard<()>,
) {
    let mut buffer = vec![0; MAX_DATA_BYTES];
    let mut open = true;
    loop {
        let read = tokio::select! {
            biased;
            asked = &mut stop => {
                let Ok(request) = asked else { return };
                // What the binding wrote before it asked goes first: what is
                // in the pipe now, at most one pipe's worth.
                let mut drained = 0;
                while open && drained < PIPE_BYTES {
                    match tokio::task::unconstrained(input.read(&mut buffer)).now_or_never() {
                        None => break,
                        Some(Ok(read)) if read > 0 => {
                            drained += read;
                            if !send_input(&frames, &lease, channel, &buffer[..read]).await {
                                return;
                            }
                        }
                        Some(_) => {
                            open = false;
                            if !send_closed(&frames, &lease, channel).await {
                                return;
                            }
                        }
                    }
                }
                let _ = frames
                    .send(ToEnvironment::Stop {
                        lease,
                        channel,
                        grace_ms: request.grace_ms,
                        kill_ms: request.kill_ms,
                    })
                    .await;
                return;
            }
            read = input.read(&mut buffer), if open => read,
        };
        let sent = match read {
            Ok(read) if read > 0 => send_input(&frames, &lease, channel, &buffer[..read]).await,
            _ => {
                open = false;
                send_closed(&frames, &lease, channel).await
            }
        };
        if !sent {
            return;
        }
    }
}

async fn send_input(
    frames: &mpsc::Sender<ToEnvironment>,
    lease: &str,
    channel: u32,
    bytes: &[u8],
) -> bool {
    frames
        .send(ToEnvironment::Input {
            lease: lease.into(),
            channel,
            data: Data(bytes.to_vec()),
        })
        .await
        .is_ok()
}

async fn send_closed(frames: &mpsc::Sender<ToEnvironment>, lease: &str, channel: u32) -> bool {
    frames
        .send(ToEnvironment::InputClosed {
            lease: lease.into(),
            channel,
        })
        .await
        .is_ok()
}
