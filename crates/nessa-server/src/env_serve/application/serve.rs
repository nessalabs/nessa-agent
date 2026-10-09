//! The environment role: serve one gateway's leases over a byte stream
//! (issue #699).
//!
//! ```text
//! gateway ──frames──▶ serve ──Grant──▶ ledger: granted ──▶ Granted
//!                          ──Start──▶ ledger: started ──▶ HarnessLauncher::launch
//!                                         input pump ──▶ harness stdin
//!                                         harness stdout ──▶ output pump ──▶ Output
//!                          ──Stop───▶ cleanup(grace, kill) ──▶ ledger: stopped ──▶ Stopped
//!                          ──End────▶ stop every harness ──▶ ledger: ended ──▶ Ended
//!                          ──Account▶ ledger ──▶ Accounted
//! end of stream ──▶ every lease ended as lost ──▶ ledger: ended(lost)
//! ```
//!
//! Arrows are frames and calls, in order. This side keeps no conversation:
//! no records, no approvals, no events — the gateway speaks the agent's
//! protocol to the harness over its stdin and stdout, which this side only
//! carries. What it owns is the harness's process tree, the executable and
//! credentials it starts with (the host's own configuration, through
//! [`HarnessLauncher`]), and its own audit of every lease, which is also the
//! cleanup evidence it answers with ([`LeaseLedger`]).
//!
//! A frame naming a lease or harness this connection does not hold is
//! dropped and the drop recorded, never applied to another. The connection
//! ending is the gateway gone: there is no reconnect, so every lease it held
//! is ended as lost, its harnesses stopped, and that recorded; a later
//! connection learns it through `Account`.
use super::wire::{write_frame, FrameStream};
use nessa_protocol::lease::{
    decode, Cleanup, Data, FromEnvironment, GrantRefusal, Hello, StartFailure, ToEnvironment,
    Unavailability, MAX_DATA_BYTES,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{HarnessControl, HarnessProcess},
};
use nessa_sdk::domain::agent_execution::leases::LeaseId;
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    io,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
    task::JoinHandle,
};

/// How the host starts an agent's harness: its own executable, arguments,
/// account variables and credentials, in its own workspace.
pub(crate) trait HarnessLauncher: Send + Sync {
    /// The absolute directory harnesses work in.
    fn workspace(&self) -> &str;
    /// Whether the host is configured to run `agent`.
    fn runs(&self, agent: &str) -> bool;
    /// Start `agent`'s harness with `environment` added to the host's own.
    ///
    /// # Errors
    /// The harness could not be started; nothing runs.
    fn launch(
        &self,
        agent: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<HarnessProcess, AgentError>;
}

/// One entry of the environment's own audit of its leases.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum LedgerEntry {
    /// A lease was admitted.
    Granted { lease: String, agent: String },
    /// A lease was refused; nothing ran.
    Refused { lease: String, reason: GrantRefusal },
    /// A harness is about to start under a lease.
    Started { lease: String, channel: u32 },
    /// The harness did not start.
    StartFailed { lease: String, channel: u32 },
    /// A harness was stopped, with what that took.
    Stopped {
        lease: String,
        channel: u32,
        cleanup: Cleanup,
    },
    /// A lease ended, ended by the gateway or lost with its connection, with
    /// what stopping everything under it took.
    Ended {
        lease: String,
        cleanup: Cleanup,
        lost: bool,
    },
    /// A frame naming nothing this connection holds was dropped.
    Dropped {
        lease: Option<String>,
        channel: Option<u32>,
        frame: String,
    },
}

/// The environment's own audit of its leases, and the evidence it answers
/// with. Written before the answer it records is sent.
pub(crate) trait LeaseLedger: Send + Sync {
    /// Append `entry`, durably.
    ///
    /// # Errors
    /// The entry is not durable.
    fn record(&self, entry: &LedgerEntry) -> io::Result<()>;
    /// What became of `lease`, as recorded: its ended entry's cleanup,
    /// [`Cleanup::Uncertain`] for a lease granted and never recorded ended,
    /// [`Cleanup::NotHeld`] for one never granted here.
    ///
    /// # Errors
    /// The audit could not be read.
    fn accounted(&self, lease: &str) -> io::Result<Cleanup>;
}

/// How long ending a lease gives each harness still running under it.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ServeTimings {
    /// To leave by itself once its input is closed.
    pub(crate) grace: Duration,
    /// For each forced step after that.
    pub(crate) kill: Duration,
}

impl Default for ServeTimings {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(2),
            kill: Duration::from_secs(5),
        }
    }
}

