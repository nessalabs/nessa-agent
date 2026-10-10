//! The environment role: serve one gateway's leases over a byte stream
//! (issue #699).
//!
//! ```text
//! gateway ──frames──▶ serve ──Grant──▶ ledger: granted ──▶ Granted
//!                          ──Start──▶ ledger: started ──▶ HarnessLauncher::launch
//!                                         input pump ──▶ harness stdin
//!                                         harness stdout ──▶ output pump ──▶ Output
//!                          ──Stop───▶ cleanup(grace, kill) ──▶ ledger: stopped ──▶ Stopped
//!                          ──Input──▶ input queue full, or the harness gone deaf
//!                                     ──▶ ledger: input_overflow ──▶ stop ──▶ Stopped (unasked)
//!                          ──End────▶ stop every harness ──▶ ledger: ended ──▶ Ended
//!                          ──Account▶ ledger ──▶ Accounted
//!                          ──GrantCommand──▶ ledger: command_granted ──▶ Granted
//!                          ──Run──▶ ledger: command_started ──▶ CommandRunner::run
//!                                   ──▶ ledger: command_ran, ended ──▶ Ran
//!                          ──End (a command running)──▶ stop it ──▶ Ran ──▶ Ended
//!                          ──Keepalive▶ (nothing: the gateway is there)
//! end of stream, or nothing read for `silence` ──▶ every lease ended as lost
//!                                                ──▶ ledger: ended(lost)
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
//! connection learns it through `Account`. So is a connection on which
//! nothing at all arrived for `silence`, past the gateway's keepalives: a
//! network that went away ends no stream, and this side would otherwise go
//! on holding the serving lock and running harnesses nobody supervises.
//!
//! A command lease runs one command once, through [`CommandRunner`], only on
//! a host configured to run commands; its `Ran` ends it. Its command is
//! checked here again, as a [`CommandWork`]: the gateway's word for it is
//! not taken.
//!
//! A lease's end is answered by that end alone: an `Account`, or another
//! `End`, of a lease whose end is still under way waits for it, rather than
//! reading a ledger that does not hold it yet.
use super::wire::{write_frame, FrameStream};
use nessa_protocol::lease::{
    decode, Cleanup, CommandEnd, Data, FromEnvironment, GrantRefusal, Hello, StartFailure,
    ToEnvironment, Unavailability, MAX_DATA_BYTES, MAX_STOP_WAIT, SILENCE_LIMIT,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{HarnessControl, HarnessProcess},
};
use nessa_sdk::domain::agent_execution::leases::{CommandWork, LeaseId};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet},
    future::Future,
    io,
    pin::Pin,
    sync::Arc,
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
    task::JoinHandle,
    time::Instant,
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

/// How the host runs a command lease's command: in its workspace, as the
/// account it serves as, with nothing enclosing it.
pub(crate) trait CommandRunner: Send + Sync {
    /// Run `command` until it ends, its timeout passes, or `stop` is said,
    /// and answer how it ended, what of its output was kept, and what
    /// releasing it took. Answers within its timeout and the stop's bounds.
    fn run(
        &self,
        command: CommandWork,
        stop: watch::Receiver<Option<CommandStop>>,
    ) -> Pin<Box<dyn Future<Output = CommandRan> + Send + 'static>>;
}

/// Why a running command is stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandStop {
    /// The gateway ended its lease.
    Ended,
    /// The gateway's connection ended.
    Lost,
}

/// What running one command came to.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CommandRan {
    pub(crate) end: CommandEnd,
    /// The newest of its standard output kept; with `stderr`, at most
    /// [`MAX_DATA_BYTES`].
    pub(crate) stdout: Vec<u8>,
    pub(crate) stderr: Vec<u8>,
    /// Bytes it printed past what was kept, the oldest.
    pub(crate) dropped_bytes: u64,
    pub(crate) cleanup: Cleanup,
}

