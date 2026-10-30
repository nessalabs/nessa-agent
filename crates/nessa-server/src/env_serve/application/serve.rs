//! The environment role: serve one gateway's leases over a byte stream
//! (issue #699).
//!
//! ```text
//! gateway ──frames──▶ serve ──Grant──▶ the lease policy allows it? (PeerGrants, read now,
//!                                     then LeasePolicyEvaluator)
//!                                     the host runs the agent, in the sandbox asked?
//!                                     ──▶ ledger: granted{sandbox, leaseMs} ──▶ Granted{terms}
//!                                     ──▶ otherwise ledger: refused ──▶ Refused
//!                          ──Start──▶ ledger: started ──▶ HarnessLauncher::launch
//!                                         input pump ──▶ harness stdin
//!                                         harness stdout ──▶ output pump ──▶ Output
//!                          ──Stop───▶ cleanup(grace, kill) ──▶ ledger: stopped ──▶ Stopped
//!                          ──Input──▶ input queue full, or the harness gone deaf
//!                                     ──▶ ledger: input_overflow ──▶ stop ──▶ Stopped (unasked)
//!                          ──End────▶ stop every harness ──▶ ledger: ended ──▶ Ended
//!                          ──Account▶ ledger ──▶ Accounted
//!                          ──GrantCommand──▶ the lease policy allows commands? the host
//!                                            runs them? timeout narrowed to its most
//!                                            ──▶ ledger: command_granted ──▶ Granted{terms}
//!                          ──Run──▶ ledger: command_started ──▶ CommandRunner::run
//!                                   ──▶ ledger: command_ran, ended ──▶ Ran
//!                          ──End (a command running)──▶ stop it ──▶ Ran ──▶ Ended
//!                          ──Keepalive▶ (nothing: the gateway is there)
//!                          ──Collected▶ the lease's artifact answered (artifacts.rs)
//!                          ──Publish──▶ ledger: publish_asked ──▶ as a harness's publish
//!                                       ──▶ Published{request} | NotPublished{request}
//!                          ──Fetch──▶ the lease's fetches (fetches.rs; a host
//!                                     taking no files refuses each, recorded)
//!                                     ──▶ FetchAccepted | FetchRefused | Fetched
//!                          ──Delivered / Undelivered──▶ the lease's fetches
//!                          ──ReadArtifact──▶ a staged copy waiting for its Collected?
//!                                            ──▶ ArtifactRead | ArtifactNotRead (artifacts.rs)
//! a harness publishes ──▶ staged copy ──▶ ledger: published ──▶ Published
//! a lease lasts its granted leaseMs ──▶ ledger: expiring ──▶ stop every harness
//!     ──▶ ledger: ended(expired) ──▶ Ending{expired}; the gateway's End is
//!     answered with what that took
//!                          ──Revoked──▶ for each lease held: ledger: revoking
//!                                       ──▶ stop everything under it ──▶ ledger: ended(revoked);
//!                                       nothing more admitted; each `End` answered with that
//! drain asked (the host is stopping) ──▶ Draining, before anything else
//!     ──▶ for each lease held: ledger: draining
//!     ──▶ stop everything under it ──▶ ledger: ended(drained) ──▶ Ending{drained};
//!     nothing more admitted; each `End` answered with that; once nothing is held
//!     and every lease's End is answered, serving ends and the connection closes
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
//! Each lease is admitted under the grant this host's owner gave the peer
//! serving is for ([`Peer`], given by composition, never by the gateway),
//! read through [`PeerGrants`] at every grant and decided by the
//! [`LeasePolicyEvaluator`] over it: what the host can run is its
//! configuration's, and the grant only narrows it, refusing `not_granted`.
//! The grant is checked first, so a peer not granted a lease learns nothing
//! of which agents the host runs, or whether it runs commands.
//! A command's timeout is granted as asked or shorter, and it runs with the
//! timeout granted. A harness's sandbox is one the host can enforce, as
//! asked, or the lease is refused `sandbox_unavailable`: none other is put
//! in its place. What was granted is recorded, and is what `Granted` says.
//!
//! A harness's lease may last at most the peer's grant's `maxLeaseMs`, from
//! its grant: `Granted` says so (`leaseMs`), and once it has lasted that
//! long, measured on the serving [`Timer`], this side ends it itself, as
//! the one authority on it, and says so with `Ending`. A deadline passing is
//! not the gateway heard from. A command lease is bounded by its own
//! timeout instead.
//!
//! A gateway that no longer lets this host serve it (its credential there
//! was revoked) says `Revoked`: every lease held on the connection is ended
//! with that cause, recorded first as the expiry's is, and nothing more is
//! admitted on it. The gateway asks each lease's `End`, answered with what
//! that end took, and closes the connection.
//!
//! A host that is stopping drains (`drain`, given by composition, never by
//! the gateway): every lease held is ended with that cause, recorded first
//! as a revocation's is, and said with `Ending{drained}`, so the gateway
//! ends its side with this side's cleanup rather than finding the lease
//! lost; a running command is stopped and answers with its `Ran`. Nothing
//! more is admitted (`draining`). Once nothing is held, no command runs and
//! the gateway has asked the `End` of every lease this side ended unasked
//! (drained, expired or revoked), serving ends:
//! the connection closes with every end recorded on both sides. A gateway
//! that never asks an `End` holds serving open until composition stops it.
//!
//! A lease's end is answered by that end alone: an `Account`, or another
//! `End`, of a lease whose end is still under way waits for it, rather than
//! reading a ledger that does not hold it yet.
use super::artifacts::{ArtifactOutbox, LeaseArtifacts, PublishRequest};
use super::fetches::{ArtifactInbox, LeaseFetches, NoInbox};
use super::grants::{admit_agent, admit_command, PeerGrants};
use super::lease_policy::LeasePolicyEvaluator;
use super::wire::{write_frame, FrameStream};
use crate::devices::application::{read_machine, MachineProbe};
use crate::env_serve::domain::{NotGranted, Peer};
use crate::timer::{timeout, within, Moment, Timer};
use nessa_protocol::lease::{
    decode, ArtifactReadRefusal, Cleanup, Collection, CommandEnd, Data, EndingCause, FetchOutcome,
    FetchRefusal, FromEnvironment, GrantRefusal, GrantedTerms, Hello, PublishRefusal, Sandbox,
    StartFailure, ToEnvironment, Unavailability, KEEPALIVE_INTERVAL, MACHINE_REPORT_INTERVAL,
    MAX_DATA_BYTES, MAX_STOP_WAIT, SILENCE_LIMIT,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{HarnessControl, HarnessProcess},
};
use nessa_sdk::domain::agent_execution::leases::{CommandWork, LeaseId, SandboxProfiles};
use serde::Serialize;
use std::{
    collections::{BTreeMap, HashMap, HashSet, VecDeque},
    future::Future,
    io,
    path::Path,
    pin::Pin,
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch, Notify},
    task::JoinHandle,
};

/// How the host starts an agent's harness: its own executable, arguments,
/// account variables and credentials, in its own workspace.
pub(crate) trait HarnessLauncher: Send + Sync {
    /// The absolute directory harnesses work in.
    fn workspace(&self) -> &str;
    /// Whether the host is configured to run `agent`.
    fn runs(&self, agent: &str) -> bool;
    /// The sandboxes the host can enforce around a harness it starts.
    fn sandboxes(&self) -> SandboxProfiles;
    /// Start `agent`'s harness with `environment`, the binding's variables,
    /// added to the host's own, and `publish_point`, its lease's, when it
    /// has one, as this side's own variable.
    ///
    /// # Errors
    /// The harness could not be started; nothing runs.
    fn launch(
        &self,
        agent: &str,
        environment: &BTreeMap<String, String>,
        publish_point: Option<&str>,
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
        publish_point: Option<String>,
    ) -> Pin<Box<dyn Future<Output = CommandRan> + Send + 'static>>;
}