/// Most a gateway's `Stop` may ask a harness to be waited on, for each step.
const MAX_STOP_WAIT: Duration = Duration::from_secs(60);
/// Most input chunks queued for one harness; past it the harness's input is
/// closed rather than bytes dropped from the middle of its protocol.
const INPUT_QUEUE: usize = 256;
/// Most frames queued for the gateway; harness output waits for room.
const OUTPUT_QUEUE: usize = 64;
/// Most drops one connection records; more are counted in the log.
const MAX_RECORDED_DROPS: u32 = 64;

/// Answer a gateway that cannot be served: the hello, so it learns this
/// build, then why, then nothing.
pub(crate) async fn refuse<W: AsyncWrite + Unpin>(
    mut output: W,
    hello: Hello,
    reason: Unavailability,
) -> io::Result<()> {
    write_frame(
        &mut output,
        &FromEnvironment::Hello {
            build: hello.build,
            workspace: hello.workspace,
        },
    )
    .await?;
    write_frame(&mut output, &FromEnvironment::Unavailable { reason }).await?;
    output.shutdown().await
}

/// Serve one gateway until its stream ends, then end every lease it held as
/// lost and return once each is recorded.
pub(crate) async fn serve<R, W>(
    input: R,
    output: W,
    build: &str,
    launcher: Arc<dyn HarnessLauncher>,
    ledger: Arc<dyn LeaseLedger>,
    timings: ServeTimings,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (frames, writer) = start_writer(output);
    let _ = frames
        .send(FromEnvironment::Hello {
            build: build.into(),
            workspace: launcher.workspace().into(),
        })
        .await;
    let mut served = Served {
        leases: HashMap::new(),
        granted: HashSet::new(),
        ending: Vec::new(),
        frames,
        launcher,
        ledger,
        timings,
        drops: 0,
    };
    let mut stream = FrameStream::new(input);
    loop {
        let body = match stream.next().await {
            Ok(Some(body)) => body,
            Ok(None) => break,
            Err(error) => {
                tracing::warn!(%error, "the gateway's lease stream could not be read; serving ends");
                break;
            }
        };
        match decode::<ToEnvironment>(&body) {
            Ok(frame) => served.handle(frame).await,
            Err(error) => {
                tracing::warn!(%error, "an unreadable lease frame was dropped");
                served.dropped(None, None, "unreadable");
            }
        }
    }
    served.lose_all().await;
    drop(served);
    let _ = writer.await;
}

fn start_writer<W>(mut output: W) -> (mpsc::Sender<FromEnvironment>, JoinHandle<()>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (sender, mut receiver) = mpsc::channel::<FromEnvironment>(OUTPUT_QUEUE);
    let writer = tokio::spawn(async move {
        while let Some(frame) = receiver.recv().await {
            if let Err(error) = write_frame(&mut output, &frame).await {
                tracing::warn!(%error, "a lease frame could not be written to the gateway");
                break;
            }
        }
        let _ = output.shutdown().await;
    });
    (sender, writer)
}

/// One harness running under a lease.
struct Channel {
    /// `None` once its input was closed.
    input: Option<mpsc::Sender<Vec<u8>>>,
    control: Box<dyn HarnessControl>,
    output: JoinHandle<()>,
}

/// One lease this connection holds.
struct Held {
    /// The agent it was granted for; every harness under it is that agent's.
    agent: String,
    channels: HashMap<u32, Channel>,
    /// Each harness the gateway asked to stop, by channel: its cleanup once
    /// known. A channel is never started again, and a second stop of it is
    /// answered with the same cleanup, never as holding nothing while the
    /// first is still stopping it.
    stopped: HashMap<u32, watch::Receiver<Option<Cleanup>>>,
    /// Stops already under way, each answering its harness's cleanup.
    stops: Vec<JoinHandle<Cleanup>>,
}

struct Served {
    leases: HashMap<String, Held>,
    /// Every lease id granted on this connection, ended ones included: an id
    /// is granted once.
    granted: HashSet<String>,
    /// Leases ending, to be waited for before serving returns.
    ending: Vec<JoinHandle<()>>,
    frames: mpsc::Sender<FromEnvironment>,
    launcher: Arc<dyn HarnessLauncher>,
    ledger: Arc<dyn LeaseLedger>,
    timings: ServeTimings,
    drops: u32,
}

