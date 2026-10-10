//! One connection to a host's `nessa env serve`, shared by every lease this
//! gateway holds there, and the routing of its frames to them.
//!
//! ```text
//! open: connect ──▶ first frame ──▶ read_hello ──▶ protocol == LEASE_PROTOCOL?
//!          no ──▶ VersionRefused, nothing sent ──▶ EnvironmentVersionMismatch
//!          empty workspace ──▶ Unavailable{busy | notConfigured} ──▶ refused
//!          yes ──▶ writer task (frames out), demux task (frames in)
//! writer: frames out, in the order queued; a Keepalive when idle `keepalive`
//! demux: frame ──▶ lease route? ──▶ grant / channel output / stopped / ended
//!                   published ──▶ the lease's offers ──▶ (taken) Collected
//!                     no room, or nobody takes them ──▶ Collected refused
//!                   none ──▶ FrameDropped, never applied elsewhere (row L9)
//! end of stream ──▶ every route dropped: waiters unanswered, outputs ended,
//!                   each lease's `gone` closed ──▶ ConnectionLost
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
use super::connector::{ArtifactChannels, LeaseConnector};
use crate::env::LEASE_PROTOCOL;
use crate::env_serve::application::FrameStream;
use futures_util::FutureExt;
use nessa_protocol::lease::{
    decode, encode, read_hello, stop_steps, Cleanup, Collection, CollectionRefusal, Data,
    FromEnvironment, GrantRefusal, StagedArtifact, ToEnvironment, Unavailability,
    MAX_ARTIFACTS_IN_FLIGHT, MAX_DATA_BYTES,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use std::{
    collections::{BTreeMap, HashMap, HashSet},
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

/// One lease held on this connection.
struct LeaseRoute {
    grant: Option<oneshot::Sender<Result<(), GrantRefusal>>>,
    ended: Option<oneshot::Sender<Cleanup>>,
    /// Its End was asked: no harness starts under it and none is asked to
    /// stop, as its `Ended` answers their stops.
    ending: bool,
    /// Its End is queued for the host, or will be once its pumps are done.
    end_sent: bool,
    /// Held for reading by each of its harnesses' pumps while it runs; its
    /// End is queued only once it can be held for writing.
    pumps: Arc<RwLock<()>>,
    channels: HashMap<u32, ChannelRoute>,
    next_channel: u32,
    /// Where the files the host publishes under it are offered: `None`
    /// once whoever takes them let go.
    published: Option<mpsc::Sender<Published>>,
    /// Dropped with the route: the lease is no longer routed here.
    _gone: watch::Sender<()>,
}

#[derive(Default)]
struct Routes {
    /// The connection ended: nothing more is routed or asked.
    closed: bool,
    leases: HashMap<String, LeaseRoute>,
    accounts: HashMap<String, Vec<oneshot::Sender<Cleanup>>>,
    drops: u32,
    /// Leases a transition of which could not be recorded in the audit:
    /// none of their outcomes is settled as confirmed.
    unaudited: HashSet<String>,
}

/// A file the host published under a lease, as the demux hands it on.
pub(crate) struct Published {
    pub(crate) artifact: u32,
    pub(crate) file: StagedArtifact,
}

/// What a granted lease is handed: its watch, and the files published
/// under it.
pub(crate) struct Granted {
    /// Closes once the lease is no longer routed here.
    pub(crate) gone: watch::Receiver<()>,
    /// Ends once the lease is no longer routed here.
    pub(crate) published: mpsc::Receiver<Published>,
}

/// One open connection to a host's environment.
pub(crate) struct HostLink {
    host: SshDestination,
    workspace: PathBuf,
    frames: mpsc::Sender<ToEnvironment>,
    routes: Arc<Mutex<Routes>>,
    audit: Arc<dyn EnvironmentAudit>,
    keep: Mutex<Box<dyn Send>>,
    /// Opens artifact channels on this same connection.
    artifacts: Arc<dyn ArtifactChannels>,
}

/// How long the writer may be idle before it says the gateway is there.
pub(crate) type Keepalive = Duration;

/// What a started harness's binding is handed.
pub(crate) struct StartedChannel {
    pub(crate) channel: u32,
    pub(crate) input: DuplexStream,
    pub(crate) output: DuplexStream,
}

impl HostLink {
    /// Connect to `host` and read its hello within `deadline`.
    ///
    /// # Errors
    /// The typed refusal: unreachable, another build, or busy.
    pub(crate) async fn open(
        host: &SshDestination,
        connector: &dyn LeaseConnector,
        audit: Arc<dyn EnvironmentAudit>,
        deadline: Duration,
        keepalive: Keepalive,
    ) -> Result<Arc<Self>, LeaseRefusal> {
        let connection = connector.connect(host).map_err(|error| {
            tracing::warn!(host = host.as_str(), %error, "ssh could not be started");
            LeaseRefusal::EnvironmentUnreachable
        })?;
        let mut stream = FrameStream::new(connection.from_environment);
        let first = match tokio::time::timeout(deadline, stream.next()).await {
            Ok(Ok(Some(body))) => body,
            Ok(Ok(None) | Err(_)) | Err(_) => {
                tracing::warn!(
                    host = host.as_str(),
                    "the host's environment did not answer"
                );
                return Err(LeaseRefusal::EnvironmentUnreachable);
            }
        };
        let hello = match read_hello(&first) {
            Some(hello) if hello.protocol == LEASE_PROTOCOL => hello,
            other => {
                let (build, protocol) = other.map(|hello| (hello.build, hello.protocol)).unzip();
                let event = EnvironmentEvent::VersionRefused {
                    host: host.as_str().into(),
                    build,
                    protocol,
                };
                if let Err(error) = audit.record(&event) {
                    tracing::error!(%error, "an environment's version refusal could not be recorded");
                }
                tracing::warn!(
                    host = host.as_str(),
                    "the host speaks another lease protocol"
                );
                return Err(LeaseRefusal::EnvironmentVersionMismatch);
            }
        };
        let workspace = PathBuf::from(&hello.workspace);
        if !workspace.is_absolute() {
            return Err(Self::unavailable(host, &mut stream, audit.as_ref(), deadline).await);
        }
        let connected = EnvironmentEvent::Connected {
            host: host.as_str().into(),
            workspace: hello.workspace.clone(),
        };
        if let Err(error) = audit.record(&connected) {
            tracing::error!(%error, "an environment connection could not be recorded; it is not used");
            return Err(LeaseRefusal::EnvironmentUnreachable);
        }
        let (frames, outgoing) = mpsc::channel(TO_HOST_QUEUE);
        tokio::spawn(write_frames(connection.to_environment, outgoing, keepalive));
        let routes = Arc::new(Mutex::new(Routes::default()));
        let link = Arc::new(Self {
            host: host.clone(),
            workspace,
            frames,
            routes,
            audit,
            keep: Mutex::new(connection.keep),
            artifacts: connection.artifacts,
        });
        tokio::spawn(demux(Arc::downgrade(&link), link.shared(), stream));
        Ok(link)
    }

    /// The refusal a hello without a workspace precedes.
    async fn unavailable<R: AsyncRead + Unpin>(
        host: &SshDestination,
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
        let host_name = host.as_str().to_owned();
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

    /// How artifact channels are opened on this connection.
    pub(crate) fn artifact_channels(&self) -> Arc<dyn ArtifactChannels> {
        self.artifacts.clone()
    }

    /// Whether the connection still serves leases.
    pub(crate) fn is_open(&self) -> bool {
        !self.routes().closed && !self.frames.is_closed()
    }

    /// Ask the host to admit `lease` for `agent`, waiting at most `deadline`.
    /// Granted, it answers the lease's watch: it closes once the lease is no
    /// longer routed here — ended, or the connection lost — and, being the
    /// lease's own from its grant, says so even when that happened before
    /// anyone looked.
    ///
    /// # Errors
    /// The typed refusal; a connection lost or silent is unreachable.
    pub(crate) async fn grant(
        &self,
        lease: &str,
        agent: &str,
        deadline: Duration,
    ) -> Result<Granted, LeaseRefusal> {
        let (answer, answered) = oneshot::channel();
        let (offers, published) = mpsc::channel(MAX_ARTIFACTS_IN_FLIGHT);
        let gone = {
            let mut routes = self.routes();
            if routes.closed || routes.leases.contains_key(lease) {
                return Err(LeaseRefusal::EnvironmentUnreachable);
            }
            let (gone_sender, gone) = watch::channel(());
            routes.leases.insert(
                lease.into(),
                LeaseRoute {
                    grant: Some(answer),
                    ended: None,
                    ending: false,
                    end_sent: false,
                    pumps: Arc::new(RwLock::new(())),
                    channels: HashMap::new(),
                    next_channel: 1,
                    published: Some(offers),
                    _gone: gone_sender,
                },
            );
            gone
        };
        let sent = self
            .frames
            .send(ToEnvironment::Grant {
                lease: lease.into(),
                agent: agent.into(),
            })
            .await;
        if sent.is_err() {
            self.routes().leases.remove(lease);
            return Err(LeaseRefusal::EnvironmentUnreachable);
        }
        match tokio::time::timeout(deadline, answered).await {
            Ok(Ok(Ok(()))) => Ok(Granted { gone, published }),
            refused => {
                self.routes().leases.remove(lease);
                if refused.is_err() {
                    // No answer in time: the host may still admit it, so it
                    // is ended there rather than held for nobody.
                    deliver(
                        &self.frames,
                        None,
                        ToEnvironment::End {
                            lease: lease.into(),
                        },
                    );
                }
                Err(match refused {
                    Ok(Ok(Err(GrantRefusal::AgentUnavailable))) => LeaseRefusal::AgentUnavailable,
                    _ => LeaseRefusal::EnvironmentUnreachable,
                })
            }
        }
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
            if routes.closed {
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
        let mut routes = self.routes();
        if let Some(route) = routes.leases.get_mut(lease).filter(|route| !route.end_sent) {
            route.end_sent = true;
            let pumps = begin_end(route);
            deliver(
                &self.frames,
                Some(pumps),
                ToEnvironment::End {
                    lease: lease.into(),
                },
            );
        }
    }

    /// Tell the host what became of a file it published under `lease`,
    /// unless the lease is no longer held here, when the host already
    /// answered it as ended. Always sent, behind what is queued.
    pub(crate) fn collected(&self, lease: &str, artifact: u32, outcome: Collection) {
        if self.routes().leases.contains_key(lease) {
            deliver(
                &self.frames,
                None,
                ToEnvironment::Collected {
                    lease: lease.into(),
                    artifact,
                    outcome,
                },
            );
        }
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
    host: SshDestination,
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
            Routed::Answer(frame) => {
                if let Some(frames) = self.frames.upgrade() {
                    deliver(&frames, None, frame);
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
        }
    }
}

enum Routed {
    Done,
    /// Routed, and answered at once by this gateway.
    Answer(ToEnvironment),
    Dropped,
    Overflow {
        lease: String,
        channel: u32,
    },
}

fn route_frame(routes: &mut Routes, frame: FromEnvironment) -> Routed {
    match frame {
        FromEnvironment::Hello { .. } | FromEnvironment::Unavailable { .. } => Routed::Dropped,
        FromEnvironment::Accounted { lease, cleanup } => match routes.accounts.remove(&lease) {
            Some(waiters) => {
                for waiter in waiters {
                    let _ = waiter.send(cleanup);
                }
                Routed::Done
            }
            None => Routed::Dropped,
        },
        FromEnvironment::Granted { lease } => {
            match routes
                .leases
                .get_mut(&lease)
                .and_then(|route| route.grant.take())
            {
                Some(waiter) => {
                    let _ = waiter.send(Ok(()));
                    Routed::Done
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
                    let _ = waiter.send(Err(reason));
                    Routed::Done
                }
                None => Routed::Dropped,
            }
        }
        FromEnvironment::Published {
            lease,
            artifact,
            file,
        } => {
            let Some(route) = routes.leases.get_mut(&lease) else {
                return Routed::Dropped;
            };
            let refused = |reason| {
                Routed::Answer(ToEnvironment::Collected {
                    lease: lease.clone(),
                    artifact,
                    outcome: Collection::Refused { reason },
                })
            };
            // Once its End is asked, nothing more is taken under it.
            if route.ending {
                return refused(CollectionRefusal::LeaseEnded);
            }
            match route
                .published
                .as_ref()
                .map(|offers| offers.try_send(Published { artifact, file }))
            {
                Some(Ok(())) => Routed::Done,
                // The host keeps at most as many in flight as there is room
                // for: one past it is not this gateway's to take.
                Some(Err(mpsc::error::TrySendError::Full(_))) => {
                    refused(CollectionRefusal::BudgetExceeded)
                }
                Some(Err(mpsc::error::TrySendError::Closed(_))) | None => {
                    route.published = None;
                    refused(CollectionRefusal::ChannelUnavailable)
                }
            }
        }
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
        FromEnvironment::Granted { .. } => "granted",
        FromEnvironment::Refused { .. } => "refused",
        FromEnvironment::StartFailed { .. } => "start_failed",
        FromEnvironment::Output { .. } => "output",
        FromEnvironment::OutputClosed { .. } => "output_closed",
        FromEnvironment::Stopped { .. } => "stopped",
        FromEnvironment::Ended { .. } => "ended",
        FromEnvironment::Accounted { .. } => "accounted",
        FromEnvironment::Published { .. } => "published",
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
) {
    loop {
        let body = match stream.next().await {
            Ok(Some(body)) => body,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(host = shared.host.as_str(), %error, "the host's lease stream could not be read");
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
        let mut keep = link.keep.lock().unwrap_or_else(PoisonError::into_inner);
        drop(std::mem::replace(&mut *keep, Box::new(())));
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

/// Queue `frame` for the host even when there is no room just now: on a
/// task of its own, once every pump `after` holds is done, behind what is
/// already queued; done once it is queued or the connection is gone.
/// Outside a runtime, which a gateway never is, it is queued only if there
/// is room.
fn deliver(
    frames: &mpsc::Sender<ToEnvironment>,
    after: Option<Arc<RwLock<()>>>,
    frame: ToEnvironment,
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
