//! One connection to a host's `nessa env serve`, shared by every lease this
//! gateway holds there, and the routing of its frames to them.
//!
//! ```text
//! open: connect ──▶ first frame ──▶ read_hello ──▶ build == this build?
//!          no ──▶ VersionRefused, nothing sent ──▶ EnvironmentVersionMismatch
//!          empty workspace ──▶ Unavailable{busy | notConfigured} ──▶ refused
//!          yes ──▶ writer task (frames out), demux task (frames in)
//! demux: frame ──▶ lease route? ──▶ grant / channel output / stopped / ended
//!                   none ──▶ FrameDropped, never applied elsewhere (row L9)
//! end of stream ──▶ every route dropped: waiters unanswered, outputs ended,
//!                   each lease's `gone` closed ──▶ ConnectionLost
//! ```
//!
//! Arrows are steps and frames, in order. A lease's frames are routed only
//! while the gateway holds that lease on this connection: from its grant
//! until the host's `Ended`, a refusal, or the connection's end. Every wait
//! here is bounded by its caller or by a deadline of its own, and no lock is
//! held across one.
use super::audit::{EnvironmentAudit, EnvironmentEvent};
use super::connector::LeaseConnector;
use crate::env::VERSION;
use crate::env_serve::application::{write_frame, FrameStream};
use nessa_protocol::lease::{
    decode, read_hello, Cleanup, Data, FromEnvironment, GrantRefusal, ToEnvironment,
    Unavailability, MAX_DATA_BYTES,
};
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use std::{
    collections::{BTreeMap, HashMap},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard, PoisonError},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, DuplexStream},
    sync::{mpsc, oneshot, watch},
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
/// How long a stop waits beyond the harness's own budgets for the host's
/// answer.
const STOP_MARGIN: Duration = Duration::from_secs(5);

/// One harness under a lease, as the demux routes to it.
struct ChannelRoute {
    /// Its binding's output; `None` once ended.
    output: Option<mpsc::Sender<Vec<u8>>>,
    /// Waiting for the host's `Stopped`.
    stopped: Option<oneshot::Sender<Cleanup>>,
    /// The host said it never started.
    start_failed: bool,
}

/// One lease held on this connection.
struct LeaseRoute {
    grant: Option<oneshot::Sender<Result<(), GrantRefusal>>>,
    ended: Option<oneshot::Sender<Cleanup>>,
    channels: HashMap<u32, ChannelRoute>,
    next_channel: u32,
    /// Dropped with the route: the lease is no longer routed here.
    _gone: watch::Sender<()>,
    gone: watch::Receiver<()>,
}

#[derive(Default)]
struct Routes {
    /// The connection ended: nothing more is routed or asked.
    closed: bool,
    leases: HashMap<String, LeaseRoute>,
    accounts: HashMap<String, Vec<oneshot::Sender<Cleanup>>>,
    drops: u32,
}