impl CommandRan {
    /// A command that never started: nothing ran, nothing is held.
    pub(crate) fn not_started() -> Self {
        Self {
            end: CommandEnd::NotStarted,
            stdout: Vec::new(),
            stderr: Vec::new(),
            dropped_bytes: 0,
            cleanup: Cleanup::NotHeld,
        }
    }
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
    /// A harness stopped reading its input until its queue was full, or for
    /// good: it is stopped rather than fed a stream with bytes missing.
    InputOverflow { lease: String, channel: u32 },
    /// A command lease was admitted to run `argv` once.
    CommandGranted {
        lease: String,
        argv: Vec<String>,
        cwd: Option<String>,
        timeout_ms: u64,
    },
    /// A command lease's command is about to start.
    CommandStarted { lease: String },
    /// A command lease's command ended; its output is the gateway's to keep.
    CommandRan {
        lease: String,
        end: CommandEnd,
        dropped_bytes: u64,
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

/// How long ending a lease gives each harness still running under it, and
/// how long a silent gateway is believed.
#[derive(Clone, Copy, Debug)]
pub(crate) struct ServeTimings {
    /// To leave by itself once its input is closed.
    pub(crate) grace: Duration,
    /// For each forced step after that.
    pub(crate) kill: Duration,
    /// Nothing read from the gateway for this long, keepalives included, is
    /// the gateway gone.
    pub(crate) silence: Duration,
}

impl Default for ServeTimings {
    fn default() -> Self {
        Self {
            grace: Duration::from_secs(2),
            kill: Duration::from_secs(5),
            silence: SILENCE_LIMIT,
        }
    }
}
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
            protocol: hello.protocol,
            workspace: hello.workspace,
        },
    )
    .await?;
    write_frame(&mut output, &FromEnvironment::Unavailable { reason }).await?;
    output.shutdown().await
}

/// What this host runs for a gateway: its agents' harnesses, and commands
/// when it serves them.
#[derive(Clone)]
pub(crate) struct Runners {
    pub(crate) harnesses: Arc<dyn HarnessLauncher>,
    /// `None` refuses every command lease `commands_unavailable`.
    pub(crate) commands: Option<Arc<dyn CommandRunner>>,
}

/// Serve one gateway until its stream ends, then end every lease it held as
/// lost and return once each is recorded.
pub(crate) async fn serve<R, W>(
    input: R,
    output: W,
    build: &str,
    protocol: &str,
    runners: Runners,
    ledger: Arc<dyn LeaseLedger>,
    timings: ServeTimings,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let Runners {
        harnesses: launcher,
        commands,
    } = runners;
    let (frames, mut writer) = start_writer(output);
    let _ = frames
        .send(FromEnvironment::Hello {
            build: build.into(),
            protocol: protocol.into(),
            workspace: launcher.workspace().into(),
        })
        .await;
    let mut served = Served {
        leases: HashMap::new(),
        granted: HashSet::new(),
        ends: HashMap::new(),
        commands: HashMap::new(),
        runner: commands,
        frames,
        launcher,
        ledger,
        timings,
        drops: 0,
    };
    let mut stream = FrameStream::new(input);
    // Nothing read for `silence` is the gateway gone, whether this side is
    // waiting for its next frame or is held up answering one: a frame this
    // side cannot send is a gateway not reading, which a gateway always does.
    let mut heard = Instant::now();
    loop {
        let silent = heard + timings.silence;
        // `next` keeps what a cancelled read had read, so the bound costs
        // nothing of a frame arriving slowly.
        let body = match tokio::time::timeout_at(silent, stream.next()).await {
            Ok(Ok(Some(body))) => body,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(%error, "the gateway's lease stream could not be read; serving ends");
                break;
            }
            Err(_) => {
                served.silent(timings.silence);
                break;
            }
        };
        heard = Instant::now();
        let handled = async {
            match decode::<ToEnvironment>(&body) {
                Ok(frame) => served.handle(frame).await,
                Err(error) => {
                    tracing::warn!(%error, "an unreadable lease frame was dropped");
                    served.dropped(None, None, "unreadable");
                }
            }
        };
        // Every state change a frame makes comes before the answer it waits
        // to send, so one given up on here leaves nothing half-applied.
        if tokio::time::timeout_at(heard + timings.silence, handled)
            .await
            .is_err()
        {
            served.silent(timings.silence);
            break;
        }
    }
    served.lose_all().await;
    drop(served);
    // What is still queued for a gateway that reads nothing is given up on,
    // so serving, and the lock it holds, always ends.
    if tokio::time::timeout(timings.silence, &mut writer)
        .await
        .is_err()
    {
        writer.abort();
    }
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
    /// Writes what its input queue holds, then the end of it, to the harness.
    pump: JoinHandle<()>,
    control: Box<dyn HarnessControl>,
    output: JoinHandle<()>,
}