impl Served {
    async fn handle(&mut self, frame: ToEnvironment) {
        match frame {
            ToEnvironment::Grant { lease, agent } => self.grant(lease, agent).await,
            ToEnvironment::Start {
                lease,
                channel,
                environment,
            } => self.start(lease, channel, &environment).await,
            ToEnvironment::Input {
                lease,
                channel,
                data,
            } => self.input(&lease, channel, data),
            ToEnvironment::InputClosed { lease, channel } => match self.channel(&lease, channel) {
                Some(held) => held.input = None,
                None => self.dropped(Some(&lease), Some(channel), "input_closed"),
            },
            ToEnvironment::Stop {
                lease,
                channel,
                grace_ms,
                kill_ms,
            } => {
                let grace = Duration::from_millis(grace_ms).min(MAX_STOP_WAIT);
                let kill = Duration::from_millis(kill_ms).min(MAX_STOP_WAIT);
                let held = self.leases.get_mut(&lease);
                let taken = held.and_then(|held| match held.channels.remove(&channel) {
                    Some(running) => Some(Ok(running)),
                    None => held.stopped.get(&channel).cloned().map(Err),
                });
                match taken {
                    Some(Ok(running)) => {
                        let (done, cleanup) = watch::channel(None);
                        let stop =
                            self.stop(lease.clone(), channel, running, grace, kill, Some(done));
                        if let Some(held) = self.leases.get_mut(&lease) {
                            held.stopped.insert(channel, cleanup);
                            held.stops.push(stop);
                        }
                    }
                    // Already stopping, or stopped: answered with what that
                    // took, once it is known.
                    Some(Err(mut cleanup)) => {
                        let frames = self.frames.clone();
                        tokio::spawn(async move {
                            let known = match cleanup.wait_for(Option::is_some).await {
                                Ok(known) => *known,
                                Err(_) => return,
                            };
                            let Some(cleanup) = known else { return };
                            let _ = frames
                                .send(FromEnvironment::Stopped {
                                    lease,
                                    channel,
                                    cleanup,
                                })
                                .await;
                        });
                    }
                    None => {
                        self.dropped(Some(&lease), Some(channel), "stop");
                        // Nothing runs as that harness here: said, so the
                        // gateway does not wait for an answer that never comes.
                        self.send(FromEnvironment::Stopped {
                            lease,
                            channel,
                            cleanup: Cleanup::NotHeld,
                        })
                        .await;
                    }
                }
            }
            ToEnvironment::End { lease } => match self.leases.remove(&lease) {
                Some(held) => {
                    let ending = self.end(lease, held, false);
                    self.ending.push(ending);
                }
                // Not held on this connection: what the audit says of it.
                None => {
                    let cleanup = self.accounted(&lease);
                    self.send(FromEnvironment::Ended { lease, cleanup }).await;
                }
            },
            ToEnvironment::Account { lease } => {
                let cleanup = self.accounted(&lease);
                self.send(FromEnvironment::Accounted { lease, cleanup })
                    .await;
            }
        }
    }

    async fn send(&mut self, frame: FromEnvironment) {
        let _ = self.frames.send(frame).await;
    }

    fn accounted(&self, lease: &str) -> Cleanup {
        self.ledger.accounted(lease).unwrap_or_else(|error| {
            tracing::error!(lease, %error, "the lease audit could not be read");
            Cleanup::Uncertain
        })
    }

    fn channel(&mut self, lease: &str, channel: u32) -> Option<&mut Channel> {
        self.leases
            .get_mut(lease)
            .and_then(|held| held.channels.get_mut(&channel))
    }

    fn dropped(&mut self, lease: Option<&str>, channel: Option<u32>, frame: &str) {
        self.drops = self.drops.saturating_add(1);
        tracing::warn!(
            lease,
            channel,
            frame,
            "a lease frame naming nothing held here was dropped"
        );
        if self.drops > MAX_RECORDED_DROPS {
            return;
        }
        let entry = LedgerEntry::Dropped {
            lease: lease.map(str::to_owned),
            channel,
            frame: frame.into(),
        };
        if let Err(error) = self.ledger.record(&entry) {
            tracing::error!(%error, "a dropped lease frame could not be recorded");
        }
    }