/// One open connection to a host's environment.
pub(crate) struct HostLink {
    host: SshDestination,
    workspace: PathBuf,
    frames: mpsc::Sender<ToEnvironment>,
    routes: Arc<Mutex<Routes>>,
    audit: Arc<dyn EnvironmentAudit>,
    keep: Mutex<Box<dyn Send>>,
}

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
            Some(hello) if hello.build == VERSION => hello,
            other => {
                let event = EnvironmentEvent::VersionRefused {
                    host: host.as_str().into(),
                    build: other.map(|hello| hello.build),
                };
                if let Err(error) = audit.record(&event) {
                    tracing::error!(%error, "an environment's version refusal could not be recorded");
                }
                tracing::warn!(host = host.as_str(), "the host runs another build of nessa");
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
        tokio::spawn(write_frames(connection.to_environment, outgoing));
        let routes = Arc::new(Mutex::new(Routes::default()));
        let link = Arc::new(Self {
            host: host.clone(),
            workspace,
            frames,
            routes,
            audit,
            keep: Mutex::new(connection.keep),
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
            frames: self.frames.clone(),
            routes: self.routes.clone(),
            audit: self.audit.clone(),
        }
    }

    fn routes(&self) -> MutexGuard<'_, Routes> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The host's workspace, from its hello.
    pub(crate) fn workspace(&self) -> &Path {
        &self.workspace
    }

    /// Whether the connection still serves leases.
    pub(crate) fn is_open(&self) -> bool {
        !self.routes().closed && !self.frames.is_closed()
    }

    /// Ask the host to admit `lease` for `agent`, waiting at most `deadline`.
    ///
    /// # Errors
    /// The typed refusal; a connection lost or silent is unreachable.
    pub(crate) async fn grant(
        &self,
        lease: &str,
        agent: &str,
        deadline: Duration,
    ) -> Result<(), LeaseRefusal> {
        let (answer, answered) = oneshot::channel();
        {
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
                    channels: HashMap::new(),
                    next_channel: 1,
                    _gone: gone_sender,
                    gone,
                },
            );
        }
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
            Ok(Ok(Ok(()))) => Ok(()),
            refused => {
                self.routes().leases.remove(lease);
                Err(match refused {
                    Ok(Ok(Err(GrantRefusal::AgentUnavailable))) => LeaseRefusal::AgentUnavailable,
                    _ => LeaseRefusal::EnvironmentUnreachable,
                })
            }
        }
    }

    /// Resolves once `lease` is no longer routed here: ended, or the
    /// connection lost.
    pub(crate) fn gone(&self, lease: &str) -> Option<watch::Receiver<()>> {
        self.routes()
            .leases
            .get(lease)
            .map(|route| route.gone.clone())
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
        let channel = {
            let mut routes = self.routes();
            if routes.closed {
                return Err(StartError::Closed);
            }
            let Some(route) = routes.leases.get_mut(lease) else {
                return Err(StartError::Closed);
            };
            let channel = route.next_channel;
            route.next_channel = channel.checked_add(1).ok_or(StartError::Closed)?;
            route.channels.insert(
                channel,
                ChannelRoute {
                    output: Some(output_sender),
                    stopped: None,
                    start_failed: false,
                },
            );
            let start = ToEnvironment::Start {
                lease: lease.into(),
                channel,
                environment,
            };
            if let Err(error) = self.frames.try_send(start) {
                route.channels.remove(&channel);
                return Err(match error {
                    mpsc::error::TrySendError::Full(_) => StartError::Busy,
                    mpsc::error::TrySendError::Closed(_) => StartError::Closed,
                });
            }
            channel
        };
        let (output, pump_out) = tokio::io::duplex(PIPE_BYTES);
        let (input, pump_in) = tokio::io::duplex(PIPE_BYTES);
        tokio::spawn(pump_output(output_receiver, pump_out));
        tokio::spawn(pump_input(
            pump_in,
            self.frames.clone(),
            lease.to_owned(),
            channel,
        ));
        Ok(StartedChannel {
            channel,
            input,
            output,
        })
    }

    /// Stop one harness and wait for the host's evidence: `None` when it
    /// cannot be had — the connection lost, the lease ended, or no answer in
    /// time.
    pub(crate) async fn stop(
        &self,
        lease: &str,
        channel: u32,
        grace: Duration,
        kill: Duration,
    ) -> Option<Cleanup> {
        let answered = {
            let mut routes = self.routes();
            if routes.closed {
                return None;
            }
            let route = routes.leases.get_mut(lease)?.channels.get_mut(&channel)?;
            if route.start_failed {
                return Some(Cleanup::NotHeld);
            }
            let (answer, answered) = oneshot::channel();
            route.stopped = Some(answer);
            answered
        };
        self.frames
            .send(ToEnvironment::Stop {
                lease: lease.into(),
                channel,
                grace_ms: millis(grace),
                kill_ms: millis(kill),
            })
            .await
            .ok()?;
        let bound = grace + kill + kill + STOP_MARGIN;
        tokio::time::timeout(bound, answered).await.ok()?.ok()
    }

    /// Stop a harness whose binding let go of it without a stop: asked, not
    /// waited for.
    pub(crate) fn abandon(&self, lease: &str, channel: u32) {
        let _ = self.frames.try_send(ToEnvironment::Stop {
            lease: lease.into(),
            channel,
            grace_ms: 0,
            kill_ms: 5_000,
        });
    }

    /// End `lease` and wait for the host's evidence; `None` when it cannot be
    /// had. The caller bounds the wait.
    pub(crate) async fn end(&self, lease: &str) -> Option<Cleanup> {
        let answered = {
            let mut routes = self.routes();
            if routes.closed {
                return None;
            }
            routes.leases.get_mut(lease).map(|route| {
                let (answer, answered) = oneshot::channel();
                route.ended = Some(answer);
                answered
            })
        };
        // Not held on this connection: what the host recorded of it.
        let Some(answered) = answered else {
            return self.account(lease).await;
        };
        self.frames
            .send(ToEnvironment::End {
                lease: lease.into(),
            })
            .await
            .ok()?;
        answered.await.ok()
    }

    /// End `lease` without waiting: its hold was let go.
    pub(crate) fn abandon_lease(&self, lease: &str) {
        let _ = self.frames.try_send(ToEnvironment::End {
            lease: lease.into(),
        });
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
}