/// One harness's stop under way: its task, which answers the gateway when
/// it asked, and its cleanup once recorded.
struct Stopping {
    task: JoinHandle<()>,
    cleanup: watch::Receiver<Option<Cleanup>>,
}

/// One harness's stop, as asked.
struct Stop {
    lease: String,
    channel: u32,
    /// How long it may take to leave by itself.
    grace: Duration,
    /// How long each forced step may take.
    kill: Duration,
    /// Whether the gateway is told what it took with a `Stopped`.
    answer: bool,
    /// Whether what made it stop was recorded; unrecorded, its cleanup is
    /// answered uncertain.
    recorded: bool,
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
    stops: Vec<Stopping>,
}

/// One command lease this connection holds.
enum HeldCommand {
    /// Granted, its command not yet run.
    Granted(CommandWork),
    /// Its command started: `stop` stops it, and `ran` is its cleanup once
    /// its end is recorded.
    Running {
        stop: watch::Sender<Option<CommandStop>>,
        ran: watch::Receiver<Option<Cleanup>>,
    },
}

impl HeldCommand {
    /// Whether its command is running still: started, its end not recorded.
    fn runs(&self) -> bool {
        matches!(self, Self::Running { ran, .. } if ran.borrow().is_none())
    }
}

struct Served {
    leases: HashMap<String, Held>,
    /// Every lease id granted on this connection, ended ones included: an id
    /// is granted once.
    granted: HashSet<String>,
    /// Each lease whose end is under way on this connection: its cleanup
    /// once recorded. What answers an account or end of it meanwhile, and
    /// what serving waits for before it returns.
    ends: HashMap<String, watch::Receiver<Option<Cleanup>>>,
    /// The command leases granted on this connection and not yet ended by
    /// the gateway, by id.
    commands: HashMap<String, HeldCommand>,
    /// How this host runs commands; `None` when it is not configured to.
    runner: Option<Arc<dyn CommandRunner>>,
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
                        let stop = self.stop(
                            Stop {
                                lease: lease.clone(),
                                channel,
                                grace,
                                kill,
                                answer: true,
                                recorded: true,
                            },
                            running,
                        );
                        if let Some(held) = self.leases.get_mut(&lease) {
                            held.stopped.insert(channel, stop.cleanup.clone());
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
            ToEnvironment::GrantCommand {
                lease,
                argv,
                cwd,
                timeout_ms,
            } => self.grant_command(lease, argv, cwd, timeout_ms).await,
            ToEnvironment::Run { lease } => self.run(lease).await,
            ToEnvironment::End { lease } if self.commands.contains_key(&lease) => {
                self.end_command(lease).await;
            }
            ToEnvironment::End { lease } => match self.leases.remove(&lease) {
                Some(held) => self.end(lease, held, false),
                // Not held on this connection: what its end under way, or
                // the audit, says of it.
                None => {
                    self.answer_ended(lease, |lease, cleanup| FromEnvironment::Ended {
                        lease,
                        cleanup,
                    })
                    .await;
                }
            },
            ToEnvironment::Account { lease } => {
                self.answer_ended(lease, |lease, cleanup| FromEnvironment::Accounted {
                    lease,
                    cleanup,
                })
                .await;
            }
            ToEnvironment::Keepalive => {}
        }
    }

    /// Answer what became of `lease`, a lease this connection no longer
    /// holds: once its end under way here is recorded, with what that took,
    /// or else from the audit.
    async fn answer_ended(
        &mut self,
        lease: String,
        answer: impl FnOnce(String, Cleanup) -> FromEnvironment + Send + 'static,
    ) {
        let Some(mut ending) = self.ends.get(&lease).cloned() else {
            let cleanup = self.accounted(&lease);
            return self.send(answer(lease, cleanup)).await;
        };
        let frames = self.frames.clone();
        tokio::spawn(async move {
            // An end that never said (its task failed) is not known done.
            let cleanup = match ending.wait_for(Option::is_some).await {
                Ok(cleanup) => (*cleanup).unwrap_or(Cleanup::Uncertain),
                Err(_) => Cleanup::Uncertain,
            };
            let _ = frames.send(answer(lease, cleanup)).await;
        });
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
        let pump = tokio::spawn(pump_input(input, receiver));
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
                    pump,
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
        if input.try_send(data.0).is_ok() {
            return;
        }
        // Full, or the harness stopped reading for good. Its stream would go
        // on with these bytes missing, or end short, so the harness is
        // stopped instead, and the gateway told with its `Stopped`, as for a
        // stop it asked: its channel fails, never quietly truncated.
        let Some(running) = self
            .leases
            .get_mut(lease)
            .and_then(|held| held.channels.remove(&channel))
        else {
            return;
        };
        let entry = LedgerEntry::InputOverflow {
            lease: lease.into(),
            channel,
        };
        let recorded = match self.ledger.record(&entry) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(lease, channel, %error, "a harness's input overflow could not be recorded");
                false
            }
        };
        let stop = self.stop(
            Stop {
                lease: lease.into(),
                channel,
                grace: Duration::ZERO,
                kill: self.timings.kill,
                answer: true,
                recorded,
            },
            running,
        );
        if let Some(held) = self.leases.get_mut(lease) {
            held.stopped.insert(channel, stop.cleanup.clone());
            held.stops.push(stop);
        }
    }

    /// Stop one harness on a task of its own, recording what that took and,
    /// when the gateway is to be told (`answer`), telling it.
    fn stop(&self, stop: Stop, running: Channel) -> Stopping {
        let Stop {
            lease,
            channel,
            grace,
            kill,
            answer,
            recorded,
        } = stop;
        let ledger = self.ledger.clone();
        let frames = self.frames.clone();
        let (done, cleanup) = watch::channel(None);
        let task = tokio::spawn(async move {
            let Channel {
                input,
                mut pump,
                mut control,
                mut output,
            } = running;
            // What the harness's input already accepted, and the end of it,
            // reach it before its grace begins, so it is never stopped for
            // input still on its way. Bounded by the grace itself: a harness
            // that does not read is not waited on longer (`stop_steps`).
            drop(input);
            if tokio::time::timeout(grace, &mut pump).await.is_err() {
                pump.abort();
            }
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
                // What made it stop went unrecorded: it settles nothing.
                Ok(()) if recorded => cleanup,
                Ok(()) => Cleanup::Uncertain,
                Err(error) => {
                    tracing::error!(lease, channel, %error, "a harness's stop could not be recorded");
                    Cleanup::Uncertain
                }
            };
            done.send_replace(Some(cleanup));
            if answer {
                let _ = frames
                    .send(FromEnvironment::Stopped {
                        lease,
                        channel,
                        cleanup,
                    })
                    .await;
            }
        });
        Stopping { task, cleanup }
    }

    /// End `lease` on a task of its own: stop every harness still running
    /// under it, wait for the stops already under way, record what all of it
    /// took and, unless it was lost with the connection, answer it after
    /// every answer those stops send.
    fn end(&mut self, lease: String, held: Held, lost: bool) {
        let (done, ended) = watch::channel(None);
        // Ends already recorded are the audit's to answer from.
        self.ends.retain(|_, ended| ended.borrow().is_none());
        self.ends.insert(lease.clone(), ended);
        let mut stops = held.stops;
        for (channel, running) in held.channels {
            stops.push(self.stop(
                Stop {
                    lease: lease.clone(),
                    channel,
                    grace: self.timings.grace,
                    kill: self.timings.kill,
                    answer: false,
                    recorded: true,
                },
                running,
            ));
        }
        let ledger = self.ledger.clone();
        let frames = self.frames.clone();
        tokio::spawn(async move {
            let mut cleanup = Cleanup::Confirmed { forced: false };
            for stop in &mut stops {
                // A stop whose task failed before recording is not known done.
                let stopped = match stop.cleanup.wait_for(Option::is_some).await {
                    Ok(stopped) => (*stopped).unwrap_or(Cleanup::Uncertain),
                    Err(_) => Cleanup::Uncertain,
                };
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
            done.send_replace(Some(cleanup));
            if !lost {
                for stop in stops {
                    let _ = stop.task.await;
                }
                let _ = frames.send(FromEnvironment::Ended { lease, cleanup }).await;
            }
        });
    }

    /// Admit a command lease, or refuse it, as [`Self::grant`] does a
    /// harness's: only on a host that runs commands, and only for a command
    /// a command lease can hold.
    async fn grant_command(
        &mut self,
        lease: String,
        argv: Vec<String>,
        cwd: Option<String>,
        timeout_ms: u64,
    ) {
        // Commands whose end is recorded are the audit's to answer for.
        self.commands.retain(|_, held| match held {
            HeldCommand::Granted(_) => true,
            HeldCommand::Running { ran, .. } => ran.borrow().is_none(),
        });
        let work = CommandWork::new(argv.clone(), cwd.clone(), timeout_ms);
        let refusal = if LeaseId::new(lease.clone()).is_err() || self.granted.contains(&lease) {
            Err(GrantRefusal::Duplicate)
        } else if self.runner.is_none() {
            Err(GrantRefusal::CommandsUnavailable)
        } else if let Some(refusal) = self.recorded_before(&lease) {
            Err(refusal)
        } else {
            match work {
                Err(_) => Err(GrantRefusal::InvalidCommand),
                Ok(work) => match self.ledger.record(&LedgerEntry::CommandGranted {
                    lease: lease.clone(),
                    argv,
                    cwd,
                    timeout_ms,
                }) {
                    Ok(()) => Ok(work),
                    Err(error) => {
                        tracing::error!(lease, %error, "a command lease could not be recorded; it is refused");
                        Err(GrantRefusal::AuditUnavailable)
                    }
                },
            }
        };
        match refusal {
            Ok(work) => {
                self.granted.insert(lease.clone());
                self.commands
                    .insert(lease.clone(), HeldCommand::Granted(work));
                self.send(FromEnvironment::Granted { lease }).await;
            }
            Err(reason) => {
                if reason != GrantRefusal::AuditUnavailable {
                    let entry = LedgerEntry::Refused {
                        lease: lease.clone(),
                        reason,
                    };
                    if let Err(error) = self.ledger.record(&entry) {
                        tracing::error!(lease, %error, "a refused command lease could not be recorded");
                    }
                }
                self.send(FromEnvironment::Refused { lease, reason }).await;
            }
        }
    }

    /// Run a granted command lease's command, once, on a task of its own,
    /// which records how it ended and answers with its `Ran`. A lease not
    /// granted here, or run already, is answered as never started.
    async fn run(&mut self, lease: String) {
        let work = match self.commands.remove(&lease) {
            Some(HeldCommand::Granted(work)) => work,
            // Still running: its own `Ran` will answer.
            Some(running @ HeldCommand::Running { .. }) if running.runs() => {
                self.commands.insert(lease.clone(), running);
                return self.dropped(Some(&lease), None, "run");
            }
            held => {
                if let Some(held) = held {
                    self.commands.insert(lease.clone(), held);
                }
                // Never granted here, or run already: nothing runs, and that
                // is said, so the gateway does not wait for an answer.
                self.dropped(Some(&lease), None, "run");
                return self.send(ran(lease, CommandRan::not_started())).await;
            }
        };
        let Some(runner) = self.runner.clone() else {
            unreachable!("a command lease is granted only on a host that runs commands");
        };
        let (stop, stopped) = watch::channel(None);
        let (done, ran_receiver) = watch::channel(None);
        let entry = LedgerEntry::CommandStarted {
            lease: lease.clone(),
        };
        let started = match self.ledger.record(&entry) {
            Ok(()) => Some(runner.run(work, stopped.clone())),
            Err(error) => {
                tracing::error!(lease, %error, "a command's start could not be recorded; it is not run");
                None
            }
        };
        self.commands.insert(
            lease.clone(),
            HeldCommand::Running {
                stop,
                ran: ran_receiver,
            },
        );
        let ledger = self.ledger.clone();
        let frames = self.frames.clone();
        tokio::spawn(async move {
            let mut outcome = match started {
                Some(running) => running.await,
                None => CommandRan::not_started(),
            };
            let lost = *stopped.borrow() == Some(CommandStop::Lost);
            // Its end is written after its exit, and says uncertain once the
            // exit could not be written, so the ledger never reads confirmed
            // over a missing record.
            let ran_entry = LedgerEntry::CommandRan {
                lease: lease.clone(),
                end: outcome.end,
                dropped_bytes: outcome.dropped_bytes,
            };
            if let Err(error) = ledger.record(&ran_entry) {
                tracing::error!(lease, %error, "a command's exit could not be recorded");
                outcome.cleanup = Cleanup::Uncertain;
            }
            let ended_entry = LedgerEntry::Ended {
                lease: lease.clone(),
                cleanup: outcome.cleanup,
                lost,
            };
            if let Err(error) = ledger.record(&ended_entry) {
                tracing::error!(lease, %error, "a command's end could not be recorded");
                outcome.cleanup = Cleanup::Uncertain;
            }
            let cleanup = outcome.cleanup;
            // Its answer goes before anything that waits for its end, so a
            // gateway's `End` of it is answered after its `Ran`.
            if !lost {
                let _ = frames.send(ran(lease, outcome)).await;
            }
            done.send_replace(Some(cleanup));
        });
    }

    /// End a command lease: stop its command if it runs, and answer its end
    /// once that is recorded; one never run ends holding nothing.
    async fn end_command(&mut self, lease: String) {
        match self.commands.remove(&lease) {
            Some(HeldCommand::Running { stop, ran }) => {
                ask_stop(&stop, CommandStop::Ended);
                self.ends.retain(|_, ended| ended.borrow().is_none());
                self.ends.insert(lease.clone(), ran);
                self.answer_ended(lease, |lease, cleanup| FromEnvironment::Ended {
                    lease,
                    cleanup,
                })
                .await;
            }
            Some(HeldCommand::Granted(_)) => {
                let cleanup = self.end_unrun(&lease, false);
                self.send(FromEnvironment::Ended { lease, cleanup }).await;
            }
            None => unreachable!("only a held command lease is ended here"),
        }
    }

    /// Record the end of a command lease whose command never ran.
    fn end_unrun(&self, lease: &str, lost: bool) -> Cleanup {
        let entry = LedgerEntry::Ended {
            lease: lease.into(),
            cleanup: Cleanup::NotHeld,
            lost,
        };
        match self.ledger.record(&entry) {
            Ok(()) => Cleanup::NotHeld,
            Err(error) => {
                tracing::error!(lease, %error, "a command lease's end could not be recorded");
                Cleanup::Uncertain
            }
        }
    }

    /// The gateway is gone: end every lease it held as lost, and wait for
    /// every end under way.
    async fn lose_all(&mut self) {
        let leases: Vec<_> = self.leases.drain().collect();
        for (lease, held) in leases {
            tracing::warn!(lease, "the gateway's connection ended; its lease is lost");
            self.end(lease, held, true);
        }
        let commands: Vec<_> = self.commands.drain().collect();
        for (lease, held) in commands {
            match held {
                HeldCommand::Running { stop, ran } => {
                    ask_stop(&stop, CommandStop::Lost);
                    self.ends.insert(lease, ran);
                }
                HeldCommand::Granted(_) => {
                    self.end_unrun(&lease, true);
                }
            }
        }
        // Each end is waited for until it is recorded, not until its answer
        // is sent: a gateway that reads nothing more is never waited on.
        for (_, mut ended) in self.ends.drain() {
            let _ = ended.wait_for(Option::is_some).await;
        }
    }

    /// The gateway said nothing for `silence`.
    fn silent(&self, silence: Duration) {
        tracing::warn!(
            silence_ms = silence.as_millis(),
            leases = self.leases.len(),
            "nothing arrived from the gateway, not even a keepalive; it is taken as gone"
        );
    }
}

/// Stop a running command for `why`, unless it was already asked to stop.
fn ask_stop(stop: &watch::Sender<Option<CommandStop>>, why: CommandStop) {
    stop.send_if_modified(|asked| match asked {
        None => {
            *asked = Some(why);
            true
        }
        Some(_) => false,
    });
}

/// A command lease's `Ran`.
fn ran(lease: String, ran: CommandRan) -> FromEnvironment {
    FromEnvironment::Ran {
        lease,
        end: ran.end,
        stdout: Data(ran.stdout),
        stderr: Data(ran.stderr),
        dropped_bytes: ran.dropped_bytes,
        cleanup: ran.cleanup,
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