/// Why a running command is stopped.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum CommandStop {
    /// The gateway ended its lease.
    Ended,
    /// The gateway's connection ended.
    Lost,
    /// The gateway no longer lets this host serve it; `recorded` says
    /// whether that was recorded for the lease, and unrecorded its end
    /// settles nothing.
    Revoked { recorded: bool },
    /// The host is draining before it stops; `recorded` as for
    /// [`Self::Revoked`].
    Drained { recorded: bool },
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

/// What the gateway's `Fetch` asked for, as it said it: every fetch entry
/// carries it, so each stands on its own.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub(crate) struct FetchAsked {
    /// Where it goes: absolute, or relative to the workspace.
    pub(crate) destination: String,
    /// Its SHA-256, as the gateway named it.
    pub(crate) digest: String,
    pub(crate) media_type: String,
    pub(crate) size: u64,
}

/// What a refused lease asked for, in the fields its grant would have been
/// recorded with.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(untagged, rename_all_fields = "camelCase")]
pub(crate) enum Asked {
    /// A harness of `agent`, enclosed by `sandbox`.
    Agent { agent: String, sandbox: Sandbox },
    /// `argv` run once in `cwd`, for at most `asked_timeout_ms`.
    Command {
        argv: Vec<String>,
        cwd: Option<String>,
        asked_timeout_ms: u64,
    },
}

/// One entry of the environment's own audit of its leases. Each grant and
/// refusal names the peer it was for: `ssh`, or the dialed gateway's pin.
/// Lines from before a field was added still read: accounting reads only a
/// line's kind, lease and cleanup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum LedgerEntry {
    /// A lease was admitted for `peer`, enclosed by `sandbox`, to last at
    /// most `lease_ms` from now, or until ended when there is none.
    Granted {
        lease: String,
        peer: Peer,
        agent: String,
        sandbox: Sandbox,
        #[serde(skip_serializing_if = "Option::is_none")]
        lease_ms: Option<u64>,
    },
    /// A lease `peer` asked for was refused, and what it asked; nothing ran.
    Refused {
        lease: String,
        peer: Peer,
        reason: GrantRefusal,
        #[serde(flatten)]
        asked: Asked,
    },
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
    /// A lease lasted the `lease_ms` it was granted: this side is ending it.
    Expiring { lease: String, lease_ms: u64 },
    /// `peer`, the gateway the lease was for, said it no longer lets this
    /// host serve it (`Revoked`): this side is ending the lease.
    Revoking { lease: String, peer: Peer },
    /// The host is stopping and drains: this side is ending the lease it
    /// holds for `peer`.
    Draining { lease: String, peer: Peer },
    /// A lease ended, for `cause`, with what stopping everything under it
    /// took.
    Ended {
        lease: String,
        cleanup: Cleanup,
        cause: EndCause,
    },
    /// A harness stopped reading its input until its queue was full, or for
    /// good: it is stopped rather than fed a stream with bytes missing.
    InputOverflow { lease: String, channel: u32 },
    /// The gateway asked for a file to be published under a lease, for its
    /// `request`; what came of it is the `published` naming that request,
    /// or nothing staged.
    PublishAsked { lease: String, request: String },
    /// A file was published: its staged copy, by digest, was offered to the
    /// gateway; `request` is the gateway's ask it answers, `None` for a
    /// harness's.
    Published {
        lease: String,
        artifact: u32,
        #[serde(skip_serializing_if = "Option::is_none")]
        request: Option<String>,
        digest: String,
        size: u64,
    },
    /// What the gateway made of a published file, or the lease ending first.
    Collected {
        lease: String,
        artifact: u32,
        outcome: Collection,
    },
    /// The connection to the gateway was lost before it answered a
    /// published file: what the gateway made of it is not known here.
    Unanswered { lease: String, artifact: u32 },
    /// The gateway's fetch `request` was taken: its bytes go to part
    /// `fetch` of the lease's inbox, to be placed as `asked`.
    FetchAccepted {
        lease: String,
        request: String,
        fetch: u32,
        #[serde(flatten)]
        asked: FetchAsked,
    },
    /// The last word on the gateway's fetch `request`, which asked for
    /// `asked`: placed, already placed, or refused, as the gateway is told.
    /// The only entry of a fetch refused or already placed when asked.
    Fetched {
        lease: String,
        request: String,
        #[serde(flatten)]
        asked: FetchAsked,
        outcome: FetchOutcome,
    },
    /// A fetch taken whose bytes never came: the gateway gave up on it, or
    /// was lost with the lease. Nothing was placed; its part is let go.
    FetchUnanswered {
        lease: String,
        request: String,
        #[serde(flatten)]
        asked: FetchAsked,
    },
    /// A command lease was admitted for `peer` to run `argv` once, for at
    /// most `timeout_ms`: the `asked_timeout_ms`, or shorter as the peer's
    /// grant has it. Nothing encloses it (`sandbox`).
    CommandGranted {
        lease: String,
        peer: Peer,
        argv: Vec<String>,
        cwd: Option<String>,
        timeout_ms: u64,
        asked_timeout_ms: u64,
        sandbox: Sandbox,
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

/// Why a lease ended, as the ledger has it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum EndCause {
    /// The gateway ended it, or its command's `Ran` did.
    Asked,
    /// Its connection was lost.
    Lost,
    /// It lasted as long as it was granted.
    Expired,
    /// The gateway it was for no longer lets this host serve it.
    Revoked,
    /// The host drained it before stopping.
    Drained,
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

/// Who serving is for, and what they are granted: the peer as composition
/// knows it, never as the gateway says, where its grant is read, and the
/// policy that decides each lease under it.
#[derive(Clone)]
pub(crate) struct Serving {
    pub(crate) peer: Peer,
    pub(crate) grants: Arc<dyn PeerGrants>,
    pub(crate) policy: Arc<dyn LeasePolicyEvaluator>,
    /// What this machine is and how busy, reported to the gateway after the
    /// hello and every [`MACHINE_REPORT_INTERVAL`]; `None` reports nothing.
    pub(crate) machine: Option<Arc<dyn MachineProbe>>,
}

/// Serve one gateway, `serving`'s peer, until its stream ends, then end
/// every lease it held as lost and return once each is recorded. Every wait
/// is measured on `timer`. Once `drain` says true, every lease is ended as
/// drained and serving returns when nothing is left to answer; a `drain`
/// whose sender is gone never asks it.
#[allow(clippy::too_many_arguments)]
pub(crate) async fn serve<R, W>(
    input: R,
    output: W,
    build: &str,
    protocol: &str,
    serving: Serving,
    runners: Runners,
    ledger: Arc<dyn LeaseLedger>,
    outbox: Arc<dyn ArtifactOutbox>,
    inbox: Option<Arc<dyn ArtifactInbox>>,
    timings: ServeTimings,
    timer: Arc<dyn Timer>,
    mut drain: watch::Receiver<bool>,
) where
    R: AsyncRead + Unpin + Send + 'static,
    W: AsyncWrite + Unpin + Send + 'static,
{
    let Runners {
        harnesses: launcher,
        commands,
    } = runners;
    let (frames, mut writer) = start_writer(output, timer.clone(), serving.machine.clone());
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
        revoked: false,
        draining: false,
        unended: HashSet::new(),
        unrun: HashMap::new(),
        finished: Arc::new(Notify::new()),
        gone: watch::channel(false).0,
        runner: commands,
        serving,
        frames,
        launcher,
        ledger,
        outbox,
        inbox: inbox.unwrap_or_else(|| Arc::new(NoInbox)),
        timings,
        timer: timer.clone(),
        drops: 0,
        outgoing: VecDeque::new(),
    };
    let mut stream = FrameStream::new(input);
    // Nothing read for `silence` is the gateway gone, whether this side is
    // waiting for its next frame or is held up answering one: a frame this
    // side cannot send is a gateway not reading, which a gateway always does.
    let mut heard = timer.now();
    loop {
        let silent = heard + timings.silence;
        // A deadline that has passed is acted on before another frame is
        // read: the read wins `within`'s race, so a gateway that sends
        // without pause would otherwise put an expiry off for good. Saying
        // a lease is ending is bounded as any answer is.
        if !served.settle(silent, &mut drain).await || timer.now() >= silent {
            served.silent(timings.silence);
            break;
        }
        // Drained, with every answer handed to the writer: nothing is left
        // to serve, and closing says so.
        if served.drained() {
            tracing::info!("drained: every lease ended and answered; serving ends");
            break;
        }
        // The wait ends at the earliest lease deadline too; one passing is
        // not the gateway heard from, so `heard` stays as it was.
        let wake = served
            .next_deadline()
            .map_or(silent, |deadline| deadline.min(silent));
        // `next` keeps what a cancelled read had read, so the bound costs
        // nothing of a frame arriving slowly, nor a drain asked meanwhile.
        let finished = served.finished.clone();
        // A drain asked wins over a frame ready at the same time: a grant
        // the host reads after the signal is refused, never admitted.
        let read = tokio::select! {
            biased;
            () = drain_asked(&mut drain), if !served.draining => {
                served.drain();
                continue;
            }
            read = within(&*timer, wake, stream.next()) => read,
            // A command's `Ran` is recorded: look again. It stays held
            // until the gateway's `End`, so draining does not finish on
            // the answer alone.
            () = finished.notified(), if served.draining => continue,
        };
        let body = match read {
            Ok(Ok(Some(body))) => body,
            Ok(Ok(None)) => break,
            Ok(Err(error)) => {
                tracing::warn!(%error, "the gateway's lease stream could not be read; serving ends");
                break;
            }
            // A deadline, or the silence bound: the top of the loop says which.
            Err(_) => continue,
        };
        heard = timer.now();
        // A frame read as a deadline passed acts on what is left after it:
        // a Start under a lease that has expired starts nothing.
        if !served.settle(heard + timings.silence, &mut drain).await {
            served.silent(timings.silence);
            break;
        }
        // Handling waits for nothing: its answers are queued, and the next
        // pass hands them to the writer bounded by the deadlines as they
        // stand then, a lease this frame granted among them. They reach the
        // writer before another frame is read, as when handling sent them.
        match decode::<ToEnvironment>(&body) {
            Ok(frame) => served.handle(frame),
            Err(error) => {
                tracing::warn!(%error, "an unreadable lease frame was dropped");
                served.dropped(None, None, "unreadable");
            }
        }
    }
    // However serving ends, at the silence bound above all, a lease whose
    // deadline has come is expired, as it would have been a moment
    // earlier; only what is left is lost.
    served.expire_passed();
    served.lose_all().await;
    drop(served);
    // What is still queued for a gateway that reads nothing is given up on,
    // so serving, and the lock it holds, always ends.
    if timeout(&*timer, timings.silence, &mut writer)
        .await
        .is_err()
    {
        writer.abort();
    }
}