    async fn grant(&mut self, lease: String, agent: String) {
        let refusal = if LeaseId::new(lease.clone()).is_err() || self.granted.contains(&lease) {
            Some(GrantRefusal::Duplicate)
        } else if let Some(refusal) = self.recorded_before(&lease) {
            Some(refusal)
        } else if !self.launcher.runs(&agent) {
            Some(GrantRefusal::AgentUnavailable)
        } else {
            match self.ledger.record(&LedgerEntry::Granted {
                lease: lease.clone(),
                agent: agent.clone(),
            }) {
                Ok(()) => None,
                Err(error) => {
                    tracing::error!(lease, %error, "a lease could not be recorded; it is refused");
                    Some(GrantRefusal::AuditUnavailable)
                }
            }
        };
        match refusal {
            None => {
                self.granted.insert(lease.clone());
                self.leases.insert(
                    lease.clone(),
                    Held {
                        agent,
                        channels: HashMap::new(),
                        stopped: HashMap::new(),
                        stops: Vec::new(),
                    },
                );
                self.send(FromEnvironment::Granted { lease }).await;
            }
            Some(reason) => {
                if reason != GrantRefusal::AuditUnavailable {
                    let entry = LedgerEntry::Refused {
                        lease: lease.clone(),
                        reason,
                    };
                    if let Err(error) = self.ledger.record(&entry) {
                        tracing::error!(lease, %error, "a refused lease could not be recorded");
                    }
                }
                self.send(FromEnvironment::Refused { lease, reason }).await;
            }
        }
    }

    /// Why a lease id this host's audit already records, from this
    /// connection or an earlier one, is not granted again: a lease is
    /// granted once, and its id never names a second one.
    fn recorded_before(&self, lease: &str) -> Option<GrantRefusal> {
        match self.ledger.accounted(lease) {
            Ok(Cleanup::NotHeld) => None,
            Ok(_) => Some(GrantRefusal::Duplicate),
            Err(error) => {
                tracing::error!(lease, %error, "the lease audit could not be read; the lease is refused");
                Some(GrantRefusal::AuditUnavailable)
            }
        }
    }

    async fn start(&mut self, lease: String, channel: u32, environment: &BTreeMap<String, String>) {
        let free = self.leases.get(&lease).is_some_and(|held| {
            !held.channels.contains_key(&channel) && !held.stopped.contains_key(&channel)
        });
        if !free {
            self.dropped(Some(&lease), Some(channel), "start");
            return self
                .start_failed(lease, channel, StartFailure::NotGranted)
                .await;
        }
        let entry = LedgerEntry::Started {
            lease: lease.clone(),
            channel,
        };
        if let Err(error) = self.ledger.record(&entry) {
            tracing::error!(lease, channel, %error, "a harness start could not be recorded; nothing started");
            return self
                .start_failed(lease, channel, StartFailure::AuditUnavailable)
                .await;
        }
        let agent = self
            .leases
            .get(&lease)
            .map(|held| held.agent.clone())
            .unwrap_or_default();
        let process = match self.launcher.launch(&agent, environment) {
            Ok(process) => process,
            Err(error) => {
                tracing::warn!(lease, channel, %error, "a harness did not start");
                let entry = LedgerEntry::StartFailed {
                    lease: lease.clone(),
                    channel,
                };
                if let Err(error) = self.ledger.record(&entry) {
                    tracing::error!(lease, channel, %error, "a failed harness start could not be recorded");
                }
                return self
                    .start_failed(lease, channel, StartFailure::SpawnFailed)
                    .await;
            }
        };
        let HarnessProcess {
            input,
            output,
            control,
        } = process;
        let (sender, receiver) = mpsc::channel(INPUT_QUEUE);
        tokio::spawn(pump_input(input, receiver));
        let output = tokio::spawn(pump_output(
            output,
            lease.clone(),
            channel,
            self.frames.clone(),
        ));
        if let Some(held) = self.leases.get_mut(&lease) {
            held.channels.insert(
                channel,
                Channel {
                    input: Some(sender),
                    control,
                    output,
                },
            );
        }
    }

    async fn start_failed(&mut self, lease: String, channel: u32, reason: StartFailure) {
        self.send(FromEnvironment::StartFailed {
            lease,
            channel,
            reason,
        })
        .await;
    }

    fn input(&mut self, lease: &str, channel: u32, data: Data) {
        let Some(running) = self.channel(lease, channel) else {
            return self.dropped(Some(lease), Some(channel), "input");
        };
        let Some(input) = &running.input else {
            return self.dropped(Some(lease), Some(channel), "input");
        };
        if input.try_send(data.0).is_err() {
            // Full, or the harness stopped reading for good: its input ends
            // rather than losing bytes from the middle of its protocol.
            running.input = None;
            self.dropped(Some(lease), Some(channel), "input_overflow");
        }
    }