fn millis(duration: Duration) -> u64 {
    u64::try_from(duration.as_millis()).unwrap_or(u64::MAX)
}

/// What the demux needs of the link without keeping it alive.
struct Shared {
    host: SshDestination,
    frames: mpsc::Sender<ToEnvironment>,
    routes: Arc<Mutex<Routes>>,
    audit: Arc<dyn EnvironmentAudit>,
}

impl Shared {
    fn routes(&self) -> MutexGuard<'_, Routes> {
        self.routes.lock().unwrap_or_else(PoisonError::into_inner)
    }

    fn record(&self, event: EnvironmentEvent) {
        if let Err(error) = self.audit.record(&event) {
            tracing::error!(%error, ?event, "an environment event could not be recorded");
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
            self.record(EnvironmentEvent::FrameDropped {
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
            Routed::Dropped => self.dropped(lease.as_deref(), channel, name),
            Routed::Overflow { lease, channel } => {
                self.record(EnvironmentEvent::OutputOverflow {
                    host: self.host.as_str().into(),
                    lease: lease.clone(),
                    channel,
                });
                let _ = self.frames.try_send(ToEnvironment::Stop {
                    lease,
                    channel,
                    grace_ms: 0,
                    kill_ms: 5_000,
                });
            }
        }
    }
}

enum Routed {
    Done,
    Dropped,
    Overflow { lease: String, channel: u32 },
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
        FromEnvironment::Ended { lease, cleanup } => match routes.leases.remove(&lease) {
            Some(mut route) => {
                if let Some(waiter) = route.ended.take() {
                    let _ = waiter.send(cleanup);
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
        } => match channel_route(routes, &lease, channel) {
            Some(route) => match &route.output {
                Some(output) => match output.try_send(bytes) {
                    Ok(()) => Routed::Done,
                    // The binding let go of its output: nothing reads it.
                    Err(mpsc::error::TrySendError::Closed(_)) => {
                        route.output = None;
                        Routed::Done
                    }
                    Err(mpsc::error::TrySendError::Full(_)) => {
                        route.output = None;
                        Routed::Overflow { lease, channel }
                    }
                },
                None => Routed::Dropped,
            },
            None => Routed::Dropped,
        },
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
            match route.channels.remove(&channel) {
                Some(mut channel) => {
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
    let leases = {
        let mut routes = shared.routes();
        routes.closed = true;
        routes.accounts.clear();
        let leases: Vec<String> = routes.leases.keys().cloned().collect();
        routes.leases.clear();
        leases
    };
    tracing::warn!(
        host = shared.host.as_str(),
        leases = leases.len(),
        "the connection to the host ended"
    );
    shared.record(EnvironmentEvent::ConnectionLost {
        host: shared.host.as_str().into(),
        leases,
    });
    // Ends the ssh process too, if it is somehow still running.
    if let Some(link) = link.upgrade() {
        let mut keep = link.keep.lock().unwrap_or_else(PoisonError::into_inner);
        drop(std::mem::replace(&mut *keep, Box::new(())));
    }
}

async fn write_frames(
    mut output: Box<dyn AsyncWrite + Send + Unpin>,
    mut frames: mpsc::Receiver<ToEnvironment>,
) {
    while let Some(frame) = frames.recv().await {
        if let Err(error) = write_frame(&mut output, &frame).await {
            tracing::warn!(%error, "a lease frame could not be written to the host");
            return;
        }
    }
    let _ = output.shutdown().await;
}

async fn pump_output(mut chunks: mpsc::Receiver<Vec<u8>>, mut output: DuplexStream) {
    while let Some(bytes) = chunks.recv().await {
        if output.write_all(&bytes).await.is_err() {
            return;
        }
    }
    let _ = output.shutdown().await;
}

async fn pump_input(
    mut input: DuplexStream,
    frames: mpsc::Sender<ToEnvironment>,
    lease: String,
    channel: u32,
) {
    let mut buffer = vec![0; MAX_DATA_BYTES];
    loop {
        match input.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let frame = ToEnvironment::Input {
                    lease: lease.clone(),
                    channel,
                    data: Data(buffer[..read].to_vec()),
                };
                if frames.send(frame).await.is_err() {
                    return;
                }
            }
        }
    }
    let _ = frames
        .send(ToEnvironment::InputClosed { lease, channel })
        .await;
}