fn start_writer<W>(
    mut output: W,
    timer: Arc<dyn Timer>,
    machine: Option<Arc<dyn MachineProbe>>,
) -> (mpsc::Sender<FromEnvironment>, JoinHandle<()>)
where
    W: AsyncWrite + Unpin + Send + 'static,
{
    let (sender, mut receiver) = mpsc::channel::<FromEnvironment>(OUTPUT_QUEUE);
    let writer = tokio::spawn(async move {
        // The machine is reported only after the hello, which is always the
        // first frame, then whenever a report is due, busy or idle.
        let mut reported: Option<Moment> = None;
        let mut started = false;
        loop {
            // Idle for a keepalive interval: the gateway is told this side is
            // still there, which a network gone silent cannot tell it. A
            // machine report says so too, and takes its place when one is due.
            let frame = match timeout(&*timer, KEEPALIVE_INTERVAL, receiver.recv()).await {
                Ok(Some(frame)) => frame,
                Ok(None) => break,
                Err(_) => FromEnvironment::Keepalive,
            };
            let due = |reported: Option<Moment>| {
                reported.is_none_or(|at| timer.now() >= at + MACHINE_REPORT_INTERVAL)
            };
            let frame = match (&frame, &machine) {
                (FromEnvironment::Keepalive, Some(probe)) if started && due(reported) => {
                    reported = Some(timer.now());
                    report(probe).await.unwrap_or(frame)
                }
                _ => frame,
            };
            if let Err(error) = write_frame(&mut output, &frame).await {
                tracing::warn!(%error, "a lease frame could not be written to the gateway");
                break;
            }
            started = true;
            let Some(probe) = &machine else { continue };
            if !due(reported) {
                continue;
            }
            reported = Some(timer.now());
            if let Some(frame) = report(probe).await {
                if let Err(error) = write_frame(&mut output, &frame).await {
                    tracing::warn!(%error, "a lease frame could not be written to the gateway");
                    break;
                }
            }
        }
        let _ = output.shutdown().await;
    });
    (sender, writer)
}

/// The machine as `probe` reads it (`read_machine`): `None` when it cannot
/// be read in time.
async fn report(probe: &Arc<dyn MachineProbe>) -> Option<FromEnvironment> {
    read_machine(probe)
        .await
        .map(|report| FromEnvironment::Machine { report })
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
    /// Its publish point and the artifacts in flight under it; `None` when
    /// the point could not be opened.
    artifacts: Option<LeaseArtifacts>,
    /// The files fetched into its workspace; taken once its end begins.
    fetches: Option<LeaseFetches>,
    /// When it ends by itself, and how long it was granted for; `None` when
    /// it lasts until ended.
    deadline: Option<(Moment, u64)>,
}

/// One command lease this connection holds. Its publish point stays open
/// from the grant until the gateway ends the lease, so a file still in
/// flight can be answered between `Ran` and `End`.
enum HeldCommand {
    /// Granted, its command not yet run. `artifacts` is its publish point;
    /// `None` when the point could not be opened.
    Granted {
        work: CommandWork,
        artifacts: Option<LeaseArtifacts>,
    },
    /// Its command started: `stop` stops it, and `ran` is its cleanup once
    /// its end is recorded and its `Ran` handed on. `ending` turns true,
    /// under its lock, once the command's own task has taken what ended it:
    /// from then on its end is what that task records, and a revocation
    /// coming after it records nothing of this lease. `artifacts` stays
    /// until the gateway's `End`, a loss, or the command is dropped finished.
    Running {
        stop: watch::Sender<Option<CommandStop>>,
        ran: watch::Receiver<Option<Cleanup>>,
        ending: Arc<Mutex<bool>>,
        artifacts: Option<LeaseArtifacts>,
    },
}