    /// Stop one harness on a task of its own, recording what that took and,
    /// when the gateway asked for it (`done`), answering it and keeping it
    /// for a second ask.
    fn stop(
        &self,
        lease: String,
        channel: u32,
        running: Channel,
        grace: Duration,
        kill: Duration,
        done: Option<watch::Sender<Option<Cleanup>>>,
    ) -> JoinHandle<Cleanup> {
        let ledger = self.ledger.clone();
        let frames = self.frames.clone();
        tokio::spawn(async move {
            let Channel {
                input,
                mut control,
                mut output,
            } = running;
            drop(input);
            let cleanup = match control.cleanup(grace, kill).await {
                Ok(outcome) => Cleanup::Confirmed {
                    forced: outcome.forced,
                },
                Err(error) => {
                    tracing::error!(lease, channel, %error, "a harness could not be confirmed stopped");
                    Cleanup::Uncertain
                }
            };
            // Its process tree is gone, so its output ends; past the bound
            // the pump is stopped, and nothing more is read from it.
            if tokio::time::timeout(kill, &mut output).await.is_err() {
                output.abort();
            }
            let entry = LedgerEntry::Stopped {
                lease: lease.clone(),
                channel,
                cleanup,
            };
            let cleanup = match ledger.record(&entry) {
                Ok(()) => cleanup,
                Err(error) => {
                    tracing::error!(lease, channel, %error, "a harness's stop could not be recorded");
                    Cleanup::Uncertain
                }
            };
            if let Some(done) = done {
                done.send_replace(Some(cleanup));
                let _ = frames
                    .send(FromEnvironment::Stopped {
                        lease,
                        channel,
                        cleanup,
                    })
                    .await;
            }
            cleanup
        })
    }

    /// End `lease` on a task of its own: stop every harness still running
    /// under it, wait for the stops already under way, record what all of it
    /// took and, unless it was lost with the connection, answer it.
    fn end(&self, lease: String, held: Held, lost: bool) -> JoinHandle<()> {
        let mut stops = held.stops;
        for (channel, running) in held.channels {
            stops.push(self.stop(
                lease.clone(),
                channel,
                running,
                self.timings.grace,
                self.timings.kill,
                None,
            ));
        }
        let ledger = self.ledger.clone();
        let frames = self.frames.clone();
        tokio::spawn(async move {
            let mut cleanup = Cleanup::Confirmed { forced: false };
            for stop in stops {
                let stopped = stop.await.unwrap_or(Cleanup::Uncertain);
                cleanup = combined(cleanup, stopped);
            }
            let entry = LedgerEntry::Ended {
                lease: lease.clone(),
                cleanup,
                lost,
            };
            let cleanup = match ledger.record(&entry) {
                Ok(()) => cleanup,
                Err(error) => {
                    tracing::error!(lease, %error, "a lease's end could not be recorded");
                    Cleanup::Uncertain
                }
            };
            if !lost {
                let _ = frames.send(FromEnvironment::Ended { lease, cleanup }).await;
            }
        })
    }

    /// The gateway is gone: end every lease it held as lost, and wait for
    /// every end under way.
    async fn lose_all(&mut self) {
        let leases: Vec<_> = self.leases.drain().collect();
        for (lease, held) in leases {
            tracing::warn!(lease, "the gateway's connection ended; its lease is lost");
            let ending = self.end(lease, held, true);
            self.ending.push(ending);
        }
        for ending in self.ending.drain(..) {
            let _ = ending.await;
        }
    }
}

/// What two harnesses' cleanups make together: confirmed only if both are.
fn combined(left: Cleanup, right: Cleanup) -> Cleanup {
    match (left, right) {
        (Cleanup::Uncertain, _) | (_, Cleanup::Uncertain) => Cleanup::Uncertain,
        (left, right) => Cleanup::Confirmed {
            forced: forced(left) || forced(right),
        },
    }
}

fn forced(cleanup: Cleanup) -> bool {
    matches!(cleanup, Cleanup::Confirmed { forced: true })
}

async fn pump_input(
    mut input: Box<dyn AsyncWrite + Send + Unpin>,
    mut receiver: mpsc::Receiver<Vec<u8>>,
) {
    while let Some(bytes) = receiver.recv().await {
        if input.write_all(&bytes).await.is_err() || input.flush().await.is_err() {
            return;
        }
    }
    let _ = input.shutdown().await;
}

async fn pump_output(
    mut output: Box<dyn AsyncRead + Send + Unpin>,
    lease: String,
    channel: u32,
    frames: mpsc::Sender<FromEnvironment>,
) {
    let mut buffer = vec![0; MAX_DATA_BYTES];
    loop {
        match output.read(&mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => {
                let frame = FromEnvironment::Output {
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
        .send(FromEnvironment::OutputClosed { lease, channel })
        .await;
}

#[cfg(test)]
#[path = "../../../tests/env_serve/serve.rs"]
mod tests;