/// Close a command lease's publish point. `unasked` leaves a file still in
/// flight unanswered; an end the gateway asked answers it `leaseEnded`.
fn close_command_artifacts(
    artifacts: Option<LeaseArtifacts>,
    unasked: bool,
) -> Vec<JoinHandle<()>> {
    artifacts
        .map(|artifacts| artifacts.end(unasked))
        .unwrap_or_default()
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
    /// The gateway said `Revoked`: nothing more is admitted here.
    revoked: bool,
    /// The host is draining: nothing more is admitted here, and serving
    /// ends once nothing is left to answer.
    draining: bool,
    /// Leases this side ended unasked (drained, expired or revoked) whose
    /// `End` the gateway has not yet asked: a drain waits for each.
    unended: HashSet<String>,
    /// Commands a drain ended unrun, with what their end took, whose `Run`
    /// or `End` the gateway has not yet sent: a drain waits for each, and
    /// its `Run` is answered never started with that cleanup.
    unrun: HashMap<String, Cleanup>,
    /// Told whenever a command's end is recorded, its `Ran` handed on: a
    /// drain waiting on it looks again.
    finished: Arc<Notify>,
    /// Set once the gateway is gone: an answer still waiting for room, a
    /// command's or a fetch's, is given up on, whatever stopped what it
    /// answers, so nothing that ends a lease waits on a full queue.
    gone: watch::Sender<bool>,
    /// How this host runs commands; `None` when it is not configured to.
    runner: Option<Arc<dyn CommandRunner>>,
    serving: Serving,
    frames: mpsc::Sender<FromEnvironment>,
    launcher: Arc<dyn HarnessLauncher>,
    ledger: Arc<dyn LeaseLedger>,
    outbox: Arc<dyn ArtifactOutbox>,
    /// Where fetched files arrive; [`NoInbox`] when this host takes none
    /// (`envServe.artifactsIn` off).
    inbox: Arc<dyn ArtifactInbox>,
    timings: ServeTimings,
    /// What every wait, the stops' own included, is measured on.
    timer: Arc<dyn Timer>,
    drops: u32,
    /// Answers not yet taken by the writer, in order. One leaves only once
    /// there is room for it, so a wait for room given up on loses nothing,
    /// and what is said after it goes behind it.
    outgoing: VecDeque<FromEnvironment>,
}

impl Served {
    fn handle(&mut self, frame: ToEnvironment) {
        match frame {
            ToEnvironment::Grant {
                lease,
                agent,
                sandbox,
            } => self.grant(lease, agent, sandbox),
            ToEnvironment::Start {
                lease,
                channel,
                environment,
            } => self.start(lease, channel, &environment),
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
                    // Its lease is ending here (it expired): the stop is
                    // answered with what that end took, never as holding
                    // nothing while its harness may still be stopping.
                    None if self.ends.contains_key(&lease) => {
                        self.answer_ended(lease, Vec::new(), move |lease, cleanup| {
                            FromEnvironment::Stopped {
                                lease,
                                channel,
                                cleanup,
                            }
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
                        });
                    }
                }
            }
            ToEnvironment::GrantCommand {
                lease,
                argv,
                cwd,
                timeout_ms,
            } => self.grant_command(lease, argv, cwd, timeout_ms),
            ToEnvironment::Run { lease } => self.run(lease),
            ToEnvironment::End { lease } if self.commands.contains_key(&lease) => {
                self.end_command(lease);
            }
            ToEnvironment::End { lease } => match self.leases.remove(&lease) {
                Some(held) => self.end(lease, held, EndCause::Asked, true),
                // Not held on this connection: what its end under way, or
                // the audit, says of it.
                None => {
                    self.unended.remove(&lease);
                    if let Some(cleanup) = self.unrun.remove(&lease) {
                        return self.send(FromEnvironment::Ended { lease, cleanup });
                    }
                    self.answer_ended(lease, Vec::new(), |lease, cleanup| FromEnvironment::Ended {
                        lease,
                        cleanup,
                    });
                }
            },
            ToEnvironment::Account { lease } => {
                self.answer_ended(lease, Vec::new(), |lease, cleanup| {
                    FromEnvironment::Accounted { lease, cleanup }
                });
            }
            ToEnvironment::Keepalive => {}
            ToEnvironment::Revoked => self.revoke(),
            ToEnvironment::Collected {
                lease,
                artifact,
                outcome,
            } => {
                let answered = self
                    .artifacts_of(&lease)
                    .is_some_and(|artifacts| artifacts.collected(artifact, outcome));
                if !answered {
                    self.dropped(Some(&lease), None, "collected");
                }
            }
            ToEnvironment::Publish {
                lease,
                request,
                path,
                media_type,
            } => self.publish(lease, request, path, media_type),
            ToEnvironment::Fetch {
                lease,
                request,
                destination,
                digest,
                media_type,
                size,
            } => {
                let asked = FetchAsked {
                    destination,
                    digest,
                    media_type,
                    size,
                };
                match self
                    .leases
                    .get(&lease)
                    .and_then(|held| held.fetches.as_ref())
                {
                    Some(fetches) => fetches.ask(request, asked),
                    // Taken by no lease here: refused, so the gateway does
                    // not wait for an answer that never comes.
                    None => {
                        self.dropped(Some(&lease), None, "fetch");
                        self.send(FromEnvironment::FetchRefused {
                            lease,
                            request,
                            reason: FetchRefusal::LeaseEnded,
                        });
                    }
                }
            }
            ToEnvironment::Delivered { lease, request } => {
                let placed = self
                    .leases
                    .get(&lease)
                    .and_then(|held| held.fetches.as_ref())
                    .is_some_and(|fetches| fetches.delivered(&request));
                if !placed {
                    self.dropped(Some(&lease), None, "delivered");
                }
            }
            ToEnvironment::Undelivered { lease, request } => {
                let let_go = self
                    .leases
                    .get(&lease)
                    .and_then(|held| held.fetches.as_ref())
                    .is_some_and(|fetches| fetches.undelivered(&request));
                if !let_go {
                    self.dropped(Some(&lease), None, "undelivered");
                }
            }
            ToEnvironment::ReadArtifact {
                lease,
                artifact,
                offset,
                length,
            } => {
                let read = match self.artifacts_of(&lease) {
                    Some(artifacts) => artifacts.read(artifact, offset, length),
                    None => {
                        self.dropped(Some(&lease), None, "read_artifact");
                        Err(ArtifactReadRefusal::NotStaged)
                    }
                };
                // Said, so the gateway does not wait for bytes that never come.
                if let Err(reason) = read {
                    self.send(FromEnvironment::ArtifactNotRead {
                        lease,
                        artifact,
                        offset,
                        reason,
                    });
                }
            }
        }
    }

    /// Publish a file the gateway asked for under `lease`, as its publish
    /// point would for a harness: the same bound, outbox and ledger. Its
    /// path is the workspace's when relative; the outbox decides whether it
    /// is inside it. Not published, it is answered `NotPublished`.
    fn publish(
        &mut self,
        lease: String,
        request: String,
        path: String,
        media_type: Option<String>,
    ) {
        let refusal = match self.leases.get(&lease).map(|held| held.artifacts.is_some()) {
            // A command lease, or none: nothing is published under it.
            None => {
                self.dropped(Some(&lease), None, "publish");
                Some(PublishRefusal::LeaseEnded)
            }
            // Its outbox could not be opened: nothing can be staged for it.
            Some(false) => Some(PublishRefusal::StagingFailed),
            Some(true) => {
                let entry = LedgerEntry::PublishAsked {
                    lease: lease.clone(),
                    request: request.clone(),
                };
                match self.ledger.record(&entry) {
                    Ok(()) => None,
                    Err(error) => {
                        tracing::error!(lease, %error, "a gateway's publish could not be recorded; it is not published");
                        Some(PublishRefusal::AuditUnavailable)
                    }
                }
            }
        };
        if let Some(reason) = refusal {
            return self.send(FromEnvironment::NotPublished {
                lease,
                request,
                reason,
            });
        }
        let path = Path::new(self.launcher.workspace())
            .join(path)
            .to_string_lossy()
            .into_owned();
        if let Some(artifacts) = self
            .leases
            .get(&lease)
            .and_then(|held| held.artifacts.as_ref())
        {
            artifacts.ask(request, PublishRequest { path, media_type });
        }
    }

    /// Answer what became of `lease`, a lease this connection no longer
    /// holds: once its end under way here is recorded, with what that took,
    /// or else from the audit.
    fn answer_ended(
        &mut self,
        lease: String,
        reads: Vec<JoinHandle<()>>,
        answer: impl FnOnce(String, Cleanup) -> FromEnvironment + Send + 'static,
    ) {
        let Some(mut ending) = self.ends.get(&lease).cloned() else {
            let cleanup = self.accounted(&lease);
            return self.send_after_reads(reads, answer(lease, cleanup));
        };
        let frames = self.frames.clone();
        tokio::spawn(async move {
            for read in reads {
                let _ = read.await;
            }
            // An end that never said (its task failed) is not known done.
            let cleanup = match ending.wait_for(Option::is_some).await {
                Ok(cleanup) => (*cleanup).unwrap_or(Cleanup::Uncertain),
                Err(_) => Cleanup::Uncertain,
            };
            let _ = frames.send(answer(lease, cleanup)).await;
        });
    }

    /// Send `frame` once `reads` have finished. None waiting: it is queued
    /// with everything else. Reads under way go out on their own, after
    /// they finish, so a lease's end is not said over a read still holding
    /// a file.
    fn send_after_reads(&mut self, reads: Vec<JoinHandle<()>>, frame: FromEnvironment) {
        if reads.is_empty() {
            self.send(frame);
            return;
        }
        let frames = self.frames.clone();
        tokio::spawn(async move {
            for read in reads {
                let _ = read.await;
            }
            let _ = frames.send(frame).await;
        });
    }

    /// Queue an answer. It reaches the writer from [`Self::settle`], so a
    /// handler never waits on a gateway that is not reading.
    fn send(&mut self, frame: FromEnvironment) {
        self.outgoing.push_back(frame);
    }

    /// Hand the writer every answer still waiting, in order. Cancellable at
    /// any await: a frame is taken off only with a place to put it.
    async fn flush(&mut self) {
        while !self.outgoing.is_empty() {
            // The writer is gone: nothing more reaches the gateway.
            let Ok(permit) = self.frames.reserve().await else {
                self.outgoing.clear();
                return;
            };
            if let Some(frame) = self.outgoing.pop_front() {
                permit.send(frame);
            }
        }
    }

    /// End every lease whose deadline has passed, and hand the writer what
    /// is waiting, until nothing is or `silent` passes: `false` then. A wait
    /// for room is given up on at the next lease deadline, which is acted
    /// on at once, so a gateway reading nothing holds no expiry back.
    async fn settle(&mut self, silent: Moment, drain: &mut watch::Receiver<bool>) -> bool {
        let timer = self.timer.clone();
        loop {
            // Ends each due lease without waiting: its `Ending` is queued.
            self.expire_passed();
            if self.outgoing.is_empty() {
                return true;
            }
            let wake = self
                .next_deadline()
                .map_or(silent, |deadline| deadline.min(silent));
            // A drain asked while a gateway that is not reading holds the
            // writer up starts at once: its leases end, recorded, before a
            // deadline closes the connection, and are not found lost.
            let flushed = tokio::select! {
                biased;
                () = drain_asked(drain), if !self.draining => {
                    self.drain();
                    continue;
                }
                flushed = within(&*timer, wake, self.flush()) => flushed,
            };
            if flushed.is_err() && timer.now() >= silent {
                return false;
            }
        }
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

    fn grant(&mut self, lease: String, agent: String, sandbox: Sandbox) {
        // How long the peer's grant lets it last, once admitted.
        let mut lease_ms = None;
        let refusal = if LeaseId::new(lease.clone()).is_err() || self.granted.contains(&lease) {
            Some(GrantRefusal::Duplicate)
        } else if let Some(refusal) = self.recorded_before(&lease) {
            Some(refusal)
        } else if self.revoked {
            // The gateway said it no longer lets this host serve it: the
            // ask crossed that, and the owner's grant has no part in it.
            Some(GrantRefusal::Revoked)
        } else if self.draining {
            // The host is stopping: the ask crossed that.
            Some(GrantRefusal::Draining)
        } else if self
            .admit_agent(&agent)
            .map(|admitted| lease_ms = admitted)
            .is_err()
        {
            // First: a peer not granted the agent learns nothing of what
            // the host runs.
            Some(GrantRefusal::NotGranted)
        } else if !self.launcher.runs(&agent) {
            Some(GrantRefusal::AgentUnavailable)
        } else if !self.launcher.sandboxes().contains(sandbox.into()) {
            Some(GrantRefusal::SandboxUnavailable)
        } else {
            match self.ledger.record(&LedgerEntry::Granted {
                lease: lease.clone(),
                peer: self.serving.peer,
                agent: agent.clone(),
                sandbox,
                lease_ms,
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
                let artifacts = LeaseArtifacts::open(
                    &lease,
                    self.outbox.clone(),
                    self.ledger.clone(),
                    self.frames.clone(),
                    self.gone.subscribe(),
                );
                let fetches = Some(LeaseFetches::new(
                    &lease,
                    self.inbox.clone(),
                    self.ledger.clone(),
                    self.frames.clone(),
                    self.gone.subscribe(),
                ));
                self.leases.insert(
                    lease.clone(),
                    Held {
                        agent,
                        channels: HashMap::new(),
                        stopped: HashMap::new(),
                        stops: Vec::new(),
                        artifacts,
                        fetches,
                        deadline: lease_ms
                            .map(|ms| (self.timer.now() + Duration::from_millis(ms), ms)),
                    },
                );
                let terms = GrantedTerms {
                    sandbox,
                    timeout_ms: None,
                    lease_ms,
                };
                self.send(FromEnvironment::Granted { lease, terms });
            }
            Some(reason) => {
                if reason != GrantRefusal::AuditUnavailable {
                    let entry = LedgerEntry::Refused {
                        lease: lease.clone(),
                        peer: self.serving.peer,
                        reason,
                        asked: Asked::Agent { agent, sandbox },
                    };
                    if let Err(error) = self.ledger.record(&entry) {
                        tracing::error!(lease, %error, "a refused lease could not be recorded");
                    }
                }
                self.send(FromEnvironment::Refused { lease, reason });
            }
        }
    }

    /// Whether the peer may lease `agent` now, and for how long: its grant
    /// read again for every lease, so a change applies from the next one.
    fn admit_agent(&self, agent: &str) -> Result<Option<u64>, NotGranted> {
        admit_agent(
            self.serving.grants.as_ref(),
            self.serving.policy.as_ref(),
            &self.serving.peer,
            agent,
        )
    }

    /// Whether the peer may run a command now, and the timeout it is
    /// granted, as [`Self::admit_agent`] reads its grant.
    fn admit_command(&self, timeout_ms: u64) -> Result<u64, NotGranted> {
        admit_command(
            self.serving.grants.as_ref(),
            self.serving.policy.as_ref(),
            &self.serving.peer,
            timeout_ms,
        )
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

    fn start(&mut self, lease: String, channel: u32, environment: &BTreeMap<String, String>) {
        let free = self.leases.get(&lease).is_some_and(|held| {
            !held.channels.contains_key(&channel) && !held.stopped.contains_key(&channel)
        });
        if !free {
            self.dropped(Some(&lease), Some(channel), "start");
            return self.start_failed(lease, channel, StartFailure::NotGranted);
        }
        let entry = LedgerEntry::Started {
            lease: lease.clone(),
            channel,
        };
        if let Err(error) = self.ledger.record(&entry) {
            tracing::error!(lease, channel, %error, "a harness start could not be recorded; nothing started");
            return self.start_failed(lease, channel, StartFailure::AuditUnavailable);
        }
        let (agent, publish_point) = self
            .leases
            .get(&lease)
            .map(|held| {
                let point = held
                    .artifacts
                    .as_ref()
                    .map(|artifacts| artifacts.address().to_owned());
                (held.agent.clone(), point)
            })
            .unwrap_or_default();
        let process = match self
            .launcher
            .launch(&agent, environment, publish_point.as_deref())
        {
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
                return self.start_failed(lease, channel, StartFailure::SpawnFailed);
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

    fn start_failed(&mut self, lease: String, channel: u32, reason: StartFailure) {
        self.send(FromEnvironment::StartFailed {
            lease,
            channel,
            reason,
        });
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
        let timer = self.timer.clone();
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
            if timeout(&*timer, grace, &mut pump).await.is_err() {
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
            if timeout(&*timer, kill, &mut output).await.is_err() {
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

    /// The earliest moment a held lease ends by itself, if one does.
    fn next_deadline(&self) -> Option<Moment> {
        self.leases
            .values()
            .filter_map(|held| held.deadline.map(|(deadline, _)| deadline))
            .min()
    }

    /// End every held lease that has lasted as long as it was granted, if
    /// any has. Nothing here waits, so one lease's expiry never holds
    /// another's back.
    fn expire_passed(&mut self) {
        let now = self.timer.now();
        let mut due: Vec<_> = self
            .leases
            .iter()
            .filter_map(|(lease, held)| {
                let (deadline, ms) = held.deadline?;
                (deadline <= now).then(|| (deadline, lease.clone(), ms))
            })
            .collect();
        due.sort();
        for (_, lease, lease_ms) in due {
            if let Some(held) = self.leases.remove(&lease) {
                self.expire(lease, held, lease_ms);
            }
        }
    }

    /// End `lease`, which has lasted the `lease_ms` it was granted: the
    /// cause recorded first, then everything under it stopped as an end
    /// does (on tasks of their own), and the gateway's `Ending` queued
    /// behind the answers already waiting, for [`Self::settle`] to send.
    /// Its `End` is answered with what that took. Nothing here waits.
    fn expire(&mut self, lease: String, held: Held, lease_ms: u64) {
        let recorded = match self.ledger.record(&LedgerEntry::Expiring {
            lease: lease.clone(),
            lease_ms,
        }) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(lease, %error, "a lease's expiry could not be recorded; it is ended still");
                false
            }
        };
        self.end(lease.clone(), held, EndCause::Expired, recorded);
        self.unended.insert(lease.clone());
        self.outgoing.push_back(FromEnvironment::Ending {
            lease,
            cause: EndingCause::Expired,
        });
    }

    /// End `lease` on a task of its own: stop every harness still running
    /// under it, wait for the stops already under way, record what all of it
    /// took and, when the gateway asked for it, answer it after every answer
    /// those stops send. One lost with the connection, expired or revoked
    /// is not answered unasked: its cleanup answers the gateway's `End` or
    /// `Account`. Unless what made it end was `recorded`, its cleanup is
    /// uncertain.
    fn end(&mut self, lease: String, mut held: Held, cause: EndCause, recorded: bool) {
        // Nothing more is published under it, and nothing it staged is read
        // again: the gateway ended or revoked it, is gone, or it expired.
        // Only the gateway's own end says what it made of a file still in
        // flight (it took none); once lost, expired or revoked, the gateway
        // may have kept it, so it is recorded unanswered rather than
        // refused.
        let reads = held
            .artifacts
            .take()
            .map(|artifacts| artifacts.end(cause != EndCause::Asked))
            .unwrap_or_default();
        // Nothing more is taken or placed under it from here; what is under
        // way finishes and is answered before its end is.
        let fetches = held
            .fetches
            .take()
            .map(|fetches| fetches.end(cause == EndCause::Lost));
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
                    recorded,
                },
                running,
            ));
        }
        let ledger = self.ledger.clone();
        let frames = self.frames.clone();
        tokio::spawn(async move {
            // Its reads were stopped: none answers after its end is known.
            for read in reads {
                let _ = read.await;
            }
            // A fetch whose entry went unrecorded leaves its end unknown.
            let fetches_recorded = match fetches {
                Some(fetches) => fetches.await,
                None => true,
            };
            let mut cleanup = if recorded && fetches_recorded {
                Cleanup::Confirmed { forced: false }
            } else {
                Cleanup::Uncertain
            };

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
                cause,
            };
            let cleanup = match ledger.record(&entry) {
                Ok(()) => cleanup,
                Err(error) => {
                    tracing::error!(lease, %error, "a lease's end could not be recorded");
                    Cleanup::Uncertain
                }
            };
            done.send_replace(Some(cleanup));
            if cause == EndCause::Asked {
                for stop in stops {
                    let _ = stop.task.await;
                }
                let _ = frames.send(FromEnvironment::Ended { lease, cleanup }).await;
            }
        });
    }

    /// The publish point of `lease`, a harness lease or a command lease.
    fn artifacts_of(&self, lease: &str) -> Option<&LeaseArtifacts> {
        self.leases
            .get(lease)
            .and_then(|held| held.artifacts.as_ref())
            .or_else(|| {
                self.commands.get(lease).and_then(|held| match held {
                    HeldCommand::Granted { artifacts, .. }
                    | HeldCommand::Running { artifacts, .. } => artifacts.as_ref(),
                })
            })
    }

    /// Admit a command lease, or refuse it, as [`Self::grant`] does a
    /// harness's: only as the peer's grant has it, its timeout narrowed to
    /// the grant's longest, then only on a host that runs commands, and only
    /// for a command a command lease can hold with the timeout granted.
    fn grant_command(
        &mut self,
        lease: String,
        argv: Vec<String>,
        cwd: Option<String>,
        timeout_ms: u64,
    ) {
        // A command whose exit is recorded stays until its own `End`, or
        // the connection is lost. Closing it here would cut off a copy the
        // gateway is still reading, which is what holds that `End` back.
        let refusal = if LeaseId::new(lease.clone()).is_err() || self.granted.contains(&lease) {
            Err(GrantRefusal::Duplicate)
        } else if let Some(refusal) = self.recorded_before(&lease) {
            Err(refusal)
        } else if self.revoked {
            Err(GrantRefusal::Revoked)
        } else if self.draining {
            Err(GrantRefusal::Draining)
        } else {
            // The peer's grant first: a peer granted no commands learns
            // nothing of whether the host runs any.
            match self.admit_command(timeout_ms) {
                Err(_) => Err(GrantRefusal::NotGranted),
                Ok(_) if self.runner.is_none() => Err(GrantRefusal::CommandsUnavailable),
                // Checked here as the command it will run, with the timeout
                // granted: the one asked, or shorter.
                Ok(granted) => match CommandWork::new(argv.clone(), cwd.clone(), granted) {
                    Err(_) => Err(GrantRefusal::InvalidCommand),
                    Ok(work) => match self.ledger.record(&LedgerEntry::CommandGranted {
                        lease: lease.clone(),
                        peer: self.serving.peer,
                        argv: argv.clone(),
                        cwd: cwd.clone(),
                        timeout_ms: granted,
                        asked_timeout_ms: timeout_ms,
                        sandbox: Sandbox::None,
                    }) {
                        Ok(()) => Ok(work),
                        Err(error) => {
                            tracing::error!(lease, %error, "a command lease could not be recorded; it is refused");
                            Err(GrantRefusal::AuditUnavailable)
                        }
                    },
                },
            }
        };
        match refusal {
            Ok(work) => {
                let terms = GrantedTerms {
                    sandbox: Sandbox::None,
                    timeout_ms: Some(work.timeout_ms()),
                    // Its timeout is its deadline: no lease limit applies.
                    lease_ms: None,
                };
                self.granted.insert(lease.clone());
                let artifacts = LeaseArtifacts::open(
                    &lease,
                    self.outbox.clone(),
                    self.ledger.clone(),
                    self.frames.clone(),
                    self.gone.subscribe(),
                );
                self.commands
                    .insert(lease.clone(), HeldCommand::Granted { work, artifacts });
                self.send(FromEnvironment::Granted { lease, terms });
            }
            Err(reason) => {
                if reason != GrantRefusal::AuditUnavailable {
                    let entry = LedgerEntry::Refused {
                        lease: lease.clone(),
                        peer: self.serving.peer,
                        reason,
                        asked: Asked::Command {
                            argv,
                            cwd,
                            asked_timeout_ms: timeout_ms,
                        },
                    };
                    if let Err(error) = self.ledger.record(&entry) {
                        tracing::error!(lease, %error, "a refused command lease could not be recorded");
                    }
                }
                self.send(FromEnvironment::Refused { lease, reason });
            }
        }
    }

    /// Run a granted command lease's command, once, on a task of its own,
    /// which records how it ended and answers with its `Ran`. A lease not
    /// granted here, or run already, is answered as never started.
    fn run(&mut self, lease: String) {
        // Ended unrun by a drain: never started, with what its end took.
        if let Some(cleanup) = self.unrun.remove(&lease) {
            let not_started = CommandRan {
                cleanup,
                ..CommandRan::not_started()
            };
            return self.send(ran(lease, not_started));
        }
        let (work, artifacts) = match self.commands.remove(&lease) {
            Some(HeldCommand::Granted { work, artifacts }) => (work, artifacts),
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
                return self.send(ran(lease, CommandRan::not_started()));
            }
        };
        let Some(runner) = self.runner.clone() else {
            unreachable!("a command lease is granted only on a host that runs commands");
        };
        let (stop, stopped) = watch::channel(None);
        let (done, ran_receiver) = watch::channel(None);
        let ending = Arc::new(Mutex::new(false));
        let entry = LedgerEntry::CommandStarted {
            lease: lease.clone(),
        };
        let point = artifacts
            .as_ref()
            .map(|artifacts| artifacts.address().to_owned());
        let started = match self.ledger.record(&entry) {
            Ok(()) => Some(runner.run(work, stopped.clone(), point)),
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
                ending: ending.clone(),
                artifacts,
            },
        );
        let ledger = self.ledger.clone();
        let frames = self.frames.clone();
        let mut gone = self.gone.subscribe();
        let finished = self.finished.clone();
        tokio::spawn(async move {
            let mut outcome = match started {
                Some(running) => running.await,
                None => CommandRan::not_started(),
            };
            // What ended it is taken once, under the lock a revocation
            // decides under: a revocation either came first, and is this
            // end's cause, or comes after, and records nothing of it.
            let stop = {
                let mut ending = ending.lock().unwrap_or_else(PoisonError::into_inner);
                *ending = true;
                *stopped.borrow()
            };
            let lost = stop == Some(CommandStop::Lost);
            let cause = match stop {
                Some(CommandStop::Lost) => EndCause::Lost,
                Some(CommandStop::Revoked { recorded }) => {
                    // What ended it went unrecorded: it settles nothing.
                    if !recorded {
                        outcome.cleanup = Cleanup::Uncertain;
                    }
                    EndCause::Revoked
                }
                Some(CommandStop::Drained { recorded }) => {
                    if !recorded {
                        outcome.cleanup = Cleanup::Uncertain;
                    }
                    // Said so in its `Ran`: the gateway asked no stop, and
                    // would otherwise take it for lost.
                    if outcome.end == CommandEnd::Stopped {
                        outcome.end = CommandEnd::Drained;
                    }
                    EndCause::Drained
                }
                Some(CommandStop::Ended) | None => EndCause::Asked,
            };
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
                cause,
            };
            if let Err(error) = ledger.record(&ended_entry) {
                tracing::error!(lease, %error, "a command's end could not be recorded");
                outcome.cleanup = Cleanup::Uncertain;
            }
            let cleanup = outcome.cleanup;
            // Its answer goes before anything that waits for its end, so a
            // gateway's `End` of it is answered after its `Ran`.
            if !lost {
                // A gateway taken as gone while this waits for room to say
                // it is not waited on: its end is recorded either way. The
                // gateway's going is its own signal, apart from why the
                // command was stopped, so a command stopped first for
                // another cause (an end, a revocation) is not waited on
                // either.
                tokio::select! {
                    _ = frames.send(ran(lease, outcome)) => {}
                    _ = gone.wait_for(|gone| *gone) => {}
                }
            }
            done.send_replace(Some(cleanup));
            finished.notify_one();
        });
    }

    /// End a command lease: stop its command if it runs, and answer its end
    /// once that is recorded; one never run ends holding nothing.
    fn end_command(&mut self, lease: String) {
        match self.commands.remove(&lease) {
            Some(HeldCommand::Running {
                stop,
                ran,
                artifacts,
                ..
            }) => {
                let reads = close_command_artifacts(artifacts, false);
                ask_stop(&stop, CommandStop::Ended);
                self.ends.retain(|_, ended| ended.borrow().is_none());
                self.ends.insert(lease.clone(), ran);
                self.answer_ended(lease, reads, |lease, cleanup| FromEnvironment::Ended {
                    lease,
                    cleanup,
                });
            }
            Some(HeldCommand::Granted { artifacts, .. }) => {
                let reads = close_command_artifacts(artifacts, false);
                let cleanup = self.end_unrun(&lease, EndCause::Asked);
                self.send_after_reads(reads, FromEnvironment::Ended { lease, cleanup });
            }
            None => unreachable!("only a held command lease is ended here"),
        }
    }

    /// Record the end of a command lease whose command never ran.
    fn end_unrun(&self, lease: &str, cause: EndCause) -> Cleanup {
        self.end_unrun_with(lease, cause, Cleanup::NotHeld)
    }

    /// Record the end of a command lease whose command never ran, with
    /// `cleanup`: nothing held, or uncertain when what ended it went
    /// unrecorded.
    fn end_unrun_with(&self, lease: &str, cause: EndCause, cleanup: Cleanup) -> Cleanup {
        let entry = LedgerEntry::Ended {
            lease: lease.into(),
            cleanup,
            cause,
        };
        match self.ledger.record(&entry) {
            Ok(()) => cleanup,
            Err(error) => {
                tracing::error!(lease, %error, "a command lease's end could not be recorded");
                Cleanup::Uncertain
            }
        }
    }

    /// The gateway no longer lets this host serve it: end every lease held
    /// on this connection as revoked, each recorded `revoking` first, as an
    /// expiry is, and admit nothing more on it. Nothing here waits: each
    /// harness lease ends as an end does, on tasks of its own, and its
    /// `End` is answered with what that took; a running command is stopped
    /// and answers with its `Ran`; one granted and never run ends holding
    /// nothing. Said twice, it is acted on once.
    fn revoke(&mut self) {
        if std::mem::replace(&mut self.revoked, true) {
            return;
        }
        tracing::warn!(
            leases = self.leases.len(),
            commands = self.commands.len(),
            "the gateway no longer lets this host serve it; every lease it holds here ends"
        );
        let mut leases: Vec<_> = self.leases.drain().collect();
        leases.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (lease, held) in leases {
            let recorded = self.revoking(&lease);
            self.end(lease.clone(), held, EndCause::Revoked, recorded);
            self.unended.insert(lease);
        }
        let mut commands: Vec<_> = self.commands.keys().cloned().collect();
        commands.sort();
        for lease in commands {
            match self.commands.get(&lease) {
                // Its own `Ran` answers, and the gateway's `End` of it waits
                // for that; one already asked to stop ends as it was asked.
                // Decided under its task's lock: one whose task already took
                // what ended it (its end recorded, or about to be, while its
                // `Ran` may still wait for room) is not blamed on this.
                Some(HeldCommand::Running { stop, ending, .. }) if stop.borrow().is_none() => {
                    let ending = ending.lock().unwrap_or_else(PoisonError::into_inner);
                    if !*ending {
                        let recorded = self.revoking(&lease);
                        ask_stop(stop, CommandStop::Revoked { recorded });
                    }
                }
                Some(HeldCommand::Granted { .. }) => {
                    let artifacts = match self.commands.remove(&lease) {
                        Some(HeldCommand::Granted { artifacts, .. }) => artifacts,
                        _ => None,
                    };
                    let _reads = close_command_artifacts(artifacts, true);
                    let recorded = self.revoking(&lease);
                    let cleanup = if recorded {
                        Cleanup::NotHeld
                    } else {
                        Cleanup::Uncertain
                    };
                    self.end_unrun_with(&lease, EndCause::Revoked, cleanup);
                }
                _ => {}
            }
        }
    }

    /// Record that `lease` is ending because the gateway revoked this host:
    /// whether that was recorded.
    fn revoking(&self, lease: &str) -> bool {
        let entry = LedgerEntry::Revoking {
            lease: lease.into(),
            peer: self.serving.peer,
        };
        match self.ledger.record(&entry) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(lease, %error, "a lease's revocation could not be recorded; it is ended still");
                false
            }
        }
    }

    /// The host is stopping: end every lease held on this connection as
    /// drained, each recorded `draining` first, as a revocation is, and say
    /// `Ending{drained}` for each harness lease, so the gateway asks its
    /// `End` and gets this end's cleanup rather than finding it lost. A
    /// running command is stopped and answers with its `Ran`, and stays
    /// held until the gateway's `End`: closing on the answer would shut
    /// its publish point as a loss while that `End` was still coming. One
    /// granted and never run ends holding nothing, and serving waits for
    /// the gateway's `Run` of it (answered never started) or its `End`.
    /// Nothing more is admitted. Nothing here waits; asked twice, it acts once.
    fn drain(&mut self) {
        if std::mem::replace(&mut self.draining, true) {
            return;
        }
        tracing::info!(
            leases = self.leases.len(),
            commands = self.commands.len(),
            "draining: every lease held here ends before the host stops"
        );
        // Said first, so the gateway asks for nothing more here before it
        // learns of any lease's end.
        self.outgoing.push_back(FromEnvironment::Draining);
        let mut leases: Vec<_> = self.leases.drain().collect();
        leases.sort_by(|(left, _), (right, _)| left.cmp(right));
        for (lease, held) in leases {
            let recorded = self.draining_recorded(&lease);
            self.end(lease.clone(), held, EndCause::Drained, recorded);
            self.unended.insert(lease.clone());
            self.outgoing.push_back(FromEnvironment::Ending {
                lease,
                cause: EndingCause::Drained,
            });
        }
        let mut commands: Vec<_> = self.commands.keys().cloned().collect();
        commands.sort();
        for lease in commands {
            match self.commands.get(&lease) {
                // As a revocation decides it: one whose own task already took
                // what ended it is not blamed on the drain.
                Some(HeldCommand::Running { stop, ending, .. }) if stop.borrow().is_none() => {
                    let ending = ending.lock().unwrap_or_else(PoisonError::into_inner);
                    if !*ending {
                        let recorded = self.draining_recorded(&lease);
                        ask_stop(stop, CommandStop::Drained { recorded });
                    }
                }
                Some(HeldCommand::Granted { .. }) => {
                    let artifacts = match self.commands.remove(&lease) {
                        Some(HeldCommand::Granted { artifacts, .. }) => artifacts,
                        _ => None,
                    };
                    let _reads = close_command_artifacts(artifacts, true);
                    let recorded = self.draining_recorded(&lease);
                    let cleanup = if recorded {
                        Cleanup::NotHeld
                    } else {
                        Cleanup::Uncertain
                    };
                    let cleanup = self.end_unrun_with(&lease, EndCause::Drained, cleanup);
                    // The gateway takes no `Ending` for a command: its `Run`
                    // (answered never started) or `End` tells it, so the
                    // drain waits for one.
                    self.unrun.insert(lease, cleanup);
                }
                _ => {}
            }
        }
    }

    /// Record that `lease` is ending because the host drains: whether that
    /// was recorded.
    fn draining_recorded(&self, lease: &str) -> bool {
        let entry = LedgerEntry::Draining {
            lease: lease.into(),
            peer: self.serving.peer,
        };
        match self.ledger.record(&entry) {
            Ok(()) => true,
            Err(error) => {
                tracing::error!(lease, %error, "a lease's drain could not be recorded; it is ended still");
                false
            }
        }
    }

    /// Whether a drain has left nothing to serve: no lease held, no command
    /// still held (a finished one included, until its `End`), the `End` of
    /// every lease this side ended unasked asked, the `Run` or `End` of
    /// every command it ended unrun sent, and every answer queued here
    /// handed to the writer. Answers already handed on, or still awaiting
    /// an end's record, reach the gateway before the stream closes.
    fn drained(&self) -> bool {
        self.draining
            && self.leases.is_empty()
            && self.commands.is_empty()
            && self.unended.is_empty()
            && self.unrun.is_empty()
            && self.outgoing.is_empty()
    }

    /// The gateway is gone: end every lease it held as lost, and wait for
    /// every end under way.
    async fn lose_all(&mut self) {
        // Said first: nothing waits for room to answer a gateway that is
        // gone, an end already under way (an expiry's among them) included.
        self.gone.send_replace(true);
        let leases: Vec<_> = self.leases.drain().collect();
        for (lease, held) in leases {
            tracing::warn!(lease, "the gateway's connection ended; its lease is lost");
            self.end(lease, held, EndCause::Lost, true);
        }
        let commands: Vec<_> = self.commands.drain().collect();
        for (lease, held) in commands {
            match held {
                HeldCommand::Running {
                    stop,
                    ran,
                    artifacts,
                    ..
                } => {
                    let _reads = close_command_artifacts(artifacts, true);
                    ask_stop(&stop, CommandStop::Lost);
                    self.ends.insert(lease, ran);
                }
                HeldCommand::Granted { artifacts, .. } => {
                    let _reads = close_command_artifacts(artifacts, true);
                    self.end_unrun(&lease, EndCause::Lost);
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

/// Resolves once `drain` says true; never, once nothing can say it.
async fn drain_asked(drain: &mut watch::Receiver<bool>) {
    if drain.wait_for(|drain| *drain).await.is_err() {
        std::future::pending::<()>().await;
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
