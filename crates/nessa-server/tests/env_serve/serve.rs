//! The environment role's frame loop, driven in process over a pipe: what it
//! answers, what it records, and what it does when the gateway goes away.
use super::super::fetches::{Destination, FetchedFile, FileIdentity, PlacedFile, Placement};
use super::FetchAsked;
use super::*;
use crate::env_serve::domain::{AgentName, Agents, CommandGrant, MaxLease, PeerGrant};
use crate::timer::{manual::ManualTimer, TokioTimer};
use nessa_protocol::lease::{KEEPALIVE_INTERVAL, SILENCE_LIMIT};
use nessa_sdk::application::agent_execution::providers::{CloseOutcome, HarnessCleanupFuture};
use std::sync::{
    atomic::{AtomicBool, AtomicUsize, Ordering},
    Mutex,
};
use tokio::io::{duplex, split, AsyncReadExt, AsyncWriteExt, DuplexStream, ReadHalf, WriteHalf};

/// A harness that echoes its input, and counts how often it was stopped.
/// Its stop takes `slow`. A `deaf` one never reads its input, until it is
/// stopped. A `leisurely` one reads its input a little at a time, echoes
/// nothing, and leaves by itself a while after its input ends; its stop is
/// forced unless it left within the grace.
struct EchoLauncher {
    stopped: Arc<AtomicUsize>,
    launched: Mutex<Vec<BTreeMap<String, String>>>,
    /// The publish point each launch was given.
    points: Mutex<Vec<Option<String>>>,
    slow: Mutex<Duration>,
    deaf: AtomicBool,
    leisurely: AtomicBool,
}

struct EchoControl {
    stopped: Arc<AtomicUsize>,
    slow: Duration,
    /// Lets a deaf harness go once it is stopped.
    released: Arc<tokio::sync::Notify>,
    /// A leisurely harness's leaving by itself.
    left: Option<tokio::sync::watch::Receiver<bool>>,
}

impl HarnessControl for EchoControl {
    fn cleanup(&mut self, grace: Duration, _kill: Duration) -> HarnessCleanupFuture<'_> {
        let slow = self.slow;
        Box::pin(async move {
            tokio::time::sleep(slow).await;
            let forced = match &mut self.left {
                Some(left) => tokio::time::timeout(grace, left.wait_for(|left| *left))
                    .await
                    .is_err(),
                None => false,
            };
            self.released.notify_one();
            self.stopped.fetch_add(1, Ordering::SeqCst);
            Ok(CloseOutcome { forced })
        })
    }
}

impl HarnessLauncher for EchoLauncher {
    fn workspace(&self) -> &str {
        "/work"
    }
    fn runs(&self, agent: &str) -> bool {
        agent == "claude" || agent == "codex"
    }
    fn sandboxes(&self) -> SandboxProfiles {
        SandboxProfiles::HARNESS_DEFAULT
    }
    fn launch(
        &self,
        _agent: &str,
        environment: &BTreeMap<String, String>,
        publish_point: Option<&str>,
    ) -> Result<HarnessProcess, AgentError> {
        self.launched.lock().unwrap().push(environment.clone());
        self.points
            .lock()
            .unwrap()
            .push(publish_point.map(str::to_owned));
        let (input, mut harness_in) = duplex(4096);
        let (mut harness_out, output) = duplex(4096);
        let released = Arc::new(tokio::sync::Notify::new());
        let deaf = self.deaf.load(Ordering::SeqCst).then(|| released.clone());
        let (leaving, left) = tokio::sync::watch::channel(false);
        let left = self.leisurely.load(Ordering::SeqCst).then_some(left);
        let leisurely = left.is_some();
        tokio::spawn(async move {
            if leisurely {
                let mut buffer = [0; 4096];
                while let Ok(read) = harness_in.read(&mut buffer).await {
                    if read == 0 {
                        tokio::time::sleep(Duration::from_millis(300)).await;
                        leaving.send_replace(true);
                        return;
                    }
                    tokio::time::sleep(Duration::from_millis(20)).await;
                }
                return;
            }
            if let Some(released) = deaf {
                released.notified().await;
                return;
            }
            let mut buffer = [0; 256];
            loop {
                match harness_in.read(&mut buffer).await {
                    Ok(0) | Err(_) => break,
                    Ok(read) => {
                        if harness_out.write_all(&buffer[..read]).await.is_err() {
                            break;
                        }
                    }
                }
            }
        });
        Ok(HarnessProcess {
            input: Box::new(input),
            output: Box::new(output),
            control: Box::new(EchoControl {
                stopped: self.stopped.clone(),
                slow: *self.slow.lock().unwrap(),
                released,
                left,
            }),
        })
    }
}

/// Commands that answer at once (`echo`, with its arguments as output) or
/// run until they are stopped (anything else), and remember what ran and
/// which publish point each was given.
#[derive(Default)]
struct FakeCommands {
    ran: Mutex<Vec<CommandWork>>,
    /// The publish point of each command, in order; `None` when it had none.
    points: Mutex<Vec<Option<String>>>,
}

impl CommandRunner for FakeCommands {
    fn run(
        &self,
        command: CommandWork,
        mut stop: watch::Receiver<Option<CommandStop>>,
        publish_point: Option<String>,
    ) -> Pin<Box<dyn Future<Output = CommandRan> + Send + 'static>> {
        self.ran.lock().unwrap().push(command.clone());
        self.points.lock().unwrap().push(publish_point);
        Box::pin(async move {
            if command.program() == "echo" {
                let said: Vec<&str> = command.argv()[1..].iter().map(|a| &**a).collect();
                return CommandRan {
                    end: CommandEnd::Exited { code: 0 },
                    stdout: said.join(" ").into_bytes(),
                    stderr: b"e".to_vec(),
                    dropped_bytes: 3,
                    cleanup: Cleanup::Confirmed { forced: false },
                };
            }
            let _ = stop.wait_for(Option::is_some).await;
            CommandRan {
                end: CommandEnd::Stopped,
                stdout: b"partial".to_vec(),
                stderr: Vec::new(),
                dropped_bytes: 0,
                cleanup: Cleanup::Confirmed { forced: true },
            }
        })
    }
}

#[derive(Default)]
struct MemoryLedger {
    entries: Mutex<Vec<LedgerEntry>>,
    failing: AtomicBool,
    /// A collection takes a while to be recorded.
    slow_collections: AtomicBool,
    /// A publish takes a while to be recorded, and says when it starts.
    slow_publishes: AtomicBool,
    publishing: tokio::sync::Notify,
    /// Refuses only a command's exit, as a disk that went away for one write.
    failing_command_ran: AtomicBool,
    /// Refuses only a fetch's acceptance.
    failing_fetch_accepted: AtomicBool,
    /// Refuses only a fetch's last word.
    failing_fetched: AtomicBool,
    /// Refuses only a fetch recorded unanswered.
    failing_fetch_unanswered: AtomicBool,
    /// Refuses only a lease's expiry, likewise.
    failing_expiring: AtomicBool,
    /// Refuses only a lease's revocation, likewise.
    failing_revoking: AtomicBool,
    /// Refuses only a lease's drain, likewise.
    failing_draining: AtomicBool,
}

impl MemoryLedger {
    fn entries(&self) -> Vec<LedgerEntry> {
        self.entries.lock().unwrap().clone()
    }
}

impl LeaseLedger for MemoryLedger {
    fn record(&self, entry: &LedgerEntry) -> io::Result<()> {
        if self.failing.load(Ordering::SeqCst)
            || (matches!(entry, LedgerEntry::CommandRan { .. })
                && self.failing_command_ran.load(Ordering::SeqCst))
            || (matches!(entry, LedgerEntry::FetchAccepted { .. })
                && self.failing_fetch_accepted.load(Ordering::SeqCst))
            || (matches!(entry, LedgerEntry::Fetched { .. })
                && self.failing_fetched.load(Ordering::SeqCst))
            || (matches!(entry, LedgerEntry::FetchUnanswered { .. })
                && self.failing_fetch_unanswered.load(Ordering::SeqCst))
            || (matches!(entry, LedgerEntry::Expiring { .. })
                && self.failing_expiring.load(Ordering::SeqCst))
            || (matches!(entry, LedgerEntry::Revoking { .. })
                && self.failing_revoking.load(Ordering::SeqCst))
            || (matches!(entry, LedgerEntry::Draining { .. })
                && self.failing_draining.load(Ordering::SeqCst))
        {
            return Err(io::Error::other("disk gone"));
        }
        if matches!(entry, LedgerEntry::Collected { .. })
            && self.slow_collections.load(Ordering::SeqCst)
        {
            std::thread::sleep(Duration::from_millis(200));
        }
        if matches!(entry, LedgerEntry::Published { .. })
            && self.slow_publishes.load(Ordering::SeqCst)
        {
            self.publishing.notify_one();
            std::thread::sleep(Duration::from_millis(200));
        }
        self.entries.lock().unwrap().push(entry.clone());
        Ok(())
    }
    fn accounted(&self, lease: &str) -> io::Result<Cleanup> {
        let entries = self.entries.lock().unwrap();
        let mut granted = false;
        let mut ended = None;
        for entry in entries.iter() {
            match entry {
                LedgerEntry::Granted { lease: id, .. }
                | LedgerEntry::CommandGranted { lease: id, .. }
                    if id == lease =>
                {
                    granted = true;
                    ended = None;
                }
                LedgerEntry::Ended {
                    lease: id, cleanup, ..
                } if id == lease => ended = Some(*cleanup),
                _ => {}
            }
        }
        Ok(ended.unwrap_or(if granted {
            Cleanup::Uncertain
        } else {
            Cleanup::NotHeld
        }))
    }
}

/// Grants as a test sets them: every peer's entry, or a failure to read.
struct MemoryGrants {
    entries: Mutex<Result<HashMap<Peer, PeerGrant>, String>>,
}

impl MemoryGrants {
    fn none() -> Arc<Self> {
        Arc::new(Self {
            entries: Mutex::new(Ok(HashMap::new())),
        })
    }
    fn set(&self, peer: Peer, grant: Option<PeerGrant>) {
        let mut entries = self.entries.lock().unwrap();
        let entries = entries.as_mut().unwrap();
        match grant {
            Some(grant) => entries.insert(peer, grant),
            None => entries.remove(&peer),
        };
    }
}

impl PeerGrants for MemoryGrants {
    fn entry(&self, peer: &Peer) -> Result<Option<PeerGrant>, String> {
        self.entries
            .lock()
            .unwrap()
            .clone()
            .map(|entries| entries.get(peer).cloned())
    }
}

/// A dialed gateway's peer, as the tests name it.
const DIALED: Peer = Peer::Gateway { pin: [5; 44] };

fn harness_terms() -> GrantedTerms {
    GrantedTerms {
        sandbox: Sandbox::HarnessDefault,
        timeout_ms: None,
        lease_ms: None,
    }
}

fn command_terms(timeout_ms: u64) -> GrantedTerms {
    GrantedTerms {
        sandbox: Sandbox::None,
        timeout_ms: Some(timeout_ms),
        lease_ms: None,
    }
}

/// What most tests here are served on: what they check is frames, and the
/// harnesses they stop leave on the runtime's clock, which a stop's grace and
/// kill bounds have to be measured against.
fn real_time() -> Arc<dyn Timer> {
    Arc::new(TokioTimer::new())
}

struct Gateway {
    frames: FrameStream<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    served: JoinHandle<()>,
    ledger: Arc<MemoryLedger>,
    stopped: Arc<AtomicUsize>,
    launcher: Arc<EchoLauncher>,
    commands: Arc<FakeCommands>,
    grants: Arc<MemoryGrants>,
    /// Asks the host to drain, as its composition does at a signal.
    drain: watch::Sender<bool>,
}

impl Gateway {
    fn start() -> Self {
        Self::with(Arc::new(MemoryLedger::default()))
    }
    fn with(ledger: Arc<MemoryLedger>) -> Self {
        Self::serving(ledger, true)
    }
    fn serving(ledger: Arc<MemoryLedger>, runs_commands: bool) -> Self {
        Self::over(ledger, Arc::new(NoOutbox), runs_commands, real_time())
    }
    /// Served on `timer`, which only the test moves.
    fn timed(timer: Arc<ManualTimer>) -> Self {
        Self::over(
            Arc::new(MemoryLedger::default()),
            Arc::new(NoOutbox),
            true,
            timer,
        )
    }
    fn over(
        ledger: Arc<MemoryLedger>,
        outbox: Arc<dyn ArtifactOutbox>,
        runs_commands: bool,
        timer: Arc<dyn Timer>,
    ) -> Self {
        Self::through(
            1 << 20,
            Peer::Ssh,
            MemoryGrants::none(),
            ledger,
            outbox,
            None,
            runs_commands,
            timer,
        )
    }
    /// Serving `peer`, granted as `grants` say, on `timer`.
    fn for_peer(
        peer: Peer,
        grants: Arc<MemoryGrants>,
        ledger: Arc<MemoryLedger>,
        outbox: Arc<dyn ArtifactOutbox>,
        runs_commands: bool,
        timer: Arc<dyn Timer>,
    ) -> Self {
        Self::through(
            1 << 20,
            peer,
            grants,
            ledger,
            outbox,
            None,
            runs_commands,
            timer,
        )
    }
    /// [`Self::over`], taking fetched files into `inbox` when one is given.
    fn taking(
        ledger: Arc<MemoryLedger>,
        outbox: Arc<dyn ArtifactOutbox>,
        inbox: Option<Arc<dyn ArtifactInbox>>,
        runs_commands: bool,
        timer: Arc<dyn Timer>,
    ) -> Self {
        Self::through(
            1 << 20,
            Peer::Ssh,
            MemoryGrants::none(),
            ledger,
            outbox,
            inbox,
            runs_commands,
            timer,
        )
    }
    /// Serving `peer`, granted as `grants` say, taking fetched files into
    /// `inbox` when one is given, over a connection holding at most
    /// `buffer` bytes each way that are not yet read.
    #[allow(clippy::too_many_arguments, reason = "a test harness's one builder")]
    fn through(
        buffer: usize,
        peer: Peer,
        grants: Arc<MemoryGrants>,
        ledger: Arc<MemoryLedger>,
        outbox: Arc<dyn ArtifactOutbox>,
        inbox: Option<Arc<dyn ArtifactInbox>>,
        runs_commands: bool,
        timer: Arc<dyn Timer>,
    ) -> Self {
        let (gateway, environment) = duplex(buffer);
        let (environment_in, environment_out) = split(environment);
        let stopped = Arc::new(AtomicUsize::new(0));
        let launcher = Arc::new(EchoLauncher {
            stopped: stopped.clone(),
            launched: Mutex::new(Vec::new()),
            points: Mutex::new(Vec::new()),
            slow: Mutex::new(Duration::ZERO),
            deaf: AtomicBool::new(false),
            leisurely: AtomicBool::new(false),
        });
        let commands = Arc::new(FakeCommands::default());
        let runner = runs_commands.then(|| commands.clone() as Arc<dyn CommandRunner>);
        let (drain, draining) = watch::channel(false);
        let served = tokio::spawn(serve(
            environment_in,
            environment_out,
            "1.2.3",
            "protocol",
            Serving {
                peer,
                grants: grants.clone(),
                policy: Arc::new(
                    crate::env_serve::infrastructure::CedarLeasePolicy::new().unwrap(),
                ),
            },
            Runners {
                harnesses: launcher.clone(),
                commands: runner,
            },
            ledger.clone(),
            outbox,
            inbox,
            ServeTimings {
                grace: Duration::from_millis(10),
                kill: Duration::from_millis(100),
                ..ServeTimings::default()
            },
            timer,
            draining,
        ));
        let (reader, writer) = split(gateway);
        Self {
            frames: FrameStream::new(reader),
            writer,
            served,
            ledger,
            stopped,
            launcher,
            commands,
            grants,
            drain,
        }
    }
    async fn send(&mut self, frame: ToEnvironment) {
        write_frame(&mut self.writer, &frame).await.unwrap();
    }
    /// The next frame that says something: a keepalive only says the host
    /// is there, and comes whenever it has been quiet.
    async fn next(&mut self) -> FromEnvironment {
        loop {
            match self.frame().await {
                FromEnvironment::Keepalive => {}
                frame => return frame,
            }
        }
    }
    async fn frame(&mut self) -> FromEnvironment {
        let body = tokio::time::timeout(Duration::from_secs(5), self.frames.next())
            .await
            .expect("a frame in time")
            .unwrap()
            .expect("the stream is open");
        decode(&body).unwrap()
    }
    async fn granted(&mut self, lease: &str) {
        self.send(ToEnvironment::Grant {
            lease: lease.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
        assert_eq!(
            self.next().await,
            FromEnvironment::Granted {
                lease: lease.into(),
                terms: harness_terms(),
            }
        );
    }
}

const LEASE: &str = "0b7d3a1e-0000-4000-8000-000000000001";

fn hello() -> FromEnvironment {
    FromEnvironment::Hello {
        build: "1.2.3".into(),
        protocol: "protocol".into(),
        workspace: "/work".into(),
    }
}

/// Gate 1 and 2: a lease runs a harness whose bytes are carried both ways
/// under its lease and channel; its end stops the harness and is recorded
/// with what that took before it is answered.
#[tokio::test]
async fn a_lease_runs_a_harness_and_its_end_is_answered_with_recorded_cleanup() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::from([("ANTHROPIC_MODEL".into(), "m".into())]),
        })
        .await;
    gateway
        .send(ToEnvironment::Input {
            lease: LEASE.into(),
            channel: 1,
            data: Data(b"ping\n".to_vec()),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Output {
            lease: LEASE.into(),
            channel: 1,
            data: Data(b"ping\n".to_vec()),
        }
    );
    assert_eq!(
        gateway.launcher.launched.lock().unwrap()[0]["ANTHROPIC_MODEL"],
        "m"
    );
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    let ended = loop {
        match gateway.next().await {
            FromEnvironment::OutputClosed { .. } => continue,
            other => break other,
        }
    };
    assert_eq!(
        ended,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        gateway.ledger.entries(),
        [
            LedgerEntry::Granted {
                lease: LEASE.into(),
                peer: Peer::Ssh,
                agent: "claude".into(),
                sandbox: Sandbox::HarnessDefault,
                lease_ms: None,
            },
            LedgerEntry::Started {
                lease: LEASE.into(),
                channel: 1,
            },
            LedgerEntry::Stopped {
                lease: LEASE.into(),
                channel: 1,
                cleanup: Cleanup::Confirmed { forced: false },
            },
            LedgerEntry::Ended {
                lease: LEASE.into(),
                cleanup: Cleanup::Confirmed { forced: false },
                cause: EndCause::Asked,
            },
        ]
    );
}

/// Gate 3: a frame naming a lease or harness this connection does not hold
/// is dropped and recorded, never applied to another; a start under a lease
/// not granted starts nothing.
#[tokio::test]
async fn frames_naming_nothing_held_are_dropped_with_evidence() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(ToEnvironment::Input {
            lease: "other".into(),
            channel: 1,
            data: Data(b"x".to_vec()),
        })
        .await;
    gateway
        .send(ToEnvironment::Start {
            lease: "other".into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::StartFailed {
            lease: "other".into(),
            channel: 1,
            reason: StartFailure::NotGranted,
        }
    );
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ended { .. }
    ));
    // Late: the lease has ended.
    gateway
        .send(ToEnvironment::InputClosed {
            lease: LEASE.into(),
            channel: 1,
        })
        .await;
    gateway
        .send(ToEnvironment::Account {
            lease: LEASE.into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Accounted {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    let drops: Vec<_> = gateway
        .ledger
        .entries()
        .into_iter()
        .filter_map(|entry| match entry {
            LedgerEntry::Dropped { lease, frame, .. } => Some((lease, frame)),
            _ => None,
        })
        .collect();
    assert_eq!(
        drops,
        [
            (Some("other".into()), "input".into()),
            (Some("other".into()), "start".into()),
            (Some(LEASE.into()), "input_closed".into()),
        ]
    );
    assert_eq!(gateway.launcher.launched.lock().unwrap().len(), 0);
}

/// Gate 2: the gateway's stream ending is every lease it held lost: each
/// harness is stopped and the end recorded as lost with its cleanup, which a
/// later connection is answered from.
#[tokio::test]
async fn the_gateway_gone_ends_every_lease_as_lost_with_recorded_cleanup() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 7,
            environment: BTreeMap::new(),
        })
        .await;
    let Gateway {
        writer,
        frames,
        served,
        ledger,
        stopped,
        ..
    } = gateway;
    drop(writer);
    drop(frames);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("serving ends with its stream")
        .unwrap();
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
            cause: EndCause::Lost,
        })
    );
    let mut later = Gateway::with(ledger);
    assert_eq!(later.next().await, hello());
    later
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert_eq!(
        later.next().await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
}

#[tokio::test]
async fn account_answers_only_what_was_recorded() {
    let ledger = Arc::new(MemoryLedger::default());
    for entry in [
        LedgerEntry::Granted {
            lease: "ended".into(),
            peer: Peer::Ssh,
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
            lease_ms: None,
        },
        LedgerEntry::Ended {
            lease: "ended".into(),
            cleanup: Cleanup::Confirmed { forced: true },
            cause: EndCause::Lost,
        },
        LedgerEntry::Granted {
            lease: "unfinished".into(),
            peer: Peer::Ssh,
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
            lease_ms: None,
        },
    ] {
        ledger.record(&entry).unwrap();
    }
    let mut gateway = Gateway::with(ledger);
    assert_eq!(gateway.next().await, hello());
    for (lease, cleanup) in [
        ("ended", Cleanup::Confirmed { forced: true }),
        ("unfinished", Cleanup::Uncertain),
        ("never", Cleanup::NotHeld),
    ] {
        gateway
            .send(ToEnvironment::Account {
                lease: lease.into(),
            })
            .await;
        assert_eq!(
            gateway.next().await,
            FromEnvironment::Accounted {
                lease: lease.into(),
                cleanup,
            }
        );
    }
}

/// A lease is admitted only for an agent the host runs, once, and only once
/// it is recorded.
#[tokio::test]
async fn grants_are_refused_with_their_reason() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(ToEnvironment::Grant {
            lease: LEASE.into(),
            agent: "opencode".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: LEASE.into(),
            reason: GrantRefusal::AgentUnavailable,
        }
    );
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Grant {
            lease: LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: LEASE.into(),
            reason: GrantRefusal::Duplicate,
        }
    );
    // Ended, its id still names it: never granted again, on this
    // connection or a later one sharing the host's audit.
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ended { .. }
    ));
    for _ in 0..2 {
        gateway
            .send(ToEnvironment::Grant {
                lease: LEASE.into(),
                agent: "claude".into(),
                sandbox: Sandbox::HarnessDefault,
            })
            .await;
        assert_eq!(
            gateway.next().await,
            FromEnvironment::Refused {
                lease: LEASE.into(),
                reason: GrantRefusal::Duplicate,
            }
        );
        let ledger = gateway.ledger.clone();
        gateway = Gateway::with(ledger);
        assert_eq!(gateway.next().await, hello());
    }
    gateway.ledger.failing.store(true, Ordering::SeqCst);
    gateway
        .send(ToEnvironment::Grant {
            lease: "second".into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: "second".into(),
            reason: GrantRefusal::AuditUnavailable,
        }
    );
}

fn claude_only() -> PeerGrant {
    PeerGrant::new(
        Agents::Named([AgentName::new("claude").unwrap()].into()),
        None,
    )
}

/// Row I4: a dialed gateway the owner granted nothing is refused every
/// lease `not_granted`, recorded, and nothing starts or runs for it.
#[tokio::test]
async fn i4_a_dialed_gateway_without_a_grant_is_refused_and_nothing_runs() {
    let mut gateway = Gateway::for_peer(
        DIALED,
        MemoryGrants::none(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        true,
        real_time(),
    );
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(ToEnvironment::Grant {
            lease: LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: LEASE.into(),
            reason: GrantRefusal::NotGranted,
        }
    );
    gateway
        .send(grant_command(COMMAND, &["echo", "a"], None))
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: COMMAND.into(),
            reason: GrantRefusal::NotGranted,
        }
    );
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::StartFailed {
            lease: LEASE.into(),
            channel: 1,
            reason: StartFailure::NotGranted,
        }
    );
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ran {
            end: CommandEnd::NotStarted,
            ..
        }
    ));
    assert!(gateway.launcher.launched.lock().unwrap().is_empty());
    assert!(gateway.commands.ran.lock().unwrap().is_empty());
    assert_eq!(
        gateway.ledger.entries()[..2],
        [
            LedgerEntry::Refused {
                lease: LEASE.into(),
                peer: DIALED,
                reason: GrantRefusal::NotGranted,
                asked: Asked::Agent {
                    agent: "claude".into(),
                    sandbox: Sandbox::HarnessDefault,
                },
            },
            LedgerEntry::Refused {
                lease: COMMAND.into(),
                peer: DIALED,
                reason: GrantRefusal::NotGranted,
                asked: Asked::Command {
                    argv: vec!["echo".into(), "a".into()],
                    cwd: None,
                    asked_timeout_ms: 1_000,
                },
            },
        ]
    );
}

/// Row I4: the peer's grant is checked before what the host runs, so a
/// gateway not granted a lease learns nothing of it: an agent the host does
/// not run, a sandbox it cannot enforce, and commands on a host that runs
/// none are each refused `NotGranted`. Once granted, it is told why.
#[tokio::test]
async fn an_ungranted_gateway_learns_nothing_of_what_the_host_runs() {
    let grants = MemoryGrants::none();
    let mut gateway = Gateway::for_peer(
        DIALED,
        grants.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        false,
        real_time(),
    );
    assert_eq!(gateway.next().await, hello());
    let asks = |n: u8| {
        [
            ToEnvironment::Grant {
                lease: format!("lease-unknown-{n}"),
                agent: "unknown".into(),
                sandbox: Sandbox::HarnessDefault,
            },
            ToEnvironment::Grant {
                lease: format!("lease-unsandboxed-{n}"),
                agent: "claude".into(),
                sandbox: Sandbox::None,
            },
            grant_command(&format!("command-{n}"), &["echo", "a"], None),
        ]
    };
    for ask in asks(1) {
        gateway.send(ask).await;
        assert!(
            matches!(
                gateway.next().await,
                FromEnvironment::Refused {
                    reason: GrantRefusal::NotGranted,
                    ..
                }
            ),
            "refused not granted"
        );
    }
    grants.set(
        DIALED,
        Some(PeerGrant::new(
            Agents::All,
            Some(CommandGrant::new(CommandWork::MAX_TIMEOUT_MS).unwrap()),
        )),
    );
    for (ask, reason) in asks(2).into_iter().zip([
        GrantRefusal::AgentUnavailable,
        GrantRefusal::SandboxUnavailable,
        GrantRefusal::CommandsUnavailable,
    ]) {
        gateway.send(ask).await;
        assert!(
            matches!(gateway.next().await, FromEnvironment::Refused { reason: got, .. } if got == reason),
            "{reason:?}"
        );
    }
}

/// Row I6: a grant is read at every lease, so an ungrant refuses the next
/// lease while the lease already running goes on; a grant names agents,
/// and one it leaves out is refused.
#[tokio::test]
async fn i6_an_ungrant_applies_to_the_next_lease_and_running_leases_stay() {
    let grants = MemoryGrants::none();
    grants.set(DIALED, Some(claude_only()));
    let mut gateway = Gateway::for_peer(
        DIALED,
        grants.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        true,
        real_time(),
    );
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Grant {
            lease: "codex-lease".into(),
            agent: "codex".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: "codex-lease".into(),
            reason: GrantRefusal::NotGranted,
        }
    );
    grants.set(DIALED, None);
    gateway
        .send(ToEnvironment::Grant {
            lease: "next".into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: "next".into(),
            reason: GrantRefusal::NotGranted,
        }
    );
    // The lease granted before the ungrant still runs its harness.
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    gateway
        .send(ToEnvironment::Input {
            lease: LEASE.into(),
            channel: 1,
            data: Data(b"still\n".to_vec()),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Output {
            lease: LEASE.into(),
            channel: 1,
            data: Data(b"still\n".to_vec()),
        }
    );
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    let ended = loop {
        match gateway.next().await {
            FromEnvironment::OutputClosed { .. } => continue,
            other => break other,
        }
    };
    assert!(matches!(ended, FromEnvironment::Ended { .. }));
}

/// Row I5 on the host: a command's timeout is narrowed to the peer's
/// grant, recorded as granted beside what was asked, answered as granted,
/// and the command runs with it.
#[tokio::test]
async fn i5_a_command_timeout_is_narrowed_recorded_and_run_with() {
    let grants = MemoryGrants::none();
    grants.set(
        DIALED,
        Some(PeerGrant::new(
            Agents::Named(Default::default()),
            Some(CommandGrant::new(400).unwrap()),
        )),
    );
    let mut gateway = Gateway::for_peer(
        DIALED,
        grants,
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        true,
        real_time(),
    );
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(grant_command(COMMAND, &["echo", "a"], None))
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Granted {
            lease: COMMAND.into(),
            terms: command_terms(400),
        }
    );
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ran {
            end: CommandEnd::Exited { code: 0 },
            ..
        }
    ));
    assert_eq!(gateway.commands.ran.lock().unwrap()[0].timeout_ms(), 400);
    assert_eq!(
        gateway.ledger.entries()[0],
        LedgerEntry::CommandGranted {
            lease: COMMAND.into(),
            peer: DIALED,
            argv: vec!["echo".into(), "a".into()],
            cwd: None,
            timeout_ms: 400,
            asked_timeout_ms: 1_000,
            sandbox: Sandbox::None,
        }
    );
}

/// SSH with no grants at all keeps what it always had: every agent the
/// host runs, and commands for as long as asked. An entry for it narrows
/// it like any peer's.
#[tokio::test]
async fn ssh_without_grants_keeps_everything_the_host_runs() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Grant {
            lease: "codex-lease".into(),
            agent: "codex".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Granted {
            lease: "codex-lease".into(),
            terms: harness_terms(),
        }
    );
    gateway
        .send(grant_command(COMMAND, &["echo", "a"], None))
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Granted {
            lease: COMMAND.into(),
            terms: command_terms(1_000),
        }
    );
    gateway.grants.set(Peer::Ssh, Some(claude_only()));
    gateway
        .send(ToEnvironment::Grant {
            lease: "narrowed".into(),
            agent: "codex".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: "narrowed".into(),
            reason: GrantRefusal::NotGranted,
        }
    );
}

/// Grants that cannot be read grant nothing, to SSH too: a broken file is
/// not taken as no file.
#[tokio::test]
async fn grants_that_cannot_be_read_refuse_every_lease() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    *gateway.grants.entries.lock().unwrap() = Err("grants.json is not this shape".into());
    gateway
        .send(ToEnvironment::Grant {
            lease: LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: LEASE.into(),
            reason: GrantRefusal::NotGranted,
        }
    );
    gateway
        .send(grant_command(COMMAND, &["echo", "a"], None))
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: COMMAND.into(),
            reason: GrantRefusal::NotGranted,
        }
    );
}

/// A sandbox the host cannot enforce is refused, and recorded so: the host
/// never puts another in its place.
#[tokio::test]
async fn a_sandbox_the_host_cannot_enforce_is_refused() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(ToEnvironment::Grant {
            lease: LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::None,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: LEASE.into(),
            reason: GrantRefusal::SandboxUnavailable,
        }
    );
    assert_eq!(
        gateway.ledger.entries(),
        [LedgerEntry::Refused {
            lease: LEASE.into(),
            peer: Peer::Ssh,
            reason: GrantRefusal::SandboxUnavailable,
            asked: Asked::Agent {
                agent: "claude".into(),
                sandbox: Sandbox::None,
            },
        }]
    );
    assert!(gateway.launcher.launched.lock().unwrap().is_empty());
}

/// A harness's stop is answered with its cleanup; a stop for a harness not
/// running is answered as holding nothing, so the gateway is never left
/// waiting.
#[tokio::test]
async fn a_stop_is_answered_with_the_harness_cleanup() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    for channel in [1, 2] {
        gateway
            .send(ToEnvironment::Stop {
                lease: LEASE.into(),
                channel,
                grace_ms: 10,
                kill_ms: 100,
            })
            .await;
    }
    let mut answers = Vec::new();
    while answers.len() < 2 {
        match gateway.next().await {
            FromEnvironment::Stopped {
                channel, cleanup, ..
            } => answers.push((channel, cleanup)),
            FromEnvironment::OutputClosed { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    answers.sort_by_key(|(channel, _)| *channel);
    assert_eq!(
        answers,
        [
            (1, Cleanup::Confirmed { forced: false }),
            (2, Cleanup::NotHeld)
        ]
    );
    // Asked again, a stopped harness is answered with its own cleanup,
    // never as one that held nothing, and its channel is not started again.
    gateway
        .send(ToEnvironment::Stop {
            lease: LEASE.into(),
            channel: 1,
            grace_ms: 10,
            kill_ms: 100,
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Stopped {
            lease: LEASE.into(),
            channel: 1,
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::StartFailed {
            lease: LEASE.into(),
            channel: 1,
            reason: StartFailure::NotGranted,
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(gateway.launcher.launched.lock().unwrap().len(), 1);
}

/// A second stop that arrives while the first is still stopping the harness
/// waits for that stop's cleanup instead of answering that nothing ran.
#[tokio::test]
async fn a_stop_asked_twice_while_stopping_answers_the_same_cleanup() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    for _ in 0..2 {
        gateway
            .send(ToEnvironment::Stop {
                lease: LEASE.into(),
                channel: 1,
                grace_ms: 10,
                kill_ms: 100,
            })
            .await;
    }
    let mut answers = Vec::new();
    while answers.len() < 2 {
        match gateway.next().await {
            FromEnvironment::Stopped { cleanup, .. } => answers.push(cleanup),
            FromEnvironment::OutputClosed { .. } => {}
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(answers, [Cleanup::Confirmed { forced: false }; 2]);
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_host_that_cannot_serve_says_its_protocol_then_why() {
    let (gateway, environment) = duplex(4096);
    refuse(
        environment,
        Hello {
            build: "1.2.3".into(),
            protocol: "protocol".into(),
            workspace: String::new(),
        },
        Unavailability::Busy,
    )
    .await
    .unwrap();
    let mut frames = FrameStream::new(gateway);
    let hello = read_hello(&frames.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(hello.build, "1.2.3");
    assert_eq!(hello.protocol, "protocol");
    assert_eq!(
        decode::<FromEnvironment>(&frames.next().await.unwrap().unwrap()).unwrap(),
        FromEnvironment::Unavailable {
            reason: Unavailability::Busy
        }
    );
    assert_eq!(frames.next().await.unwrap(), None);
}

/// A gateway that goes silent without closing its stream, as one cut off by
/// the network, is noticed: past the silence bound every lease it held is
/// ended as lost and recorded, and serving ends, so the next connection is
/// not refused as busy for as long as TCP takes to give up.
#[tokio::test]
async fn a_silent_gateway_is_lost_and_serving_ends() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = Gateway::timed(timer.clone());
    gateway.started(LEASE).await;
    // Just short of the bound after the last frame it is still served, as
    // the account answered here shows; that account is the last frame the
    // gateway sends. Its stream stays open, and the bound runs from it.
    timer.advance(SILENCE_LIMIT - Duration::from_millis(1));
    gateway.settled().await;
    timer.advance(SILENCE_LIMIT);
    let Gateway {
        writer: _writer,
        frames: _frames,
        served,
        ledger,
        stopped,
        ..
    } = gateway;
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("serving ends once the gateway is silent past its bound")
        .unwrap();
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
            cause: EndCause::Lost,
        })
    );
}

/// A gateway's keepalives are what keep a quiet connection served, and a
/// quiet host sends its own, so the gateway does not take it for lost.
#[tokio::test]
async fn keepalives_keep_a_quiet_gateway_served() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = Gateway::timed(timer.clone());
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    // Eight intervals, far past the silence bound, with only keepalives.
    for _ in 0..8 {
        timer.advance(KEEPALIVE_INTERVAL);
        gateway.send(ToEnvironment::Keepalive).await;
        // Read: what it is waited on for is measured from now.
        let heard = timer.now();
        timer
            .started(|_, wait| wait.deadline == heard + SILENCE_LIMIT)
            .await;
    }
    assert!(!gateway.served.is_finished());
    assert_eq!(gateway.frame().await, FromEnvironment::Keepalive);
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
}

/// What a lease's own end is still recording is that end's to answer: an
/// account, or a second end, asked meanwhile waits for it rather than
/// reading the ledger before the end is in it.
#[tokio::test]
async fn an_account_asked_while_the_lease_is_ending_answers_that_end() {
    let mut gateway = Gateway::start();
    *gateway.launcher.slow.lock().unwrap() = Duration::from_millis(300);
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    gateway
        .send(ToEnvironment::Account {
            lease: LEASE.into(),
        })
        .await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    let mut answers = Vec::new();
    while answers.len() < 3 {
        match gateway.next().await {
            FromEnvironment::OutputClosed { .. } => {}
            other => answers.push(other),
        }
    }
    let confirmed = Cleanup::Confirmed { forced: false };
    for expected in [
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: confirmed,
        },
        FromEnvironment::Accounted {
            lease: LEASE.into(),
            cleanup: confirmed,
        },
    ] {
        assert!(answers.contains(&expected), "{answers:?}");
    }
    assert!(
        answers.iter().all(|answer| !matches!(
            answer,
            FromEnvironment::Ended {
                cleanup: Cleanup::Uncertain,
                ..
            } | FromEnvironment::Accounted {
                cleanup: Cleanup::Uncertain,
                ..
            }
        )),
        "{answers:?}"
    );
}

use nessa_protocol::lease::read_hello;

/// The lease protocol names every source of the lease contract: each that
/// writes or reads its frames, each of `env serve` (how the host launches,
/// records and cleans up), each binding declaring the variables a launch may
/// set, and the harness cleanup both ends report. A change to any of them
/// changes the protocol, so mismatched builds are refused at the hello.
#[test]
fn every_source_of_the_lease_contract_names_the_protocol() {
    fn sources(directory: &std::path::Path, all: bool, found: &mut Vec<std::path::PathBuf>) {
        for entry in std::fs::read_dir(directory).unwrap() {
            let path = entry.unwrap().path();
            if path.is_dir() {
                sources(&path, all, found);
            } else if path.extension().is_some_and(|extension| extension == "rs") {
                let text = std::fs::read_to_string(&path).unwrap();
                if all
                    || text.contains("ToEnvironment")
                    || text.contains("FromEnvironment")
                    || text.contains("const LAUNCH_VARIABLES")
                {
                    found.push(path.canonicalize().unwrap());
                }
            }
        }
    }
    let crate_root = std::path::Path::new(env!("CARGO_MANIFEST_DIR"));
    let config = crate_root.join("src").join("env");
    let named: Vec<_> = crate::env::LEASE_PROTOCOL_SOURCES
        .iter()
        .map(|source| config.join(source).canonicalize().unwrap())
        .collect();
    let mut found = Vec::new();
    sources(&crate_root.join("src"), false, &mut found);
    sources(&crate_root.join("src/env_serve"), true, &mut found);
    sources(&crate_root.join("../nessa-protocol/src"), false, &mut found);
    sources(&crate_root.join("../nessa-sdk/src"), false, &mut found);
    let sdk = crate_root.join("../nessa-sdk/src/infrastructure");
    for cleanup in ["harness_process.rs", "process.rs"] {
        found.push(sdk.join(cleanup).canonicalize().unwrap());
    }
    let launch_variables = found
        .iter()
        .filter(|path| {
            std::fs::read_to_string(path)
                .unwrap()
                .contains("const LAUNCH_VARIABLES")
        })
        .count();
    assert!(launch_variables >= 2, "{found:?}");
    assert!(found.len() >= 12, "{found:?}");
    for source in found {
        assert!(
            named.contains(&source),
            "{source:?} is not in the lease protocol"
        );
    }
}

/// A harness that stops reading its input is never fed a truncated stream:
/// once its input queue is full the host records the overflow, stops it, and
/// tells the gateway with its `Stopped`, unasked; a later stop of it is
/// answered with the same cleanup.
#[tokio::test]
async fn input_a_harness_does_not_read_stops_it_and_says_so() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway.launcher.deaf.store(true, Ordering::SeqCst);
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    // Far more than the harness's pipe and the input queue hold.
    for _ in 0..(INPUT_QUEUE + 64) {
        gateway
            .send(ToEnvironment::Input {
                lease: LEASE.into(),
                channel: 1,
                data: Data(vec![b'x'; 1024]),
            })
            .await;
    }
    let stopped = FromEnvironment::Stopped {
        lease: LEASE.into(),
        channel: 1,
        cleanup: Cleanup::Confirmed { forced: false },
    };
    let said = loop {
        match gateway.next().await {
            FromEnvironment::OutputClosed { .. } => continue,
            other => break other,
        }
    };
    assert_eq!(said, stopped);
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    let entries = gateway.ledger.entries();
    let overflow = entries.iter().position(|entry| {
        *entry
            == LedgerEntry::InputOverflow {
                lease: LEASE.into(),
                channel: 1,
            }
    });
    let recorded = entries.iter().position(|entry| {
        *entry
            == LedgerEntry::Stopped {
                lease: LEASE.into(),
                channel: 1,
                cleanup: Cleanup::Confirmed { forced: false },
            }
    });
    assert!(overflow.is_some() && overflow < recorded, "{entries:?}");
    gateway
        .send(ToEnvironment::Stop {
            lease: LEASE.into(),
            channel: 1,
            grace_ms: 10,
            kill_ms: 100,
        })
        .await;
    assert_eq!(gateway.next().await, stopped);
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
}

/// An input overflow the host cannot record still stops the harness, but
/// settles nothing: its `Stopped` answers uncertain.
#[tokio::test]
async fn an_unrecorded_input_overflow_stops_the_harness_uncertain() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway.launcher.deaf.store(true, Ordering::SeqCst);
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    // Started is recorded before its first input is read.
    gateway
        .send(ToEnvironment::Input {
            lease: LEASE.into(),
            channel: 1,
            data: Data(vec![b'x'; 1024]),
        })
        .await;
    tokio::time::timeout(Duration::from_secs(5), async {
        while !gateway
            .ledger
            .entries()
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::Started { .. }))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the start is recorded");
    gateway.ledger.failing.store(true, Ordering::SeqCst);
    for _ in 0..(INPUT_QUEUE + 64) {
        gateway
            .send(ToEnvironment::Input {
                lease: LEASE.into(),
                channel: 1,
                data: Data(vec![b'x'; 1024]),
            })
            .await;
    }
    let said = loop {
        match gateway.next().await {
            FromEnvironment::OutputClosed { .. } => continue,
            other => break other,
        }
    };
    assert_eq!(
        said,
        FromEnvironment::Stopped {
            lease: LEASE.into(),
            channel: 1,
            cleanup: Cleanup::Uncertain,
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
}

/// A stop gives the harness its grace after the input it already accepted,
/// and the end of it, reached it: a harness that leaves by itself once its
/// input ends is not forced for input still on its way.
///
/// On the runtime's paused clock: what it measures is the harness's own
/// reading and leaving against the grace its cleanup is given, and the
/// harness runs on that clock, not on the host's timer.
#[tokio::test(start_paused = true)]
async fn a_stop_starts_its_grace_after_the_harness_has_its_input() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    gateway.launcher.leisurely.store(true, Ordering::SeqCst);
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    // Ten reads' worth: about 200 ms for the harness to take in, then 300 ms
    // to leave. The grace, 400 ms, covers the leaving, not both.
    for _ in 0..10 {
        gateway
            .send(ToEnvironment::Input {
                lease: LEASE.into(),
                channel: 1,
                data: Data(vec![b'x'; 4096]),
            })
            .await;
    }
    gateway
        .send(ToEnvironment::Stop {
            lease: LEASE.into(),
            channel: 1,
            grace_ms: 400,
            kill_ms: 100,
        })
        .await;
    let said = loop {
        match gateway.next().await {
            FromEnvironment::OutputClosed { .. } => continue,
            other => break other,
        }
    };
    assert_eq!(
        said,
        FromEnvironment::Stopped {
            lease: LEASE.into(),
            channel: 1,
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
}

/// A host that stages nothing: its leases run without a publish point.
pub(crate) struct NoOutbox;
impl crate::env_serve::application::ArtifactOutbox for NoOutbox {
    fn open(&self, _lease: &str) -> std::io::Result<crate::env_serve::application::PublishPoint> {
        Err(std::io::Error::other("no outbox"))
    }
    fn stage(
        &self,
        _lease: &str,
        _artifact: u32,
        _request: &crate::env_serve::application::PublishRequest,
    ) -> Result<nessa_protocol::lease::StagedArtifact, nessa_protocol::lease::PublishRefusal> {
        Err(nessa_protocol::lease::PublishRefusal::StagingFailed)
    }
    fn read(
        &self,
        _lease: &str,
        _artifact: u32,
        _offset: u64,
        _length: usize,
    ) -> std::io::Result<Vec<u8>> {
        Err(std::io::Error::other("no outbox"))
    }
    fn discard(&self, _lease: &str, _artifact: u32) {}
    fn close(&self, _lease: &str) {}
}

/// A host that stages every file as the same copy, and keeps the sending
/// end of each publish point it opened for the test to publish through.
#[derive(Default)]
struct RecordingOutbox {
    points: Mutex<HashMap<String, mpsc::Sender<crate::env_serve::application::PublishCall>>>,
    /// Every request staged, as the outbox was asked it.
    staging: Mutex<Vec<crate::env_serve::application::PublishRequest>>,
    discarded: Mutex<Vec<(String, u32)>>,
    closed: Mutex<Vec<String>>,
    /// When set, the next staging says it began, then waits to be let go.
    stall: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
    /// Every read of a staged copy, as asked: lease, artifact, offset and
    /// length.
    reads: Mutex<Vec<(String, u32, u64, usize)>>,
    /// While true, every read waits, at most ten seconds.
    reads_held: (Mutex<bool>, std::sync::Condvar),
    /// Every read fails, as of a copy that is gone.
    unreadable: Mutex<bool>,
    /// Told as each read begins, once set.
    read_begun: Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>>,
    /// Told as each read returns, once set.
    read_done: Mutex<Option<tokio::sync::mpsc::UnboundedSender<()>>>,
}

/// The bytes of every copy [`RecordingOutbox`] stages: as long as the size
/// it says it staged.
const STAGED_BYTES: &[u8; 20] = b"0123456789abcdefghij";

impl RecordingOutbox {
    fn staged(artifact: u32) -> nessa_protocol::lease::StagedArtifact {
        nessa_protocol::lease::StagedArtifact {
            name: "report.pdf".into(),
            media_type: "application/pdf".into(),
            size: 20,
            digest: "ab".repeat(32),
            path: format!("/outbox/{artifact}"),
        }
    }
    /// Hold every read, or let them go.
    fn hold_reads(&self, hold: bool) {
        let (held, released) = &self.reads_held;
        *held.lock().unwrap() = hold;
        released.notify_all();
    }
    fn point(&self, lease: &str) -> mpsc::Sender<crate::env_serve::application::PublishCall> {
        self.points.lock().unwrap()[lease].clone()
    }
}

impl crate::env_serve::application::ArtifactOutbox for RecordingOutbox {
    fn open(&self, lease: &str) -> std::io::Result<crate::env_serve::application::PublishPoint> {
        let (sender, calls) = mpsc::channel(16);
        self.points.lock().unwrap().insert(lease.into(), sender);
        Ok(crate::env_serve::application::PublishPoint {
            address: format!("/outbox/{lease}.sock"),
            calls,
        })
    }
    fn stage(
        &self,
        _lease: &str,
        artifact: u32,
        request: &crate::env_serve::application::PublishRequest,
    ) -> Result<nessa_protocol::lease::StagedArtifact, nessa_protocol::lease::PublishRefusal> {
        self.staging.lock().unwrap().push(request.clone());
        let stall = self.stall.lock().unwrap().take();
        if let Some((began, go)) = stall {
            began.send(()).unwrap();
            let _ = go.recv();
        }
        Ok(Self::staged(artifact))
    }
    fn read(
        &self,
        lease: &str,
        artifact: u32,
        offset: u64,
        length: usize,
    ) -> std::io::Result<Vec<u8>> {
        self.reads
            .lock()
            .unwrap()
            .push((lease.into(), artifact, offset, length));
        if let Some(begun) = self.read_begun.lock().unwrap().as_ref() {
            let _ = begun.send(());
        }
        let (held, released) = &self.reads_held;
        // Bounded, so a test that fails while reads are held still ends.
        drop(
            released
                .wait_timeout_while(held.lock().unwrap(), Duration::from_secs(10), |held| *held)
                .unwrap(),
        );
        if let Some(done) = self.read_done.lock().unwrap().as_ref() {
            let _ = done.send(());
        }
        if *self.unreadable.lock().unwrap() {
            return Err(std::io::Error::other("the copy is gone"));
        }
        let start = usize::try_from(offset).unwrap().min(STAGED_BYTES.len());
        let end = start.saturating_add(length).min(STAGED_BYTES.len());
        Ok(STAGED_BYTES[start..end].to_vec())
    }
    fn discard(&self, lease: &str, artifact: u32) {
        self.discarded
            .lock()
            .unwrap()
            .push((lease.into(), artifact));
    }
    fn close(&self, lease: &str) {
        self.closed.lock().unwrap().push(lease.into());
    }
}

/// Publish through `lease`'s point as a harness would, and return where its
/// answer arrives.
async fn publish(
    outbox: &RecordingOutbox,
    lease: &str,
) -> tokio::sync::oneshot::Receiver<crate::env_serve::application::PublishAnswer> {
    let (answer, answered) = tokio::sync::oneshot::channel();
    outbox
        .point(lease)
        .send(crate::env_serve::application::PublishCall {
            request: crate::env_serve::application::PublishRequest {
                path: "/work/report.pdf".into(),
                media_type: None,
            },
            answer,
        })
        .await
        .unwrap();
    answered
}

async fn answer_of(
    answered: tokio::sync::oneshot::Receiver<crate::env_serve::application::PublishAnswer>,
) -> crate::env_serve::application::PublishAnswer {
    tokio::time::timeout(Duration::from_secs(5), answered)
        .await
        .expect("an answer in time")
        .expect("the publisher is answered")
}

impl Gateway {
    fn publishing() -> (Self, Arc<RecordingOutbox>) {
        let outbox = Arc::new(RecordingOutbox::default());
        let gateway = Self::over(
            Arc::new(MemoryLedger::default()),
            outbox.clone(),
            true,
            real_time(),
        );
        (gateway, outbox)
    }
    /// A granted lease with one harness started under it.
    async fn started(&mut self, lease: &str) {
        assert_eq!(self.next().await, hello());
        self.granted(lease).await;
        self.send(ToEnvironment::Start {
            lease: lease.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
        self.settled().await;
    }
    /// The next frame that is not a harness's output ending.
    async fn said(&mut self) -> FromEnvironment {
        loop {
            match self.next().await {
                FromEnvironment::OutputClosed { .. } => continue,
                other => break other,
            }
        }
    }
    /// Every frame sent before this is handled once its account is read.
    async fn settled(&mut self) {
        self.send(ToEnvironment::Account {
            lease: "settled".into(),
        })
        .await;
        assert_eq!(
            self.said().await,
            FromEnvironment::Accounted {
                lease: "settled".into(),
                cleanup: Cleanup::NotHeld,
            }
        );
    }
}

/// A harness's publish reaches the gateway as where the staged copy is and
/// what it hashes to, recorded first; the gateway's answer is recorded, the
/// copy let go, and the harness told the conversation holds it.
#[tokio::test]
async fn a_published_file_is_offered_by_digest_and_answered_with_what_the_gateway_kept() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let answered = publish(&outbox, LEASE).await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Published {
            lease: LEASE.into(),
            artifact: 0,
            request: None,
            file: RecordingOutbox::staged(0),
        }
    );
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Held {
            digest: "ab".repeat(32),
            size: 20,
        }
    );
    assert_eq!(
        gateway.ledger.entries()[2..],
        [
            LedgerEntry::Published {
                lease: LEASE.into(),
                artifact: 0,
                request: None,
                digest: "ab".repeat(32),
                size: 20,
            },
            LedgerEntry::Collected {
                lease: LEASE.into(),
                artifact: 0,
                outcome: Collection::Held,
            },
        ]
    );
    assert_eq!(*outbox.discarded.lock().unwrap(), [(LEASE.into(), 0)]);
}

#[tokio::test]
async fn a_file_the_gateway_refuses_is_answered_with_its_reason() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    let refused = Collection::Refused {
        reason: nessa_protocol::lease::CollectionRefusal::Mismatch,
    };
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: refused,
        })
        .await;
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Refused {
            reason: nessa_protocol::lease::CollectionRefusal::Mismatch,
        }
    );
    assert_eq!(
        gateway.ledger.entries().last(),
        Some(&LedgerEntry::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: refused,
        })
    );
    assert_eq!(*outbox.discarded.lock().unwrap(), [(LEASE.into(), 0)]);
}

/// A lease that ends while a file is still being staged records no publish
/// and offers the gateway nothing: the harness is told the lease ended.
#[tokio::test]
async fn a_lease_ending_during_staging_records_no_publish() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let (began, staging) = std::sync::mpsc::channel();
    let (go, held) = std::sync::mpsc::channel();
    *outbox.stall.lock().unwrap() = Some((began, held));
    let answered = publish(&outbox, LEASE).await;
    tokio::task::spawn_blocking(move || staging.recv_timeout(Duration::from_secs(5)))
        .await
        .unwrap()
        .expect("staging began");
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ended { .. }
    ));
    go.send(()).unwrap();
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::NotPublished {
            reason: nessa_protocol::lease::PublishRefusal::LeaseEnded,
        }
    );
    assert!(!gateway
        .ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Published { .. })));
    assert_eq!(*outbox.discarded.lock().unwrap(), [(LEASE.into(), 0)]);
}

/// A gateway's answer this side cannot record is not reported as a success:
/// the harness is told what the gateway answered and that it went
/// unrecorded.
#[tokio::test]
async fn a_collection_that_cannot_be_recorded_is_not_reported_as_held() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    gateway.ledger.failing.store(true, Ordering::SeqCst);
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Unrecorded {
            collection: Collection::Held,
        }
    );
}

/// Past the in-flight bound a publish is refused busy on this side, and the
/// gateway is offered nothing more.
#[tokio::test]
async fn a_publish_past_the_in_flight_bound_is_refused_busy() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let mut waiting = Vec::new();
    for _ in 0..nessa_protocol::lease::MAX_ARTIFACTS_IN_FLIGHT {
        waiting.push(publish(&outbox, LEASE).await);
    }
    let mut offered = Vec::new();
    while offered.len() < nessa_protocol::lease::MAX_ARTIFACTS_IN_FLIGHT {
        match gateway.said().await {
            FromEnvironment::Published { artifact, .. } => offered.push(artifact),
            other => panic!("unexpected {other:?}"),
        }
    }
    offered.sort_unstable();
    assert_eq!(offered, [0, 1, 2, 3]);
    let extra = publish(&outbox, LEASE).await;
    assert_eq!(
        answer_of(extra).await,
        crate::env_serve::application::PublishAnswer::NotPublished {
            reason: nessa_protocol::lease::PublishRefusal::Busy,
        }
    );
    let published = gateway
        .ledger
        .entries()
        .iter()
        .filter(|entry| matches!(entry, LedgerEntry::Published { .. }))
        .count();
    assert_eq!(published, nessa_protocol::lease::MAX_ARTIFACTS_IN_FLIGHT);
    for waiting in &mut waiting {
        assert!(waiting.try_recv().is_err(), "still waiting for the gateway");
    }
    // An answer frees its place.
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    answer_of(waiting.remove(0)).await;
    let _again = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 4, .. }
    ));
}

fn publish_frame(lease: &str, request: &str, path: &str) -> ToEnvironment {
    ToEnvironment::Publish {
        lease: lease.into(),
        request: request.into(),
        path: path.into(),
        media_type: None,
    }
}

/// Slice D3a: a publish the gateway asks for is staged through the same
/// outbox, under the same in-flight bound and ledger, as a harness's; its
/// path is the workspace's when relative; its `Published` names the
/// request, and past the bound it is answered `NotPublished` busy by name.
#[tokio::test]
async fn a_publish_frame_stages_as_the_publish_point_does_and_is_refused_busy_past_the_bound() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    gateway
        .send(publish_frame(LEASE, "call-1", "out/report.pdf"))
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Published {
            lease: LEASE.into(),
            artifact: 0,
            request: Some("call-1".into()),
            file: RecordingOutbox::staged(0),
        }
    );
    assert_eq!(
        outbox.staging.lock().unwrap()[0].path,
        "/work/out/report.pdf"
    );
    // Asked, then published naming the ask, in the ledger's order.
    assert_eq!(
        gateway.ledger.entries()[2..],
        [
            LedgerEntry::PublishAsked {
                lease: LEASE.into(),
                request: "call-1".into(),
            },
            LedgerEntry::Published {
                lease: LEASE.into(),
                artifact: 0,
                request: Some("call-1".into()),
                digest: "ab".repeat(32),
                size: 20,
            },
        ]
    );
    // A harness's publish and the gateway's share one bound.
    let harness = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { request: None, .. }
    ));
    for request in ["call-2", "call-3"] {
        gateway
            .send(publish_frame(LEASE, request, "/work/a.png"))
            .await;
        assert!(matches!(
            gateway.said().await,
            FromEnvironment::Published { request: Some(asked), .. } if asked == request
        ));
    }
    gateway.send(publish_frame(LEASE, "call-4", "a.png")).await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::NotPublished {
            lease: LEASE.into(),
            request: "call-4".into(),
            reason: nessa_protocol::lease::PublishRefusal::Busy,
        }
    );
    // Its collection settles it as any other's, and frees its place.
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    gateway.settled().await;
    assert!(gateway.ledger.entries().contains(&LedgerEntry::Collected {
        lease: LEASE.into(),
        artifact: 0,
        outcome: Collection::Held,
    }));
    assert!(outbox
        .discarded
        .lock()
        .unwrap()
        .contains(&(LEASE.into(), 0)));
    gateway.send(publish_frame(LEASE, "call-5", "a.png")).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 4, request: Some(asked), .. } if asked == "call-5"
    ));
    drop(harness);
}

/// A publish asked under a lease this connection does not hold, or one
/// whose outbox could not be opened, is answered by name and offers
/// nothing.
#[tokio::test]
async fn a_publish_frame_for_no_lease_held_or_no_outbox_is_answered_not_published() {
    let (mut gateway, _outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    gateway
        .send(publish_frame(
            "0b7d3a1e-0000-4000-8000-0000000000ff",
            "call-1",
            "a.png",
        ))
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::NotPublished {
            lease: "0b7d3a1e-0000-4000-8000-0000000000ff".into(),
            request: "call-1".into(),
            reason: nessa_protocol::lease::PublishRefusal::LeaseEnded,
        }
    );
    let mut bare = Gateway::start();
    assert_eq!(bare.next().await, hello());
    bare.granted(LEASE).await;
    bare.send(publish_frame(LEASE, "call-2", "a.png")).await;
    assert_eq!(
        bare.next().await,
        FromEnvironment::NotPublished {
            lease: LEASE.into(),
            request: "call-2".into(),
            reason: nessa_protocol::lease::PublishRefusal::StagingFailed,
        }
    );
}

/// The lease ending while a gateway's publish is in flight settles it as
/// it does a harness's: answered lease ended, and its staged copy let go.
#[tokio::test]
async fn a_gateway_asked_publish_in_flight_is_settled_by_its_lease_end() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    gateway.send(publish_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ended { .. }
    ));
    let entries = gateway.ledger.entries();
    let collected = entries
        .iter()
        .position(|entry| {
            *entry
                == LedgerEntry::Collected {
                    lease: LEASE.into(),
                    artifact: 0,
                    outcome: Collection::Refused {
                        reason: nessa_protocol::lease::CollectionRefusal::LeaseEnded,
                    },
                }
        })
        .expect("settled as ended");
    let ended = entries
        .iter()
        .position(|entry| matches!(entry, LedgerEntry::Ended { .. }))
        .expect("ended");
    assert!(collected < ended);
    tokio::time::timeout(Duration::from_secs(5), async {
        while !outbox
            .discarded
            .lock()
            .unwrap()
            .contains(&(LEASE.into(), 0))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("its staged copy is let go");
}

/// The lease ending answers every publish the gateway never answered, and
/// lets go of everything the lease staged.
#[tokio::test]
async fn a_lease_end_answers_every_waiting_publisher() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let first = publish(&outbox, LEASE).await;
    let second = publish(&outbox, LEASE).await;
    for _ in 0..2 {
        assert!(matches!(
            gateway.said().await,
            FromEnvironment::Published { .. }
        ));
    }
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ended { .. }
    ));
    let ended = crate::env_serve::application::PublishAnswer::Refused {
        reason: nessa_protocol::lease::CollectionRefusal::LeaseEnded,
    };
    assert_eq!(answer_of(first).await, ended);
    assert_eq!(answer_of(second).await, ended);
    assert_eq!(*outbox.closed.lock().unwrap(), [LEASE.to_owned()]);
    let collected: Vec<_> = gateway
        .ledger
        .entries()
        .into_iter()
        .filter_map(|entry| match entry {
            LedgerEntry::Collected { outcome, .. } => Some(outcome),
            _ => None,
        })
        .collect();
    assert_eq!(
        collected,
        [Collection::Refused {
            reason: nessa_protocol::lease::CollectionRefusal::LeaseEnded,
        }; 2]
    );
    // Nothing more is published under it.
    assert!(outbox
        .point(LEASE)
        .send(crate::env_serve::application::PublishCall {
            request: crate::env_serve::application::PublishRequest {
                path: "/work/late.pdf".into(),
                media_type: None,
            },
            answer: tokio::sync::oneshot::channel().0,
        })
        .await
        .is_err());
}

/// A collection the gateway sends just before its lease's End is on record
/// before that end, however long recording it takes: what the gateway
/// queued first, the ledger keeps first. On two threads, so the end could
/// run while a collection is still being recorded elsewhere.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_collection_just_before_the_end_is_recorded_before_it() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    gateway
        .ledger
        .slow_collections
        .store(true, Ordering::SeqCst);
    let answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ended { .. }
    ));
    let entries = gateway.ledger.entries();
    let at = |kind: fn(&LedgerEntry) -> bool| entries.iter().position(kind).unwrap();
    assert!(
        at(|entry| matches!(entry, LedgerEntry::Collected { .. }))
            < at(|entry| matches!(entry, LedgerEntry::Ended { .. })),
        "{entries:?}"
    );
    assert!(matches!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Held { .. }
    ));
}

/// A lease that ends while a publish is being recorded settles it only
/// once it is on record: the ledger never holds a publish's collection
/// before the publish. On two threads, so the end runs meanwhile.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_lease_ending_while_a_publish_is_recorded_settles_it_after() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    gateway.ledger.slow_publishes.store(true, Ordering::SeqCst);
    let publishing = gateway.ledger.publishing.notified();
    let answered = publish(&outbox, LEASE).await;
    tokio::time::timeout(Duration::from_secs(5), publishing)
        .await
        .expect("the publish is being recorded");
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    let answer = answer_of(answered).await;
    let entries = gateway.ledger.entries();
    let published = entries
        .iter()
        .position(|entry| matches!(entry, LedgerEntry::Published { .. }))
        .expect("the publish is on record");
    let collected = entries
        .iter()
        .position(|entry| matches!(entry, LedgerEntry::Collected { .. }))
        .expect("and so is its settlement");
    assert!(published < collected, "{entries:?}");
    assert_eq!(
        answer,
        crate::env_serve::application::PublishAnswer::Refused {
            reason: nessa_protocol::lease::CollectionRefusal::LeaseEnded,
        }
    );
}

/// A publish the gateway never answered before its connection was lost is
/// answered unanswered, not as ended: the gateway may have kept it.
#[tokio::test]
async fn a_publish_cut_off_by_a_lost_connection_is_answered_unanswered() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    let Gateway {
        writer,
        frames,
        served,
        ledger,
        ..
    } = gateway;
    drop(writer);
    drop(frames);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("serving ends with its stream")
        .unwrap();
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Unanswered
    );
    let entries = ledger.entries();
    assert!(entries.contains(&LedgerEntry::Unanswered {
        lease: LEASE.into(),
        artifact: 0,
    }));
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::Collected { .. })),
        "{entries:?}"
    );
}

/// A harness is told its lease's publish point; a host that could not open
/// one tells it of none.
#[tokio::test]
async fn a_harness_is_given_its_lease_publish_point() {
    let (mut gateway, _outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    assert_eq!(
        *gateway.launcher.points.lock().unwrap(),
        [Some(format!("/outbox/{LEASE}.sock"))]
    );
    let mut without = Gateway::start();
    without.started(LEASE).await;
    assert_eq!(*without.launcher.points.lock().unwrap(), [None]);
}

/// A collection naming an artifact or a lease not waiting for one is
/// dropped with evidence and answers nobody.
#[tokio::test]
async fn a_collection_naming_nothing_published_is_dropped() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let mut answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    for (lease, artifact) in [(LEASE, 9), ("other", 0)] {
        gateway
            .send(ToEnvironment::Collected {
                lease: lease.into(),
                artifact,
                outcome: Collection::Held,
            })
            .await;
    }
    gateway.settled().await;
    assert!(
        answered.try_recv().is_err(),
        "still waiting for the gateway"
    );
    assert!(outbox.discarded.lock().unwrap().is_empty());
    let entries = gateway.ledger.entries();
    let drops: Vec<_> = entries
        .iter()
        .filter_map(|entry| match entry {
            LedgerEntry::Dropped { lease, frame, .. } => Some((lease.clone(), frame.clone())),
            _ => None,
        })
        .collect();
    assert_eq!(
        drops,
        [
            (Some(LEASE.into()), "collected".into()),
            (Some("other".into()), "collected".into()),
        ]
    );
    assert!(!entries
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Collected { .. })));
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::AlreadyHeld,
        })
        .await;
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::AlreadyHeld {
            digest: "ab".repeat(32),
            size: 20,
        }
    );
}

fn read_artifact(lease: &str, artifact: u32, offset: u64, length: u32) -> ToEnvironment {
    ToEnvironment::ReadArtifact {
        lease: lease.into(),
        artifact,
        offset,
        length,
    }
}

fn artifact_read(offset: u64, data: &[u8]) -> FromEnvironment {
    FromEnvironment::ArtifactRead {
        lease: LEASE.into(),
        artifact: 0,
        offset,
        data: Data(data.to_vec()),
    }
}

fn not_read(
    lease: &str,
    artifact: u32,
    offset: u64,
    reason: nessa_protocol::lease::ArtifactReadRefusal,
) -> FromEnvironment {
    FromEnvironment::ArtifactNotRead {
        lease: lease.into(),
        artifact,
        offset,
        reason,
    }
}

/// Row I9, the environment's side: a gateway reads a staged copy over the
/// lease stream by artifact and range, never by path, and only while that
/// artifact waits for its answer. A range past the copy's end comes back
/// short, or empty; a longer one than a frame carries is read as one that
/// does. One never published, of a lease not held (a drop, recorded), or
/// already answered is not staged.
#[tokio::test]
async fn a_staged_copy_is_read_by_range_only_while_it_waits_for_its_answer() {
    use nessa_protocol::lease::{ArtifactReadRefusal, MAX_DATA_BYTES};
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    for (frame, said) in [
        (read_artifact(LEASE, 0, 0, 8), artifact_read(0, b"01234567")),
        (read_artifact(LEASE, 0, 16, 100), artifact_read(16, b"ghij")),
        (read_artifact(LEASE, 0, 20, 8), artifact_read(20, b"")),
        (
            read_artifact(LEASE, 0, 0, u32::MAX),
            artifact_read(0, STAGED_BYTES),
        ),
        (
            read_artifact(LEASE, 9, 0, 8),
            not_read(LEASE, 9, 0, ArtifactReadRefusal::NotStaged),
        ),
        (
            read_artifact("other", 0, 0, 8),
            not_read("other", 0, 0, ArtifactReadRefusal::NotStaged),
        ),
    ] {
        gateway.send(frame).await;
        assert_eq!(gateway.said().await, said);
    }
    assert_eq!(
        *outbox.reads.lock().unwrap(),
        [
            (LEASE.to_owned(), 0, 0, 8),
            (LEASE.to_owned(), 0, 16, 100),
            (LEASE.to_owned(), 0, 20, 8),
            (LEASE.to_owned(), 0, 0, MAX_DATA_BYTES),
        ]
    );
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    assert!(matches!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Held { .. }
    ));
    gateway.send(read_artifact(LEASE, 0, 0, 8)).await;
    assert_eq!(
        gateway.said().await,
        not_read(LEASE, 0, 0, ArtifactReadRefusal::NotStaged)
    );
    assert_eq!(outbox.reads.lock().unwrap().len(), 4);
    let drops: Vec<_> = gateway
        .ledger
        .entries()
        .into_iter()
        .filter_map(|entry| match entry {
            LedgerEntry::Dropped { lease, frame, .. } => Some((lease, frame)),
            _ => None,
        })
        .collect();
    assert_eq!(drops, [(Some("other".into()), "read_artifact".into())]);
}

/// A gateway past the bound of reads of one artifact unanswered is refused
/// `busy` for the one past it, and the others are answered; a copy that
/// cannot be read is said to be unreadable.
#[tokio::test]
async fn reads_past_the_bound_are_refused_busy_and_an_unreadable_copy_is_said() {
    use nessa_protocol::lease::{ArtifactReadRefusal, MAX_ARTIFACT_READS_IN_FLIGHT};
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let _answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    let held = ReadsHeld::new(&outbox);
    let bound = MAX_ARTIFACT_READS_IN_FLIGHT as u64;
    for offset in 0..=bound {
        gateway.send(read_artifact(LEASE, 0, offset, 1)).await;
    }
    assert_eq!(
        gateway.said().await,
        not_read(LEASE, 0, bound, ArtifactReadRefusal::Busy)
    );
    drop(held);
    let mut answered = Vec::new();
    for _ in 0..bound {
        match gateway.said().await {
            FromEnvironment::ArtifactRead { offset, data, .. } => answered.push((offset, data.0)),
            other => panic!("a read's bytes, got {other:?}"),
        }
    }
    answered.sort();
    let expected: Vec<_> = (0..bound)
        .map(|offset| (offset, vec![STAGED_BYTES[offset as usize]]))
        .collect();
    assert_eq!(answered, expected);
    // Answered, they leave room: the next is read.
    *outbox.unreadable.lock().unwrap() = true;
    gateway.send(read_artifact(LEASE, 0, 0, 1)).await;
    assert_eq!(
        gateway.said().await,
        not_read(LEASE, 0, 0, ArtifactReadRefusal::Unreadable)
    );
}

/// Holds every read of `outbox` until dropped, so a test that fails while
/// reads are held leaves none waiting.
struct ReadsHeld<'a>(&'a RecordingOutbox);

impl<'a> ReadsHeld<'a> {
    fn new(outbox: &'a RecordingOutbox) -> Self {
        outbox.hold_reads(true);
        Self(outbox)
    }
}

impl Drop for ReadsHeld<'_> {
    fn drop(&mut self) {
        self.0.hold_reads(false);
    }
}

impl RecordingOutbox {
    /// Where each read says it has begun, from now on.
    fn reads_begun(&self) -> tokio::sync::mpsc::UnboundedReceiver<()> {
        let (begun, receiver) = tokio::sync::mpsc::unbounded_channel();
        *self.read_begun.lock().unwrap() = Some(begun);
        receiver
    }
    /// Where each read says it has returned, from now on.
    fn reads_done(&self) -> tokio::sync::mpsc::UnboundedReceiver<()> {
        let (done, receiver) = tokio::sync::mpsc::unbounded_channel();
        *self.read_done.lock().unwrap() = Some(done);
        receiver
    }
}

/// Wait until a read on the outbox has said `what`: begun, or returned.
async fn read_said(said: &mut tokio::sync::mpsc::UnboundedReceiver<()>, what: &str) {
    tokio::time::timeout(Duration::from_secs(5), said.recv())
        .await
        .unwrap_or_else(|_| panic!("a read {what} in time"))
        .expect("the outbox is still there");
}

/// A copy let go while it was being read, the gateway having answered it
/// already, is answered not staged, whatever the read made of it: the
/// bytes of a copy no longer staged are not sent, and a copy gone is not
/// damaged.
#[tokio::test]
async fn a_copy_let_go_while_it_is_read_is_answered_not_staged() {
    copy_let_go_while_read(false).await;
}

/// [`a_copy_let_go_while_it_is_read_is_answered_not_staged`], for a read
/// that failed: still not staged, not unreadable.
#[tokio::test]
async fn a_copy_let_go_while_it_is_read_and_failing_is_answered_not_staged() {
    copy_let_go_while_read(true).await;
}

async fn copy_let_go_while_read(unreadable: bool) {
    use nessa_protocol::lease::ArtifactReadRefusal;
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let _answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    let mut begun = outbox.reads_begun();
    let held = ReadsHeld::new(&outbox);
    gateway.send(read_artifact(LEASE, 0, 0, 8)).await;
    read_said(&mut begun, "began").await;
    gateway
        .send(ToEnvironment::Collected {
            lease: LEASE.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    gateway.settled().await;
    *outbox.unreadable.lock().unwrap() = unreadable;
    drop(held);
    assert_eq!(
        gateway.said().await,
        not_read(LEASE, 0, 0, ArtifactReadRefusal::NotStaged)
    );
}

/// A lease ended while reads of its files are under way stops them: no
/// answer of theirs follows its `Ended`, so the gateway, which lets the
/// lease go at `Ended`, never gets one for a lease it no longer holds. A
/// read already on the disk cannot be stopped there, so `Ended` waits for
/// it to return: no read holds a file of a lease said ended.
///
/// On the runtime's paused clock, which moves only once every task waits
/// and no read is on the disk: what the reads would do once returned is
/// done before the account is asked.
#[tokio::test(start_paused = true)]
async fn no_read_answers_after_its_lease_ended() {
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let _answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    let mut begun = outbox.reads_begun();
    let mut done = outbox.reads_done();
    let held = ReadsHeld::new(&outbox);
    gateway.send(read_artifact(LEASE, 0, 0, 8)).await;
    gateway.send(read_artifact(LEASE, 0, 8, 8)).await;
    read_said(&mut begun, "began").await;
    read_said(&mut begun, "began").await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    // The reads are let go from outside the runtime a little later, so an
    // `Ended` that did not wait for them would be said while they are held.
    let releasing = outbox.clone();
    let release = std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(200));
        releasing.hold_reads(false);
    });
    loop {
        match gateway.said().await {
            FromEnvironment::Stopped { .. } => continue,
            FromEnvironment::Ended { lease, .. } => {
                assert_eq!(lease, LEASE);
                break;
            }
            other => panic!("the lease's end, got {other:?}"),
        }
    }
    // Both reads had returned from the disk before it was said.
    assert!(done.try_recv().is_ok() && done.try_recv().is_ok());
    release.join().unwrap();
    drop(held);
    // Nothing of theirs is said: an answer would be on its way ahead of the
    // account, asked once everything stands still.
    tokio::time::sleep(Duration::from_secs(1)).await;
    gateway.settled().await;
}

/// A gateway that keeps asking for ranges but reads nothing holds a bounded
/// number of answers on the environment: each read keeps its place under
/// the bound until its answer reaches the writer, so once the writer's
/// queue and the connection are full, the next read past the bound is
/// refused `busy`, which the environment waits to say, and it reads no
/// more frames meanwhile. Nothing more is read off the disk.
///
/// On the runtime's paused clock, which moves only once every task waits
/// and no read is on the disk: a wait that runs out is the environment
/// standing still, never a slow machine.
#[tokio::test(start_paused = true)]
async fn a_gateway_that_reads_nothing_holds_a_bounded_number_of_answers() {
    use nessa_protocol::lease::{ArtifactReadRefusal, MAX_ARTIFACT_READS_IN_FLIGHT};
    let outbox = Arc::new(RecordingOutbox::default());
    let mut gateway = Gateway::through(
        4096,
        Peer::Ssh,
        MemoryGrants::none(),
        Arc::new(MemoryLedger::default()),
        outbox.clone(),
        None,
        true,
        real_time(),
    );
    gateway.started(LEASE).await;
    let _answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    // Asked one at a time without reading anything, each once the one
    // before it is read off the disk, until the environment stands still
    // with one not read: refused, or not taken at all.
    let asked: u64 = 2_000;
    let mut begun = outbox.reads_begun();
    let mut sent = 0;
    for offset in 0..asked {
        let send = gateway.send(read_artifact(LEASE, 0, offset % 20, 1));
        if tokio::time::timeout(Duration::from_secs(1), send)
            .await
            .is_err()
        {
            break;
        }
        sent += 1;
        if tokio::time::timeout(Duration::from_secs(1), begun.recv())
            .await
            .is_err()
        {
            break;
        }
    }
    assert!(sent < asked, "the environment read all {asked} asks");
    // What was read off the disk is bounded by what the writer's queue and
    // the connection hold, and the reads holding their places.
    let read = outbox.reads.lock().unwrap().len();
    // No answer frame is under 64 bytes, and the writer holds one more.
    let bound = OUTPUT_QUEUE + 4096 / 64 + MAX_ARTIFACT_READS_IN_FLIGHT + 1;
    assert!(read <= bound, "{read} reads off the disk, past {bound}");
    // Read again, the gateway hears the refusal among the answers.
    let busy = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            if let FromEnvironment::ArtifactNotRead { reason, .. } = gateway.said().await {
                break reason;
            }
        }
    })
    .await
    .expect("the refusal in time");
    assert_eq!(busy, ArtifactReadRefusal::Busy);
}

/// Once the gateway says `Revoked`, the lease ends and its copies go with
/// it: a read of a file it had offered is answered not staged.
#[tokio::test]
async fn a_read_after_the_gateway_revoked_is_answered_not_staged() {
    use nessa_protocol::lease::ArtifactReadRefusal;
    let (mut gateway, outbox) = Gateway::publishing();
    gateway.started(LEASE).await;
    let answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    gateway.send(ToEnvironment::Revoked).await;
    gateway.send(read_artifact(LEASE, 0, 0, 8)).await;
    let refused = loop {
        match gateway.said().await {
            FromEnvironment::Stopped { .. } => continue,
            other => break other,
        }
    };
    assert_eq!(
        refused,
        not_read(LEASE, 0, 0, ArtifactReadRefusal::NotStaged)
    );
    assert!(outbox.reads.lock().unwrap().is_empty());
    // Ended unasked by the gateway: its publisher is told it is not known.
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Unanswered
    );
}

const COMMAND: &str = "0b7d3a1e-0000-4000-8000-0000000000c1";

fn grant_command(lease: &str, argv: &[&str], cwd: Option<&str>) -> ToEnvironment {
    ToEnvironment::GrantCommand {
        lease: lease.into(),
        argv: argv.iter().map(|a| (*a).to_owned()).collect(),
        cwd: cwd.map(Into::into),
        timeout_ms: 1_000,
    }
}

/// Row L14 on the host: a command lease is granted, its command run once,
/// and its `Ran` carries how it ended, what it printed and what releasing it
/// took — each recorded before it is answered.
#[tokio::test]
async fn a_command_lease_runs_its_command_once_and_its_ran_ends_it() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(grant_command(COMMAND, &["echo", "a", "b"], Some("sub")))
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Granted {
            lease: COMMAND.into(),
            terms: command_terms(1_000),
        }
    );
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Ran {
            lease: COMMAND.into(),
            end: CommandEnd::Exited { code: 0 },
            stdout: Data(b"a b".to_vec()),
            stderr: Data(b"e".to_vec()),
            dropped_bytes: 3,
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    let ran = gateway.commands.ran.lock().unwrap().clone();
    assert_eq!(ran.len(), 1);
    assert_eq!(ran[0].cwd(), Some("sub"));
    // Run once: a second run is dropped and nothing more runs.
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    gateway
        .send(ToEnvironment::Account {
            lease: COMMAND.into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Ran {
            lease: COMMAND.into(),
            end: CommandEnd::NotStarted,
            stdout: Data(Vec::new()),
            stderr: Data(Vec::new()),
            dropped_bytes: 0,
            cleanup: Cleanup::NotHeld,
        }
    );
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Accounted {
            lease: COMMAND.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(gateway.commands.ran.lock().unwrap().len(), 1);
    // No outbox on this host: the command ran without a publish point.
    assert_eq!(gateway.commands.points.lock().unwrap().as_slice(), &[None]);
    let entries = gateway.ledger.entries();
    assert_eq!(
        entries[..4],
        [
            LedgerEntry::CommandGranted {
                lease: COMMAND.into(),
                peer: Peer::Ssh,
                argv: vec!["echo".into(), "a".into(), "b".into()],
                cwd: Some("sub".into()),
                timeout_ms: 1_000,
                asked_timeout_ms: 1_000,
                sandbox: Sandbox::None,
            },
            LedgerEntry::CommandStarted {
                lease: COMMAND.into()
            },
            LedgerEntry::CommandRan {
                lease: COMMAND.into(),
                end: CommandEnd::Exited { code: 0 },
                dropped_bytes: 3,
            },
            LedgerEntry::Ended {
                lease: COMMAND.into(),
                cleanup: Cleanup::Confirmed { forced: false },
                cause: EndCause::Asked,
            },
        ]
    );
}

/// Row C12: a command lease publishes on its own point while it runs, and
/// the point stays open until the gateway ends the lease.
#[tokio::test]
async fn a_command_lease_publishes_on_its_own_point_until_it_ends() {
    let (mut gateway, outbox) = Gateway::publishing();
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(COMMAND, &["hold"], None)).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Granted { .. }
    ));
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    let point = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if let Some(point) = gateway.commands.points.lock().unwrap().last().cloned() {
                return point;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .expect("the command was started");
    let expected = format!("/outbox/{COMMAND}.sock");
    assert_eq!(point.as_deref(), Some(expected.as_str()));
    let answered = publish(&outbox, COMMAND).await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Published {
            lease: COMMAND.into(),
            artifact: 0,
            request: None,
            file: RecordingOutbox::staged(0),
        }
    );
    gateway
        .send(ToEnvironment::Collected {
            lease: COMMAND.into(),
            artifact: 0,
            outcome: Collection::Held,
        })
        .await;
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Held {
            digest: "ab".repeat(32),
            size: 20,
        }
    );
    gateway
        .send(ToEnvironment::End {
            lease: COMMAND.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ran {
            end: CommandEnd::Stopped,
            ..
        }
    ));
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ended { .. }
    ));
    assert_eq!(outbox.closed.lock().unwrap().as_slice(), &[COMMAND]);
}

/// Row C13: a file still unanswered when the gateway ends the command is
/// answered `leaseEnded`, and that is recorded before the lease's end.
#[tokio::test]
async fn a_command_lease_ends_a_file_still_unanswered() {
    let (mut gateway, outbox) = Gateway::publishing();
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(COMMAND, &["hold"], None)).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Granted { .. }
    ));
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    let _ = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if gateway.commands.points.lock().unwrap().last().is_some() {
                return;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    let answered = publish(&outbox, COMMAND).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    gateway
        .send(ToEnvironment::End {
            lease: COMMAND.into(),
        })
        .await;
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Refused {
            reason: nessa_protocol::lease::CollectionRefusal::LeaseEnded,
        }
    );
    assert!(matches!(gateway.next().await, FromEnvironment::Ran { .. }));
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ended { .. }
    ));
    assert!(gateway.ledger.entries().iter().any(|entry| matches!(
        entry,
        LedgerEntry::Collected {
            lease,
            outcome: Collection::Refused {
                reason: nessa_protocol::lease::CollectionRefusal::LeaseEnded,
            },
            ..
        } if lease == COMMAND
    )));
}

/// Row C18: granting another command does not close a finished command's
/// publish point. That point stays until the finished command's own End.
#[tokio::test]
async fn a_later_grant_leaves_a_finished_commands_publish_point_open() {
    let (mut gateway, outbox) = Gateway::publishing();
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(grant_command(COMMAND, &["echo", "a"], None))
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Granted { .. }
    ));
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    assert!(matches!(gateway.next().await, FromEnvironment::Ran { .. }));
    let other = "0b7d3a1e-0000-4000-8000-0000000000c2";
    gateway
        .send(grant_command(other, &["echo", "b"], None))
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Granted { lease, .. } if lease == other
    ));
    assert!(
        outbox.closed.lock().unwrap().is_empty(),
        "the finished command's point was closed by the later grant"
    );
    gateway
        .send(ToEnvironment::End {
            lease: COMMAND.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ended { lease, .. } if lease == COMMAND
    ));
    assert_eq!(outbox.closed.lock().unwrap().as_slice(), &[COMMAND]);
}

/// Each command lease refusal is typed and recorded: a host not configured
/// for commands, a command no lease can hold, an id already used, and an
/// audit that cannot record it.
#[tokio::test]
async fn a_command_lease_is_refused_with_its_reason() {
    let mut gateway = Gateway::serving(Arc::new(MemoryLedger::default()), false);
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(COMMAND, &["echo"], None)).await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: COMMAND.into(),
            reason: GrantRefusal::CommandsUnavailable,
        }
    );

    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    for (argv, cwd) in [
        (&["echo"][..], Some("../out")),
        (&[""][..], None),
        (&["echo", "\u{1b}"][..], None),
    ] {
        gateway.send(grant_command(COMMAND, argv, cwd)).await;
        assert_eq!(
            gateway.next().await,
            FromEnvironment::Refused {
                lease: COMMAND.into(),
                reason: GrantRefusal::InvalidCommand,
            }
        );
    }
    gateway.granted(LEASE).await;
    gateway.send(grant_command(LEASE, &["echo"], None)).await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: LEASE.into(),
            reason: GrantRefusal::Duplicate,
        }
    );
    gateway.ledger.failing.store(true, Ordering::SeqCst);
    gateway.send(grant_command(COMMAND, &["echo"], None)).await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Refused {
            lease: COMMAND.into(),
            reason: GrantRefusal::AuditUnavailable,
        }
    );
    gateway.ledger.failing.store(false, Ordering::SeqCst);
    assert!(gateway.commands.ran.lock().unwrap().is_empty());
    assert!(gateway.ledger.entries().contains(&LedgerEntry::Refused {
        lease: COMMAND.into(),
        peer: Peer::Ssh,
        reason: GrantRefusal::InvalidCommand,
        asked: Asked::Command {
            argv: vec!["echo".into()],
            cwd: Some("../out".into()),
            asked_timeout_ms: 1_000,
        },
    }));
}

/// Row L15 on the host: the gateway's End stops a running command, which
/// answers its `Ran` as stopped with what it printed, and then the End.
#[tokio::test]
async fn an_end_stops_a_running_command_and_is_answered_after_its_ran() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(COMMAND, &["wait"], None)).await;
    gateway.next().await;
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    gateway
        .send(ToEnvironment::End {
            lease: COMMAND.into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Ran {
            lease: COMMAND.into(),
            end: CommandEnd::Stopped,
            stdout: Data(b"partial".to_vec()),
            stderr: Data(Vec::new()),
            dropped_bytes: 0,
            cleanup: Cleanup::Confirmed { forced: true },
        }
    );
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Ended {
            lease: COMMAND.into(),
            cleanup: Cleanup::Confirmed { forced: true },
        }
    );

    // Ended before it ran: nothing was held, and nothing runs after.
    const OTHER: &str = "0b7d3a1e-0000-4000-8000-0000000000c2";
    gateway.send(grant_command(OTHER, &["echo"], None)).await;
    gateway.next().await;
    gateway
        .send(ToEnvironment::End {
            lease: OTHER.into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Ended {
            lease: OTHER.into(),
            cleanup: Cleanup::NotHeld,
        }
    );
    gateway
        .send(ToEnvironment::Run {
            lease: OTHER.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ran {
            end: CommandEnd::NotStarted,
            ..
        }
    ));
    assert_eq!(gateway.commands.ran.lock().unwrap().len(), 1);
}

/// The gateway gone: a running command is stopped as lost, its end recorded
/// before serving returns, and a granted one never run ends holding nothing.
#[tokio::test]
async fn a_lost_connection_stops_running_commands_and_records_them_lost() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    const OTHER: &str = "0b7d3a1e-0000-4000-8000-0000000000c2";
    gateway.send(grant_command(COMMAND, &["wait"], None)).await;
    gateway.next().await;
    gateway.send(grant_command(OTHER, &["wait"], None)).await;
    gateway.next().await;
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    // The run is under way before the connection goes.
    gateway.send(ToEnvironment::Keepalive).await;
    tokio::time::sleep(Duration::from_millis(50)).await;
    let Gateway {
        writer,
        frames,
        served,
        ledger,
        ..
    } = gateway;
    drop(writer);
    drop(frames);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("serving ends")
        .unwrap();
    let entries = ledger.entries();
    assert!(entries.contains(&LedgerEntry::Ended {
        lease: COMMAND.into(),
        cleanup: Cleanup::Confirmed { forced: true },
        cause: EndCause::Lost,
    }));
    assert!(entries.contains(&LedgerEntry::Ended {
        lease: OTHER.into(),
        cleanup: Cleanup::NotHeld,
        cause: EndCause::Lost,
    }));
}

#[tokio::test]
async fn a_command_whose_exit_cannot_be_recorded_ends_uncertain_in_the_ledger_too() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway
        .send(grant_command(COMMAND, &["echo", "a", "b"], None))
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Granted {
            lease: COMMAND.into(),
            terms: command_terms(1_000),
        }
    );
    gateway
        .ledger
        .failing_command_ran
        .store(true, Ordering::SeqCst);
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    let FromEnvironment::Ran { cleanup, .. } = gateway.next().await else {
        panic!("its ran is answered");
    };
    assert_eq!(cleanup, Cleanup::Uncertain);
    // The end written after the missing exit says so, and nothing reads
    // confirmed later.
    assert!(gateway.ledger.entries().iter().any(|entry| matches!(
        entry,
        LedgerEntry::Ended { lease, cleanup: Cleanup::Uncertain, .. } if lease == COMMAND
    )));
    assert_eq!(
        gateway.ledger.accounted(COMMAND).unwrap(),
        Cleanup::Uncertain
    );
}

/// An inbox that says what a test sets of each destination, makes parts by
/// name only, and remembers what it was asked to place and let go.
#[derive(Default)]
struct RecordingInbox {
    /// What is at a destination, when not nothing.
    at: Mutex<HashMap<String, Result<Destination, FetchRefusal>>>,
    /// What placing answers, when not placed.
    placing: Mutex<Option<Placement>>,
    /// What was taken back, as placed.
    taken_back: Mutex<Vec<PlacedFile>>,
    parts: Mutex<Vec<(String, u32)>>,
    placed: Mutex<Vec<(String, u32, String, FetchedFile)>>,
    discarded: Mutex<Vec<(String, u32)>>,
    closed: Mutex<Vec<String>>,
    /// When set, making the next part waits for this.
    hold_part: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
    /// When set, making a part is refused so.
    refuse_part: Mutex<Option<FetchRefusal>>,
    /// When set, a placed file cannot be taken back.
    keeps: AtomicBool,
}

impl ArtifactInbox for RecordingInbox {
    fn destination(
        &self,
        destination: &str,
        _file: &FetchedFile,
    ) -> Result<Destination, FetchRefusal> {
        self.at
            .lock()
            .unwrap()
            .get(destination)
            .cloned()
            .unwrap_or(Ok(Destination::Free {
                path: format!("/work/{destination}"),
            }))
    }
    fn open_part(&self, lease: &str, fetch: u32) -> Result<String, FetchRefusal> {
        let held = self.hold_part.lock().unwrap().take();
        if let Some(go) = held {
            let _ = go.recv();
        }
        if let Some(reason) = *self.refuse_part.lock().unwrap() {
            return Err(reason);
        }
        self.parts.lock().unwrap().push((lease.into(), fetch));
        Ok(format!("/inbox/{fetch}.part"))
    }
    fn place(&self, lease: &str, fetch: u32, destination: &str, file: &FetchedFile) -> Placement {
        self.placed
            .lock()
            .unwrap()
            .push((lease.into(), fetch, destination.into(), file.clone()));
        self.discarded.lock().unwrap().push((lease.into(), fetch));
        self.placing.lock().unwrap().clone().unwrap_or_else(|| {
            Placement::Placed(PlacedFile {
                path: format!("/work/{destination}"),
                identity: FileIdentity {
                    device: 1,
                    inode: u64::from(fetch),
                },
            })
        })
    }
    fn take_back(&self, placed: &PlacedFile) -> bool {
        self.taken_back.lock().unwrap().push(placed.clone());
        !self.keeps.load(Ordering::SeqCst)
    }
    fn discard(&self, lease: &str, fetch: u32) {
        self.discarded.lock().unwrap().push((lease.into(), fetch));
    }
    fn close(&self, lease: &str) {
        self.closed.lock().unwrap().push(lease.into());
    }
}

fn fetch_frame(lease: &str, request: &str, destination: &str) -> ToEnvironment {
    ToEnvironment::Fetch {
        lease: lease.into(),
        request: request.into(),
        destination: destination.into(),
        digest: "ab".repeat(32),
        media_type: "image/png".into(),
        size: 20,
    }
}

/// What [`fetch_frame`] asks for, as every fetch entry records it.
fn asked(destination: &str) -> FetchAsked {
    FetchAsked {
        destination: destination.into(),
        digest: "ab".repeat(32),
        media_type: "image/png".into(),
        size: 20,
    }
}

fn fetched_file() -> FetchedFile {
    FetchedFile {
        digest: "ab".repeat(32),
        size: 20,
    }
}

impl Gateway {
    /// A gateway whose host takes fetched files, with one lease granted.
    async fn fetching() -> (Self, Arc<RecordingInbox>) {
        let inbox = Arc::new(RecordingInbox::default());
        let mut gateway = Self::taking(
            Arc::new(MemoryLedger::default()),
            Arc::new(NoOutbox),
            Some(inbox.clone()),
            true,
            real_time(),
        );
        assert_eq!(gateway.next().await, hello());
        gateway.granted(LEASE).await;
        (gateway, inbox)
    }
    /// The ledger's entries about fetches, in order.
    fn fetch_entries(&self) -> Vec<LedgerEntry> {
        self.ledger
            .entries()
            .into_iter()
            .filter(|entry| {
                matches!(
                    entry,
                    LedgerEntry::FetchAccepted { .. }
                        | LedgerEntry::Fetched { .. }
                        | LedgerEntry::FetchUnanswered { .. }
                )
            })
            .collect()
    }
}

/// Slice D3b: a fetch whose destination is free is taken into a part of its
/// own, named by the host's number for it, recorded before it is answered;
/// once the gateway says every byte is delivered the part is checked and
/// placed, and that outcome recorded before it is said. No frame carries
/// the file's bytes.
#[tokio::test]
async fn a_fetch_is_taken_into_a_part_and_placed_once_delivered_each_step_recorded_first() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway
        .send(fetch_frame(LEASE, "call-1", "assets/cat.png"))
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::FetchAccepted {
            lease: LEASE.into(),
            request: "call-1".into(),
            path: "/inbox/0.part".into(),
        }
    );
    assert_eq!(*inbox.parts.lock().unwrap(), [(LEASE.into(), 0)]);
    assert_eq!(
        gateway.fetch_entries(),
        [LedgerEntry::FetchAccepted {
            lease: LEASE.into(),
            request: "call-1".into(),
            fetch: 0,
            asked: asked("assets/cat.png"),
        }]
    );
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    let placed = FetchOutcome::Placed {
        path: "/work/assets/cat.png".into(),
    };
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Fetched {
            lease: LEASE.into(),
            request: "call-1".into(),
            outcome: placed.clone(),
        }
    );
    assert_eq!(
        *inbox.placed.lock().unwrap(),
        [(LEASE.into(), 0, "assets/cat.png".into(), fetched_file())]
    );
    assert_eq!(
        gateway.fetch_entries()[1..],
        [LedgerEntry::Fetched {
            lease: LEASE.into(),
            request: "call-1".into(),
            asked: asked("assets/cat.png"),
            outcome: placed,
        }]
    );
    // Answered once: a second delivery names nothing held.
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    gateway.settled().await;
    assert!(gateway.ledger.entries().iter().any(|entry| matches!(
        entry,
        LedgerEntry::Dropped { frame, .. } if frame == "delivered"
    )));
    // What placing found is said as it is: a part that is not the file.
    *inbox.placing.lock().unwrap() = Some(Placement::Refused(FetchRefusal::Mismatch));
    gateway.send(fetch_frame(LEASE, "call-2", "b.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-2".into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Fetched {
            lease: LEASE.into(),
            request: "call-2".into(),
            outcome: FetchOutcome::Refused {
                reason: FetchRefusal::Mismatch,
            },
        }
    );
}

/// A fetch's last word that cannot be recorded is never said as it was: a
/// file this fetch placed is taken back and the gateway told
/// `auditUnavailable`; one already there is not this fetch's, and stays.
#[tokio::test]
async fn a_fetch_whose_outcome_cannot_be_recorded_is_answered_audit_unavailable_and_taken_back() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway.ledger.failing_fetched.store(true, Ordering::SeqCst);
    let unrecorded = |request: &str| FromEnvironment::Fetched {
        lease: LEASE.into(),
        request: request.into(),
        outcome: FetchOutcome::Refused {
            reason: FetchRefusal::AuditUnavailable,
        },
    };
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    assert_eq!(gateway.next().await, unrecorded("call-1"));
    assert_eq!(
        *inbox.taken_back.lock().unwrap(),
        [PlacedFile {
            path: "/work/a.png".into(),
            identity: FileIdentity {
                device: 1,
                inode: 0,
            },
        }]
    );
    // Already there: said unrecorded, and nothing taken back.
    inbox.at.lock().unwrap().insert(
        "b.png".into(),
        Ok(Destination::AlreadyPlaced {
            path: "/work/b.png".into(),
        }),
    );
    gateway.send(fetch_frame(LEASE, "call-2", "b.png")).await;
    assert_eq!(gateway.next().await, unrecorded("call-2"));
    assert_eq!(inbox.taken_back.lock().unwrap().len(), 1);
    // Placed once the record can be made again: said as it is.
    gateway
        .ledger
        .failing_fetched
        .store(false, Ordering::SeqCst);
    gateway.send(fetch_frame(LEASE, "call-3", "c.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-3".into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Fetched {
            lease: LEASE.into(),
            request: "call-3".into(),
            outcome: FetchOutcome::Placed {
                path: "/work/c.png".into(),
            },
        }
    );
    assert_eq!(inbox.taken_back.lock().unwrap().len(), 1);
}

/// A destination that already is exactly the file is answered at once, and
/// no part is made for it.
#[tokio::test]
async fn a_fetch_whose_destination_already_is_the_file_is_answered_without_a_part() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    inbox.at.lock().unwrap().insert(
        "a.png".into(),
        Ok(Destination::AlreadyPlaced {
            path: "/work/a.png".into(),
        }),
    );
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    let outcome = FetchOutcome::AlreadyPlaced {
        path: "/work/a.png".into(),
    };
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Fetched {
            lease: LEASE.into(),
            request: "call-1".into(),
            outcome: outcome.clone(),
        }
    );
    assert!(inbox.parts.lock().unwrap().is_empty());
    assert_eq!(
        gateway.fetch_entries(),
        [LedgerEntry::Fetched {
            lease: LEASE.into(),
            request: "call-1".into(),
            asked: asked("a.png"),
            outcome,
        }]
    );
}

/// Every refusal is said by name, recorded, and takes nothing: a fetch
/// refused for its own request leaves the one already taken under that name
/// as it was.
#[tokio::test]
async fn a_fetch_is_refused_with_its_reason_and_takes_nothing() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    inbox
        .at
        .lock()
        .unwrap()
        .insert("notes.txt".into(), Err(FetchRefusal::Occupied));
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    let mut empty = fetch_frame(LEASE, "call-2", "a.png");
    if let ToEnvironment::Fetch { size, .. } = &mut empty {
        *size = 0;
    }
    let mut unreadable = fetch_frame(LEASE, "call-3", "a.png");
    if let ToEnvironment::Fetch { digest, .. } = &mut unreadable {
        *digest = "AB".repeat(32);
    }
    for (frame, request, reason) in [
        (
            fetch_frame(LEASE, "call-0", "notes.txt"),
            "call-0",
            FetchRefusal::Occupied,
        ),
        (empty, "call-2", FetchRefusal::Size),
        (unreadable, "call-3", FetchRefusal::Invalid),
        (
            fetch_frame("0b7d3a1e-0000-4000-8000-0000000000ff", "call-4", "a.png"),
            "call-4",
            FetchRefusal::LeaseEnded,
        ),
    ] {
        gateway.send(frame).await;
        assert!(
            matches!(
                gateway.next().await,
                FromEnvironment::FetchRefused { request: refused, reason: said, .. }
                    if refused == request && said == reason
            ),
            "{request}"
        );
    }
    // Asked again under a request still held: dropped, nothing said of it
    // and nothing recorded, so the first's answer stays its only one.
    let before = gateway.fetch_entries();
    gateway.send(fetch_frame(LEASE, "call-1", "b.png")).await;
    gateway.settled().await;
    assert_eq!(gateway.fetch_entries(), before);
    // The first is still taken as it was: its delivery places it.
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Fetched { request, outcome: FetchOutcome::Placed { path }, .. }
            if request == "call-1" && path == "/work/a.png"
    ));
    assert_eq!(*inbox.parts.lock().unwrap(), [(LEASE.into(), 0)]);
    // Past the in-flight bound, busy.
    for n in 0..nessa_protocol::lease::MAX_FETCHES_IN_FLIGHT {
        gateway
            .send(fetch_frame(LEASE, &format!("held-{n}"), "a.png"))
            .await;
        assert!(matches!(
            gateway.next().await,
            FromEnvironment::FetchAccepted { .. }
        ));
    }
    gateway.send(fetch_frame(LEASE, "call-5", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchRefused {
            reason: FetchRefusal::Busy,
            ..
        }
    ));
    // A host that takes no files says so, recorded as any refusal.
    let mut bare = Gateway::start();
    assert_eq!(bare.next().await, hello());
    bare.granted(LEASE).await;
    bare.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert_eq!(
        bare.next().await,
        FromEnvironment::FetchRefused {
            lease: LEASE.into(),
            request: "call-1".into(),
            reason: FetchRefusal::Unavailable,
        }
    );
    assert_eq!(
        bare.fetch_entries(),
        [LedgerEntry::Fetched {
            lease: LEASE.into(),
            request: "call-1".into(),
            asked: asked("a.png"),
            outcome: FetchOutcome::Refused {
                reason: FetchRefusal::Unavailable,
            },
        }]
    );
    // Unrecorded, it is said so instead.
    bare.ledger.failing_fetched.store(true, Ordering::SeqCst);
    bare.send(fetch_frame(LEASE, "call-2", "a.png")).await;
    assert_eq!(
        bare.next().await,
        FromEnvironment::FetchRefused {
            lease: LEASE.into(),
            request: "call-2".into(),
            reason: FetchRefusal::AuditUnavailable,
        }
    );
}

/// A fetch the gateway gives up on before every byte is delivered lets its
/// part go and is recorded unanswered; nothing is placed.
#[tokio::test]
async fn an_undelivered_fetch_lets_its_part_go_and_places_nothing() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    gateway
        .send(ToEnvironment::Undelivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    gateway.settled().await;
    assert_eq!(*inbox.discarded.lock().unwrap(), [(LEASE.into(), 0)]);
    assert!(inbox.placed.lock().unwrap().is_empty());
    assert_eq!(
        gateway.fetch_entries()[1..],
        [LedgerEntry::FetchUnanswered {
            lease: LEASE.into(),
            request: "call-1".into(),
            asked: asked("a.png"),
        }]
    );
}

/// A fetch whose entry could not be recorded leaves its lease's end
/// uncertain: the audit is missing part of what happened under it.
#[tokio::test]
async fn a_fetch_entry_that_cannot_be_recorded_makes_its_leases_end_uncertain() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway
        .ledger
        .failing_fetch_unanswered
        .store(true, Ordering::SeqCst);
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    gateway
        .send(ToEnvironment::Undelivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    gateway.settled().await;
    // Its part is let go all the same.
    assert_eq!(*inbox.discarded.lock().unwrap(), [(LEASE.into(), 0)]);
    assert_eq!(
        gateway.ended(LEASE).await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Uncertain,
        }
    );
    assert!(gateway.ledger.entries().iter().any(|entry| matches!(
        entry,
        LedgerEntry::Ended { lease, cleanup: Cleanup::Uncertain, .. } if lease == LEASE
    )));
}

/// A fetch that cannot be recorded is not taken: refused as unrecorded, its
/// part let go.
#[tokio::test]
async fn a_fetch_that_cannot_be_recorded_is_refused_and_its_part_let_go() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway
        .ledger
        .failing_fetch_accepted
        .store(true, Ordering::SeqCst);
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::FetchRefused {
            lease: LEASE.into(),
            request: "call-1".into(),
            reason: FetchRefusal::AuditUnavailable,
        }
    );
    assert_eq!(*inbox.discarded.lock().unwrap(), [(LEASE.into(), 0)]);
}

/// The lease ending answers every fetch it took and never placed, recorded
/// and said before the lease's own end, and lets every part go.
#[tokio::test]
async fn a_lease_end_answers_every_fetch_under_way_before_its_ended() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    let ended = FetchOutcome::Refused {
        reason: FetchRefusal::LeaseEnded,
    };
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Fetched {
            lease: LEASE.into(),
            request: "call-1".into(),
            outcome: ended.clone(),
        }
    );
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ended { .. }
    ));
    let entries = gateway.ledger.entries();
    let fetched = entries
        .iter()
        .position(|entry| {
            *entry
                == LedgerEntry::Fetched {
                    lease: LEASE.into(),
                    request: "call-1".into(),
                    asked: asked("a.png"),
                    outcome: ended.clone(),
                }
        })
        .expect("recorded as ended");
    let end = entries
        .iter()
        .position(|entry| matches!(entry, LedgerEntry::Ended { .. }))
        .expect("ended");
    assert!(fetched < end);
    assert_eq!(*inbox.closed.lock().unwrap(), [String::from(LEASE)]);
}

/// The connection lost with a fetch under way records it unanswered: the
/// gateway is told nothing, and nothing is placed.
#[tokio::test]
async fn a_fetch_cut_off_by_a_lost_connection_is_recorded_unanswered() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    let Gateway {
        writer,
        frames,
        served,
        ledger,
        ..
    } = gateway;
    drop(writer);
    drop(frames);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("serving ends with its stream")
        .unwrap();
    let entries = ledger.entries();
    assert!(entries.contains(&LedgerEntry::FetchUnanswered {
        lease: LEASE.into(),
        request: "call-1".into(),
        asked: asked("a.png"),
    }));
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::Fetched { .. })),
        "{entries:?}"
    );
    assert!(inbox.placed.lock().unwrap().is_empty());
    assert_eq!(*inbox.closed.lock().unwrap(), [String::from(LEASE)]);
}

/// A fetch the gateway gives up on while it is still being accepted is let
/// go once its part is made: never offered, recorded unanswered, and its
/// place under the in-flight bound freed.
#[tokio::test]
async fn a_fetch_given_up_while_being_accepted_lets_its_part_go_once_made() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    let (go, held) = std::sync::mpsc::channel();
    *inbox.hold_part.lock().unwrap() = Some(held);
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    gateway
        .send(ToEnvironment::Undelivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    gateway.settled().await;
    go.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while inbox.discarded.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("its part is let go");
    assert_eq!(*inbox.discarded.lock().unwrap(), [(LEASE.into(), 0)]);
    assert_eq!(
        gateway.fetch_entries(),
        [LedgerEntry::FetchUnanswered {
            lease: LEASE.into(),
            request: "call-1".into(),
            asked: asked("a.png"),
        }]
    );
    // Its place is free: as many as the bound are taken after it.
    for n in 0..nessa_protocol::lease::MAX_FETCHES_IN_FLIGHT {
        gateway
            .send(fetch_frame(LEASE, &format!("held-{n}"), "a.png"))
            .await;
        assert!(matches!(
            gateway.next().await,
            FromEnvironment::FetchAccepted { request, .. } if request == format!("held-{n}")
        ));
    }
}

/// A fetch the gateway gave up on while it was being accepted is recorded
/// unanswered whatever the acceptance then finds, a refusal included, and
/// nothing more is said of it.
#[tokio::test]
async fn a_fetch_given_up_while_being_accepted_is_unanswered_even_when_refused() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    let (go, held) = std::sync::mpsc::channel();
    *inbox.hold_part.lock().unwrap() = Some(held);
    *inbox.refuse_part.lock().unwrap() = Some(FetchRefusal::PlacingFailed);
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    gateway
        .send(ToEnvironment::Undelivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    gateway.settled().await;
    go.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while gateway.fetch_entries().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("it is recorded");
    assert_eq!(
        gateway.fetch_entries(),
        [LedgerEntry::FetchUnanswered {
            lease: LEASE.into(),
            request: "call-1".into(),
            asked: asked("a.png"),
        }]
    );
    // Nothing was said of it: the next frame is the next fetch's.
    *inbox.refuse_part.lock().unwrap() = None;
    gateway.send(fetch_frame(LEASE, "call-2", "b.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { request, .. } if request == "call-2"
    ));
}

/// A placed file whose placing cannot be recorded, and which cannot be
/// taken back, is still said placed, never refused: the gateway keeps
/// counting it, and the lease's end is uncertain.
#[tokio::test]
async fn a_placed_file_that_cannot_be_taken_back_is_still_said_placed() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    gateway.ledger.failing_fetched.store(true, Ordering::SeqCst);
    inbox.keeps.store(true, Ordering::SeqCst);
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    gateway
        .send(ToEnvironment::Delivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Fetched {
            lease: LEASE.into(),
            request: "call-1".into(),
            outcome: FetchOutcome::Placed {
                path: "/work/a.png".into(),
            },
        }
    );
    assert_eq!(inbox.taken_back.lock().unwrap().len(), 1);
}

/// A fetch the gateway gave up on while its part was being made, whose
/// lease then ends before the part is made, keeps the gateway's giving up
/// as its cause: recorded unanswered, never relabelled lease ended.
#[tokio::test]
async fn a_fetch_given_up_before_its_lease_ends_stays_unanswered() {
    let (mut gateway, inbox) = Gateway::fetching().await;
    let (go, held) = std::sync::mpsc::channel();
    *inbox.hold_part.lock().unwrap() = Some(held);
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    gateway
        .send(ToEnvironment::Undelivered {
            lease: LEASE.into(),
            request: "call-1".into(),
        })
        .await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    gateway.settled().await;
    go.send(()).unwrap();
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ended { lease, .. } if lease == LEASE
    ));
    assert_eq!(
        gateway.fetch_entries(),
        [LedgerEntry::FetchUnanswered {
            lease: LEASE.into(),
            request: "call-1".into(),
            asked: asked("a.png"),
        }]
    );
    assert!(inbox.placed.lock().unwrap().is_empty());
}

// Row I7: a lease lasts at most what the peer's grant lets it, measured on
// the serving timer, and this side ends it itself past that.

/// How long the tests below let a harness's lease last: short of the silence
/// bound, so a deadline is reached on a connection still served.
const MAX_LEASE: Duration = Duration::from_secs(30);

/// Served as SSH granted everything, its leases lasting at most
/// [`MAX_LEASE`], on `timer`, recording in `ledger`.
fn expiring(timer: Arc<ManualTimer>, ledger: Arc<MemoryLedger>) -> Gateway {
    expiring_over(timer, ledger, Arc::new(NoOutbox))
}

/// As [`expiring`], publishing through `outbox`.
fn expiring_over(
    timer: Arc<ManualTimer>,
    ledger: Arc<MemoryLedger>,
    outbox: Arc<dyn ArtifactOutbox>,
) -> Gateway {
    expiring_through(1 << 20, timer, ledger, outbox, None)
}

/// As [`expiring_over`], over a connection holding at most `buffer` bytes
/// each way that are not yet read.
fn expiring_through(
    buffer: usize,
    timer: Arc<ManualTimer>,
    ledger: Arc<MemoryLedger>,
    outbox: Arc<dyn ArtifactOutbox>,
    inbox: Option<Arc<dyn ArtifactInbox>>,
) -> Gateway {
    let grants = MemoryGrants::none();
    grants.set(
        Peer::Ssh,
        Some(
            PeerGrant::everything()
                .with_max_lease(MaxLease::new(MAX_LEASE.as_millis() as u64).unwrap()),
        ),
    );
    Gateway::through(
        buffer,
        Peer::Ssh,
        grants,
        ledger,
        outbox,
        inbox,
        true,
        timer,
    )
}

impl Gateway {
    /// A lease granted for at most [`MAX_LEASE`], with one harness started
    /// under it.
    async fn started_expiring(&mut self, lease: &str) {
        assert_eq!(self.next().await, hello());
        self.send(ToEnvironment::Grant {
            lease: lease.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
        assert_eq!(
            self.next().await,
            FromEnvironment::Granted {
                lease: lease.into(),
                terms: GrantedTerms {
                    lease_ms: Some(MAX_LEASE.as_millis() as u64),
                    ..harness_terms()
                },
            }
        );
        self.send(ToEnvironment::Start {
            lease: lease.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
        self.settled().await;
    }
    /// What the host answers an `End` of `lease` with.
    async fn ended(&mut self, lease: &str) -> FromEnvironment {
        self.send(ToEnvironment::End {
            lease: lease.into(),
        })
        .await;
        self.said().await
    }
}

fn ended_entries(ledger: &MemoryLedger, lease: &str) -> usize {
    ledger
        .entries()
        .iter()
        .filter(|entry| matches!(entry, LedgerEntry::Ended { lease: id, .. } if id == lease))
        .count()
}

#[tokio::test]
async fn a_lease_past_its_granted_duration_is_stopped_recorded_expired_and_said() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    gateway.started_expiring(LEASE).await;
    // A moment short of it, nothing has happened: the harness still echoes.
    timer.advance(MAX_LEASE - Duration::from_millis(1));
    gateway
        .send(ToEnvironment::Input {
            lease: LEASE.into(),
            channel: 1,
            data: Data(b"ping\n".to_vec()),
        })
        .await;
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Output {
            lease: LEASE.into(),
            channel: 1,
            data: Data(b"ping\n".to_vec()),
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 0);
    assert!(!gateway
        .ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Expiring { .. })));
    // The last frame the gateway sent was read here.
    let heard = timer.now();
    // At it, the host ends the lease itself and says so.
    timer.advance(Duration::from_millis(1));
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Expired,
        }
    );
    // A deadline passing is not the gateway heard from: with nothing more
    // sent, the next read is still bounded by silence from its last frame.
    let deadline = timer.now();
    let read = timer
        .started(|_, wait| wait.started >= deadline && wait.deadline == heard + SILENCE_LIMIT)
        .await;
    assert!(timer
        .waits()
        .iter()
        .all(|wait| wait.started < deadline || wait.deadline <= read.deadline));
    // The gateway's End is answered with what that end took.
    assert_eq!(
        gateway.ended(LEASE).await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        gateway.ledger.entries(),
        [
            LedgerEntry::Granted {
                lease: LEASE.into(),
                peer: Peer::Ssh,
                agent: "claude".into(),
                sandbox: Sandbox::HarnessDefault,
                lease_ms: Some(30_000),
            },
            LedgerEntry::Started {
                lease: LEASE.into(),
                channel: 1,
            },
            LedgerEntry::Expiring {
                lease: LEASE.into(),
                lease_ms: 30_000,
            },
            LedgerEntry::Stopped {
                lease: LEASE.into(),
                channel: 1,
                cleanup: Cleanup::Confirmed { forced: false },
            },
            LedgerEntry::Ended {
                lease: LEASE.into(),
                cleanup: Cleanup::Confirmed { forced: false },
                cause: EndCause::Expired,
            },
        ]
    );
}

#[tokio::test]
async fn a_stop_after_an_expiry_is_answered_with_what_that_end_took() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    gateway.started_expiring(LEASE).await;
    timer.advance(MAX_LEASE);
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Expired,
        }
    );
    // The gateway closes its agent and stops the harness the host is
    // already stopping: answered once that stop is done, not as unheld.
    gateway
        .send(ToEnvironment::Stop {
            lease: LEASE.into(),
            channel: 1,
            grace_ms: 10,
            kill_ms: 100,
        })
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Stopped {
            lease: LEASE.into(),
            channel: 1,
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert!(!gateway
        .ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Dropped { .. })));
}

#[tokio::test]
async fn an_end_and_a_deadline_at_once_end_the_lease_once() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    gateway.started_expiring(LEASE).await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    timer.advance(MAX_LEASE);
    // Whichever came first ends it; the End is answered by that end.
    let ended = loop {
        match gateway.said().await {
            FromEnvironment::Ending { .. } => continue,
            other => break other,
        }
    };
    assert_eq!(
        ended,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    gateway.settled().await;
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(ended_entries(&gateway.ledger, LEASE), 1);
}

#[tokio::test]
async fn a_lease_with_no_max_never_expires() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = Gateway::timed(timer.clone());
    assert_eq!(gateway.next().await, hello());
    // Granted with no duration: it lasts until it is ended.
    gateway.granted(LEASE).await;
    gateway
        .send(ToEnvironment::Start {
            lease: LEASE.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
    gateway.settled().await;
    for _ in 0..8 {
        timer.advance(KEEPALIVE_INTERVAL);
        gateway.send(ToEnvironment::Keepalive).await;
        // Its wait ends only at the silence bound: no deadline shortens it.
        let heard = timer.now();
        timer
            .started(|_, wait| wait.deadline == heard + SILENCE_LIMIT)
            .await;
    }
    assert_eq!(
        gateway.ended(LEASE).await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(
        gateway.ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
            cause: EndCause::Asked,
        })
    );
    assert!(!gateway
        .ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Expiring { .. })));
}

#[tokio::test]
async fn an_expiry_that_cannot_be_recorded_still_stops_and_answers_uncertain() {
    let timer = Arc::new(ManualTimer::default());
    let ledger = Arc::new(MemoryLedger::default());
    ledger.failing_expiring.store(true, Ordering::SeqCst);
    let mut gateway = expiring(timer.clone(), ledger.clone());
    gateway.started_expiring(LEASE).await;
    timer.advance(MAX_LEASE);
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Expired,
        }
    );
    assert_eq!(
        gateway.ended(LEASE).await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Uncertain,
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Uncertain,
            cause: EndCause::Expired,
        })
    );
}

#[tokio::test]
async fn a_deadline_after_the_connection_is_lost_records_nothing_more() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    gateway.started_expiring(LEASE).await;
    let Gateway {
        writer,
        frames,
        served,
        ledger,
        stopped,
        ..
    } = gateway;
    drop(writer);
    drop(frames);
    tokio::time::timeout(Duration::from_secs(5), served)
        .await
        .expect("serving ends with its stream")
        .unwrap();
    let lost = ledger.entries();
    assert_eq!(
        lost.last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
            cause: EndCause::Lost,
        })
    );
    timer.advance(MAX_LEASE * 2);
    tokio::task::yield_now().await;
    assert_eq!(ledger.entries(), lost);
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
}

#[tokio::test]
async fn a_command_is_bounded_by_its_timeout_not_the_max_lease() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(COMMAND, &["wait"], None)).await;
    // Its timeout is its deadline: no lease duration is granted it.
    assert_eq!(
        gateway.next().await,
        FromEnvironment::Granted {
            lease: COMMAND.into(),
            terms: command_terms(1_000),
        }
    );
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    gateway.settled().await;
    timer.advance(MAX_LEASE * 2 / 3);
    gateway.settled().await;
    timer.advance(MAX_LEASE * 2 / 3);
    gateway.settled().await;
    // Past the max lease, the command still runs, with the timeout granted.
    assert_eq!(gateway.commands.ran.lock().unwrap()[0].timeout_ms(), 1_000);
    gateway
        .send(ToEnvironment::End {
            lease: COMMAND.into(),
        })
        .await;
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ran {
            end: CommandEnd::Stopped,
            ..
        }
    ));
    assert!(matches!(
        gateway.next().await,
        FromEnvironment::Ended { .. }
    ));
    assert!(!gateway
        .ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Expiring { .. })));
    assert_eq!(
        gateway.ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: COMMAND.into(),
            cleanup: Cleanup::Confirmed { forced: true },
            cause: EndCause::Asked,
        })
    );
}

/// A gateway that sends without pause, so a frame is always ready to read,
/// still has its lease ended at the deadline; a Start that arrives after it
/// starts nothing.
#[tokio::test]
async fn a_gateway_sending_without_pause_cannot_put_off_an_expiry() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    gateway.started_expiring(LEASE).await;
    let (spare, _) = duplex(1);
    let mut writer = std::mem::replace(&mut gateway.writer, split(spare).1);
    let late = Arc::new(AtomicBool::new(false));
    let (stop, mut stopped) = tokio::sync::watch::channel(false);
    let sending = {
        let late = late.clone();
        tokio::spawn(async move {
            let mut started = false;
            while !*stopped.borrow_and_update() {
                if late.load(Ordering::SeqCst) && !started {
                    started = true;
                    let start = ToEnvironment::Start {
                        lease: LEASE.into(),
                        channel: 2,
                        environment: BTreeMap::new(),
                    };
                    write_frame(&mut writer, &start).await.unwrap();
                }
                if write_frame(&mut writer, &ToEnvironment::Keepalive)
                    .await
                    .is_err()
                {
                    break;
                }
                tokio::task::yield_now().await;
            }
        })
    };
    timer.advance(MAX_LEASE);
    late.store(true, Ordering::SeqCst);
    let mut ending = false;
    let mut late_start = false;
    while !(ending && late_start) {
        match gateway.said().await {
            FromEnvironment::Ending { lease, cause } => {
                assert_eq!((lease.as_str(), cause), (LEASE, EndingCause::Expired));
                ending = true;
            }
            FromEnvironment::StartFailed {
                channel: 2, reason, ..
            } => {
                assert_eq!(reason, StartFailure::NotGranted);
                late_start = true;
            }
            other => panic!("unexpected {other:?}"),
        }
    }
    stop.send_replace(true);
    sending.await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        while ended_entries(&gateway.ledger, LEASE) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the expired lease's end is recorded");
    let entries = gateway.ledger.entries();
    assert!(entries.contains(&LedgerEntry::Expiring {
        lease: LEASE.into(),
        lease_ms: 30_000,
    }));
    assert!(entries.contains(&LedgerEntry::Ended {
        lease: LEASE.into(),
        cleanup: Cleanup::Confirmed { forced: false },
        cause: EndCause::Expired,
    }));
    // Only the harness started before the deadline was ever launched.
    assert_eq!(gateway.launcher.launched.lock().unwrap().len(), 1);
    assert!(!entries
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Started { channel: 2, .. })));
}

/// A file still in flight when its lease expires may already be kept by the
/// gateway: it is recorded unanswered, never refused as the lease ending,
/// which only the gateway's own end says.
#[tokio::test]
async fn a_file_in_flight_when_its_lease_expires_is_left_unanswered() {
    let timer = Arc::new(ManualTimer::default());
    let outbox = Arc::new(RecordingOutbox::default());
    let mut gateway = expiring_over(
        timer.clone(),
        Arc::new(MemoryLedger::default()),
        outbox.clone(),
    );
    gateway.started_expiring(LEASE).await;
    let answered = publish(&outbox, LEASE).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Published { artifact: 0, .. }
    ));
    timer.advance(MAX_LEASE);
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ending { .. }
    ));
    assert_eq!(
        answer_of(answered).await,
        crate::env_serve::application::PublishAnswer::Unanswered
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while ended_entries(&gateway.ledger, LEASE) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the expired lease's end is recorded");
    let entries = gateway.ledger.entries();
    assert!(entries.contains(&LedgerEntry::Unanswered {
        lease: LEASE.into(),
        artifact: 0,
    }));
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::Collected { .. })),
        "{entries:?}"
    );
}

/// A reply that cannot be sent, the gateway reading nothing, does not hold
/// an expiry back: at the deadline the lease is ended, its harness stopped
/// and both recorded, long before the silence bound.
#[tokio::test]
async fn a_reply_blocked_on_a_full_output_queue_does_not_hold_back_an_expiry() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring_through(
        512,
        timer.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        None,
    );
    gateway.started_expiring(LEASE).await;
    // The gateway asks far more than the connection and the output queue
    // hold, and reads none of it: the host is soon held up answering.
    let (spare, _) = duplex(1);
    let mut writer = std::mem::replace(&mut gateway.writer, split(spare).1);
    let asking = tokio::spawn(async move {
        for _ in 0..2_000 {
            let ask = ToEnvironment::Account {
                lease: "settled".into(),
            };
            if write_frame(&mut writer, &ask).await.is_err() {
                break;
            }
        }
        writer
    });
    // Once the host is held up, it starts no more waits: every frame it
    // reads and answers starts one.
    let mut seen = timer.waits().len();
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = timer.waits().len();
        if now == seen {
            break;
        }
        seen = now;
    }
    assert!(!asking.is_finished(), "the host is held up answering");
    timer.advance(MAX_LEASE);
    // Moved on only by the stops' own short bounds, never near silence.
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if ended_entries(&gateway.ledger, LEASE) == 1 {
                break;
            }
            if timer.elapsed() < MAX_LEASE + Duration::from_secs(1) {
                timer.advance(Duration::from_millis(10));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(ended.is_ok(), "{:?}", gateway.ledger.entries());
    assert!(timer.elapsed() < SILENCE_LIMIT);
    let entries = gateway.ledger.entries();
    assert!(entries.contains(&LedgerEntry::Expiring {
        lease: LEASE.into(),
        lease_ms: 30_000,
    }));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        LedgerEntry::Ended {
            lease,
            cause: EndCause::Expired,
            ..
        } if lease == LEASE
    )));
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    asking.abort();
    gateway.served.abort();
}

const OTHER_LEASE: &str = "0b7d3a1e-0000-4000-8000-000000000002";

impl Gateway {
    /// Another lease granted for at most [`MAX_LEASE`] from now, with one
    /// harness started under it.
    async fn also_expiring(&mut self, lease: &str) {
        self.send(ToEnvironment::Grant {
            lease: lease.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
        assert!(matches!(self.next().await, FromEnvironment::Granted { .. }));
        self.send(ToEnvironment::Start {
            lease: lease.into(),
            channel: 1,
            environment: BTreeMap::new(),
        })
        .await;
        self.settled().await;
    }

    /// Ask far more than the connection and the output queue hold, reading
    /// none of it, until the host is held up answering: then it starts no
    /// more waits on `timer`, as every frame it reads and answers starts one.
    async fn flood(&mut self, timer: &ManualTimer) -> JoinHandle<()> {
        let (spare, _) = duplex(1);
        let mut writer = std::mem::replace(&mut self.writer, split(spare).1);
        let asking = tokio::spawn(async move {
            for _ in 0..2_000 {
                let ask = ToEnvironment::Account {
                    lease: "settled".into(),
                };
                if write_frame(&mut writer, &ask).await.is_err() {
                    break;
                }
            }
        });
        let mut seen = timer.waits().len();
        loop {
            tokio::time::sleep(Duration::from_millis(200)).await;
            let now = timer.waits().len();
            if now == seen {
                break;
            }
            seen = now;
        }
        assert!(!asking.is_finished(), "the host is held up answering");
        asking
    }
}

/// A fetch whose answer cannot be sent, the gateway reading nothing, does
/// not hold serving open once the gateway is gone: its lease's end records
/// the fetch's last word, and at the silence bound serving ends.
#[tokio::test]
async fn a_fetch_answer_blocked_on_a_full_output_queue_does_not_hold_serving_open() {
    let timer = Arc::new(ManualTimer::default());
    let inbox = Arc::new(RecordingInbox::default());
    let mut gateway = expiring_through(
        512,
        timer.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        Some(inbox.clone()),
    );
    gateway.started_expiring(LEASE).await;
    let (go, held) = std::sync::mpsc::channel();
    *inbox.hold_part.lock().unwrap() = Some(held);
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    gateway.settled().await;
    let asking = gateway.flood(&timer).await;
    // Its part made only now: its acceptance finds the queue full.
    go.send(()).unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while !gateway.served.is_finished() {
            timer.advance(Duration::from_millis(500));
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(ended.is_ok(), "{:?}", gateway.ledger.entries());
    // Its lease ended under it (expired first, at this pace) and its last
    // word recorded, whether or not the gateway ever reads it.
    assert!(gateway.ledger.entries().iter().any(|entry| matches!(
        entry,
        LedgerEntry::Fetched { request, .. } | LedgerEntry::FetchUnanswered { request, .. }
            if request == "call-1"
    )));
    asking.abort();
}

/// Wait until `lease` is recorded ended, moving `timer` on only by the
/// stops' own short bounds, and never past `by`.
async fn until_ended(gateway: &Gateway, timer: &ManualTimer, lease: &str, by: Duration) {
    let ended = tokio::time::timeout(Duration::from_secs(5), async {
        while ended_entries(&gateway.ledger, lease) == 0 {
            if timer.elapsed() + Duration::from_millis(10) <= by {
                timer.advance(Duration::from_millis(10));
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(ended.is_ok(), "{lease}: {:?}", gateway.ledger.entries());
}

fn expired(gateway: &Gateway, lease: &str) -> bool {
    let entries = gateway.ledger.entries();
    entries.contains(&LedgerEntry::Expiring {
        lease: lease.into(),
        lease_ms: 30_000,
    }) && entries.iter().any(|entry| {
        matches!(
            entry,
            LedgerEntry::Ended {
                lease: id,
                cause: EndCause::Expired,
                ..
            } if id == lease
        )
    })
}

/// Row K1: a drain asked while a gateway that reads nothing holds the host
/// up answering starts at once: the lease is ended, recorded drained, long
/// before any deadline, and not left to be found lost.
#[tokio::test]
async fn k1_a_drain_asked_while_the_gateway_reads_nothing_ends_the_lease_at_once() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring_through(
        512,
        timer.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        None,
    );
    gateway.started_expiring(LEASE).await;
    let asking = gateway.flood(&timer).await;
    gateway.drain.send_replace(true);
    until_ended(&gateway, &timer, LEASE, Duration::from_secs(1)).await;
    let entries = gateway.ledger.entries();
    assert!(
        entries.contains(&LedgerEntry::Draining {
            lease: LEASE.into(),
            peer: Peer::Ssh,
        }),
        "{entries:?}"
    );
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            LedgerEntry::Ended {
                lease,
                cause: EndCause::Drained,
                ..
            } if lease == LEASE
        )),
        "{entries:?}"
    );
    assert!(timer.elapsed() < Duration::from_secs(1));
    asking.abort();
    gateway.served.abort();
}

/// Two leases due at once, the gateway reading nothing: saying the first
/// is ending waits for room, and the second is ended at the deadline all
/// the same.
#[tokio::test]
async fn two_leases_due_at_once_both_expire_at_the_deadline() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring_through(
        512,
        timer.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        None,
    );
    gateway.started_expiring(LEASE).await;
    gateway.also_expiring(OTHER_LEASE).await;
    let asking = gateway.flood(&timer).await;
    timer.advance(MAX_LEASE);
    let by = MAX_LEASE + Duration::from_secs(1);
    until_ended(&gateway, &timer, LEASE, by).await;
    until_ended(&gateway, &timer, OTHER_LEASE, by).await;
    assert!(expired(&gateway, LEASE) && expired(&gateway, OTHER_LEASE));
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 2);
    assert!(timer.elapsed() < SILENCE_LIMIT);
    asking.abort();
    gateway.served.abort();
}

/// Two leases due one after the other, both before the silence bound, the
/// gateway reading nothing: each is ended at its own deadline, the second
/// while saying the first is ending still waits for room.
#[tokio::test]
async fn staggered_deadlines_each_expire_at_their_own() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring_through(
        512,
        timer.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        None,
    );
    gateway.started_expiring(LEASE).await;
    let later = Duration::from_secs(10);
    timer.advance(later);
    gateway.also_expiring(OTHER_LEASE).await;
    let asking = gateway.flood(&timer).await;
    // The first deadline: only the first lease ends.
    timer.advance_to(Moment::from_origin(MAX_LEASE));
    until_ended(&gateway, &timer, LEASE, MAX_LEASE + Duration::from_secs(1)).await;
    assert!(expired(&gateway, LEASE));
    assert_eq!(ended_entries(&gateway.ledger, OTHER_LEASE), 0);
    // The second, while the first's `Ending` still waits for room.
    timer.advance_to(Moment::from_origin(later + MAX_LEASE));
    until_ended(
        &gateway,
        &timer,
        OTHER_LEASE,
        later + MAX_LEASE + Duration::from_secs(1),
    )
    .await;
    assert!(expired(&gateway, OTHER_LEASE));
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 2);
    assert!(timer.elapsed() < later + SILENCE_LIMIT);
    asking.abort();
    gateway.served.abort();
}

/// Wait until the host starts no more waits on `timer` for 200 ms: it is
/// then waiting on what it cannot do, or on nothing.
async fn quiet(timer: &ManualTimer) {
    let mut seen = timer.waits().len();
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = timer.waits().len();
        if now == seen {
            break;
        }
        seen = now;
    }
}

/// A gateway that reads nothing: a lease with no deadline whose harness
/// echoes far more than the connection and the output queue hold, then a
/// grant of [`OTHER_LEASE`] for at most `lease_ms`, its `Granted` waiting
/// for room. Nothing has moved `timer` on, so nothing has been heard
/// since its start.
async fn granted_unread(timer: &Arc<ManualTimer>, lease_ms: u64) -> Gateway {
    let grants = MemoryGrants::none();
    grants.set(Peer::Ssh, Some(PeerGrant::everything()));
    let mut gateway = Gateway::through(
        512,
        Peer::Ssh,
        grants.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        None,
        true,
        timer.clone(),
    );
    gateway.started(LEASE).await;
    for _ in 0..200 {
        gateway
            .send(ToEnvironment::Input {
                lease: LEASE.into(),
                channel: 1,
                data: Data(vec![b'x'; 2048]),
            })
            .await;
    }
    quiet(timer).await;
    grants.set(
        Peer::Ssh,
        Some(PeerGrant::everything().with_max_lease(MaxLease::new(lease_ms).unwrap())),
    );
    gateway
        .send(ToEnvironment::Grant {
            lease: OTHER_LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    quiet(timer).await;
    assert!(gateway.ledger.entries().iter().any(|entry| matches!(
        entry,
        LedgerEntry::Granted { lease, lease_ms: Some(ms), .. }
            if lease == OTHER_LEASE && *ms == lease_ms
    )));
    gateway
}

/// Whether `lease` is recorded expiring, for `lease_ms`, and then ended
/// as expired.
fn expired_after_expiring(entries: &[LedgerEntry], lease: &str, lease_ms: u64) -> bool {
    let expiring = entries.iter().position(|entry| {
        *entry
            == LedgerEntry::Expiring {
                lease: lease.into(),
                lease_ms,
            }
    });
    let ended = entries.iter().position(|entry| {
        matches!(
            entry,
            LedgerEntry::Ended {
                lease: id,
                cause: EndCause::Expired,
                ..
            } if id == lease
        )
    });
    matches!((expiring, ended), (Some(expiring), Some(ended)) if expiring < ended)
}

/// A lease granted while the gateway reads nothing, its `Granted` waiting
/// for room behind a harness's output: it is ended at its own deadline,
/// the earliest there is, and not taken as lost at the silence bound.
#[tokio::test]
async fn a_lease_granted_while_the_gateway_reads_nothing_expires_at_its_deadline() {
    let timer = Arc::new(ManualTimer::default());
    let lease_ms = 5_000;
    let gateway = granted_unread(&timer, lease_ms).await;
    let deadline = Duration::from_millis(lease_ms);
    timer.advance(deadline);
    until_ended(
        &gateway,
        &timer,
        OTHER_LEASE,
        deadline + Duration::from_secs(1),
    )
    .await;
    let entries = gateway.ledger.entries();
    assert!(
        expired_after_expiring(&entries, OTHER_LEASE, lease_ms),
        "{entries:?}"
    );
    assert!(timer.elapsed() < SILENCE_LIMIT);
    // The lease with no deadline still runs.
    assert_eq!(ended_entries(&gateway.ledger, LEASE), 0);
    gateway.served.abort();
}

/// A lease due at the very moment the gateway is taken as gone is
/// expired, not lost; the lease with no deadline is lost.
#[tokio::test]
async fn a_lease_due_as_the_gateway_falls_silent_is_expired_not_lost() {
    let timer = Arc::new(ManualTimer::default());
    let lease_ms = SILENCE_LIMIT.as_millis() as u64;
    let gateway = granted_unread(&timer, lease_ms).await;
    timer.advance(SILENCE_LIMIT);
    // Taken as gone: what it held is ended, moved on only by the stops'
    // own short bounds.
    until_ended(
        &gateway,
        &timer,
        LEASE,
        SILENCE_LIMIT + Duration::from_secs(1),
    )
    .await;
    let entries = gateway.ledger.entries();
    assert!(
        expired_after_expiring(&entries, OTHER_LEASE, lease_ms),
        "{entries:?}"
    );
    assert!(entries.iter().all(|entry| !matches!(
        entry,
        LedgerEntry::Ended { lease, cause: EndCause::Lost, .. } if lease == OTHER_LEASE
    )));
    assert!(entries.iter().any(|entry| matches!(
        entry,
        LedgerEntry::Ended { lease, cause: EndCause::Lost, .. } if lease == LEASE
    )));
    gateway.served.abort();
}

/// A fetch under way when its lease expires is answered `leaseEnded` before
/// the lease's end is recorded, as on any end, and its parts are let go.
#[tokio::test]
async fn a_fetch_under_way_when_its_lease_expires_is_answered_lease_ended() {
    let timer = Arc::new(ManualTimer::default());
    let inbox = Arc::new(RecordingInbox::default());
    let mut gateway = expiring_through(
        1 << 20,
        timer.clone(),
        Arc::new(MemoryLedger::default()),
        Arc::new(NoOutbox),
        Some(inbox.clone()),
    );
    gateway.started_expiring(LEASE).await;
    gateway.send(fetch_frame(LEASE, "call-1", "a.png")).await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::FetchAccepted { .. }
    ));
    timer.advance(MAX_LEASE);
    let ended = FetchOutcome::Refused {
        reason: FetchRefusal::LeaseEnded,
    };
    let (mut ending, mut fetched) = (false, false);
    while !(ending && fetched) {
        match gateway.said().await {
            FromEnvironment::Ending { lease, .. } if lease == LEASE => ending = true,
            FromEnvironment::Fetched {
                request, outcome, ..
            } if request == "call-1" => {
                assert_eq!(outcome, ended);
                fetched = true;
            }
            other => panic!("unexpected: {other:?}"),
        }
    }
    assert_eq!(
        gateway.ended(LEASE).await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(*inbox.closed.lock().unwrap(), [String::from(LEASE)]);
    let entries = gateway.ledger.entries();
    let fetched_at = entries
        .iter()
        .position(|entry| {
            *entry
                == LedgerEntry::Fetched {
                    lease: LEASE.into(),
                    request: "call-1".into(),
                    asked: asked("a.png"),
                    outcome: ended.clone(),
                }
        })
        .expect("its last word is recorded");
    let ended_at = entries
        .iter()
        .position(|entry| matches!(entry, LedgerEntry::Ended { .. }))
        .expect("the end is recorded");
    assert!(fetched_at < ended_at, "{entries:?}");
}

// Row I8 on the host: the gateway says it no longer lets this host serve it.

/// A dialed gateway granted everything, served on the runtime's clock.
fn revocable(ledger: Arc<MemoryLedger>) -> Gateway {
    let grants = MemoryGrants::none();
    grants.set(DIALED, Some(PeerGrant::everything()));
    Gateway::for_peer(
        DIALED,
        grants,
        ledger,
        Arc::new(NoOutbox),
        true,
        real_time(),
    )
}

const UNRUN: &str = "0b7d3a1e-0000-4000-8000-0000000000c3";

#[tokio::test]
async fn a_revoked_gateway_has_every_lease_ended_recorded_revoked_and_its_ends_answered() {
    let mut gateway = revocable(Arc::new(MemoryLedger::default()));
    gateway.started(LEASE).await;
    gateway.send(grant_command(COMMAND, &["wait"], None)).await;
    gateway.next().await;
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    gateway.send(grant_command(UNRUN, &["wait"], None)).await;
    gateway.next().await;
    gateway.settled().await;
    // Said twice, acted on once.
    gateway.send(ToEnvironment::Revoked).await;
    gateway.send(ToEnvironment::Revoked).await;
    // Nothing more is admitted.
    gateway
        .send(ToEnvironment::Grant {
            lease: OTHER_LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    const CROSSED: &str = "0b7d3a1e-0000-4000-8000-0000000000c4";
    gateway.send(grant_command(CROSSED, &["echo"], None)).await;
    let (mut refused, mut crossed, mut ran) = (None, None, None);
    while refused.is_none() || crossed.is_none() || ran.is_none() {
        match gateway.said().await {
            FromEnvironment::Refused { lease, reason } if lease == CROSSED => {
                crossed = Some(reason)
            }
            frame @ FromEnvironment::Refused { .. } => refused = Some(frame),
            frame @ FromEnvironment::Ran { .. } => ran = Some(frame),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(
        refused,
        Some(FromEnvironment::Refused {
            lease: OTHER_LEASE.into(),
            reason: GrantRefusal::Revoked,
        })
    );
    assert_eq!(crossed, Some(GrantRefusal::Revoked));
    // The running command is stopped and answers with its `Ran`.
    assert!(matches!(
        ran,
        Some(FromEnvironment::Ran {
            end: CommandEnd::Stopped,
            cleanup: Cleanup::Confirmed { forced: true },
            ..
        })
    ));
    // Each End is answered with what that end took.
    for (lease, cleanup) in [
        (LEASE, Cleanup::Confirmed { forced: false }),
        (COMMAND, Cleanup::Confirmed { forced: true }),
        (UNRUN, Cleanup::NotHeld),
    ] {
        gateway
            .send(ToEnvironment::End {
                lease: lease.into(),
            })
            .await;
        assert_eq!(
            gateway.said().await,
            FromEnvironment::Ended {
                lease: lease.into(),
                cleanup,
            }
        );
    }
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    let entries = gateway.ledger.entries();
    for (lease, cleanup) in [
        (LEASE, Cleanup::Confirmed { forced: false }),
        (COMMAND, Cleanup::Confirmed { forced: true }),
        (UNRUN, Cleanup::NotHeld),
    ] {
        let revoking = LedgerEntry::Revoking {
            lease: lease.into(),
            peer: DIALED,
        };
        let ended = LedgerEntry::Ended {
            lease: lease.into(),
            cleanup,
            cause: EndCause::Revoked,
        };
        assert_eq!(
            entries.iter().filter(|entry| **entry == revoking).count(),
            1,
            "{entries:?}"
        );
        let at = |wanted: &LedgerEntry| entries.iter().position(|entry| entry == wanted);
        assert!(at(&revoking) < at(&ended), "{entries:?}");
        assert_eq!(ended_entries(&gateway.ledger, lease), 1, "{entries:?}");
    }
    // Recorded as the revocation it crossed, never as the owner's grant.
    assert!(entries.contains(&LedgerEntry::Refused {
        lease: OTHER_LEASE.into(),
        peer: DIALED,
        reason: GrantRefusal::Revoked,
        asked: Asked::Agent {
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        },
    }));
}

#[tokio::test]
async fn a_revocation_that_cannot_be_recorded_still_stops_and_answers_uncertain() {
    let ledger = Arc::new(MemoryLedger::default());
    ledger.failing_revoking.store(true, Ordering::SeqCst);
    let mut gateway = revocable(ledger.clone());
    gateway.started(LEASE).await;
    gateway.send(ToEnvironment::Revoked).await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Uncertain,
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Uncertain,
            cause: EndCause::Revoked,
        })
    );
}

#[tokio::test]
async fn a_revocation_after_an_expiry_ends_the_lease_once_as_expired() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    gateway.started_expiring(LEASE).await;
    timer.advance(MAX_LEASE);
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Expired,
        }
    );
    gateway.send(ToEnvironment::Revoked).await;
    assert_eq!(
        gateway.ended(LEASE).await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(ended_entries(&gateway.ledger, LEASE), 1);
    assert!(!gateway
        .ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Revoking { .. })));
    assert_eq!(
        gateway.ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
            cause: EndCause::Expired,
        })
    );
}

/// A command that ended by itself, its end recorded, while its `Ran` still
/// waits for room on a gateway reading nothing, is not blamed on a
/// revocation that comes then: the host records no revoking of it, and its
/// end stays the one it recorded.
#[tokio::test]
async fn a_command_whose_end_is_recorded_is_not_revoked_while_its_ran_waits_for_room() {
    let timer = Arc::new(ManualTimer::default());
    let grants = MemoryGrants::none();
    grants.set(DIALED, Some(PeerGrant::everything()));
    let ledger = Arc::new(MemoryLedger::default());
    let mut gateway = Gateway::through(
        4096,
        DIALED,
        grants,
        ledger.clone(),
        Arc::new(NoOutbox),
        None,
        true,
        timer.clone(),
    );
    gateway.started(LEASE).await;
    gateway
        .send(grant_command(COMMAND, &["echo", "done"], None))
        .await;
    gateway.next().await;
    gateway.settled().await;
    // The gateway reads nothing from here, until the harness, unread,
    // overflows its input: every place a `Ran` could go is full.
    let mut sent = 0;
    while !ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::InputOverflow { lease, .. } if lease == LEASE))
    {
        assert!(sent < 5000, "the harness never stopped reading its input");
        gateway
            .send(ToEnvironment::Input {
                lease: LEASE.into(),
                channel: 1,
                data: Data(vec![b'x'; 4000]),
            })
            .await;
        sent += 1;
        tokio::task::yield_now().await;
    }
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    let ended = LedgerEntry::Ended {
        lease: COMMAND.into(),
        cleanup: Cleanup::Confirmed { forced: false },
        cause: EndCause::Asked,
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ledger.entries().contains(&ended) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the command ends by itself");
    gateway.send(ToEnvironment::Revoked).await;
    // The revocation is handled at once, every lease in one go: the
    // harness lease's revoking is recorded once it was.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ledger
            .entries()
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::Revoking { lease, .. } if lease == LEASE))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the revocation is handled");
    let entries = ledger.entries();
    assert!(
        !entries.iter().any(|entry| matches!(
            entry,
            LedgerEntry::Revoking { lease, .. } if lease == COMMAND
        )),
        "{entries:?}"
    );
    let ends: Vec<_> = entries
        .iter()
        .filter(|entry| matches!(entry, LedgerEntry::Ended { lease, .. } if lease == COMMAND))
        .collect();
    assert_eq!(ends, vec![&ended], "{entries:?}");
    // Nothing is left running: the gateway goes, and serving ends.
    let Gateway {
        writer,
        frames: _unread,
        mut served,
        ..
    } = gateway;
    drop(writer);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut served => break,
                () = tokio::time::sleep(Duration::from_millis(10)) => {
                    timer.advance(Duration::from_secs(1));
                }
            }
        }
    })
    .await
    .expect("serving ends once the gateway is gone");
}

/// A gateway that goes away half-open, reading nothing, while a command it
/// had revoked still waits for room to say its `Ran`: serving still ends,
/// the command's end recorded, rather than waiting on the gateway for good.
#[tokio::test]
async fn a_command_stopped_for_a_revocation_never_holds_serving_open_once_the_gateway_is_gone() {
    let timer = Arc::new(ManualTimer::default());
    let grants = MemoryGrants::none();
    grants.set(DIALED, Some(PeerGrant::everything()));
    let ledger = Arc::new(MemoryLedger::default());
    let mut gateway = Gateway::through(
        4096,
        DIALED,
        grants,
        ledger.clone(),
        Arc::new(NoOutbox),
        None,
        true,
        timer.clone(),
    );
    gateway.started(LEASE).await;
    gateway.send(grant_command(COMMAND, &["wait"], None)).await;
    gateway.next().await;
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    gateway.settled().await;
    // From here the gateway reads nothing: the harness's echoes fill what
    // the connection and the host's queue hold, until the harness, unread,
    // stops reading its input and overflows it. Only then is every place a
    // `Ran` could go full.
    let overflowed = |ledger: &MemoryLedger| {
        ledger.entries().iter().any(
            |entry| matches!(entry, LedgerEntry::InputOverflow { lease, .. } if lease == LEASE),
        )
    };
    let mut sent = 0;
    while !overflowed(&ledger) {
        assert!(sent < 5000, "the harness never stopped reading its input");
        gateway
            .send(ToEnvironment::Input {
                lease: LEASE.into(),
                channel: 1,
                data: Data(vec![b'x'; 4000]),
            })
            .await;
        sent += 1;
        tokio::task::yield_now().await;
    }
    gateway.send(ToEnvironment::Revoked).await;
    // The command is stopped and its end recorded; its `Ran` waits for room.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ledger
            .entries()
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::CommandRan { lease, .. } if lease == COMMAND))
        {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the command is stopped");
    // Half-open: the gateway's side stops sending, and its reading half
    // stays, unread.
    let Gateway {
        writer,
        frames: mut unread,
        mut served,
        ..
    } = gateway;
    drop(writer);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            tokio::select! {
                _ = &mut served => break,
                () = tokio::time::sleep(Duration::from_millis(10)) => {
                    timer.advance(Duration::from_secs(1));
                }
            }
        }
    })
    .await
    .expect("serving ends once the gateway is gone");
    assert!(ledger.entries().contains(&LedgerEntry::Ended {
        lease: COMMAND.into(),
        cleanup: Cleanup::Confirmed { forced: true },
        cause: EndCause::Revoked,
    }));
    // What did reach the gateway's unread half, up to a frame the host was
    // cut off writing, never held the `Ran`: it was still waiting for room
    // when the gateway went, as this test means.
    let mut delivered = Vec::new();
    while let Ok(Ok(Some(body))) = tokio::time::timeout(Duration::from_secs(5), unread.next()).await
    {
        delivered.push(decode::<FromEnvironment>(&body).unwrap());
    }
    assert!(
        !delivered
            .iter()
            .any(|frame| matches!(frame, FromEnvironment::Ran { lease, .. } if lease == COMMAND)),
        "the Ran reached the gateway before it went"
    );
}

// Row K1 on the host: it is stopping, and drains before it does.

/// A drain ends every lease held, each recorded `draining` before its end:
/// a harness's lease is said `Ending{drained}` and its `End` answered with
/// what that took, a running command is stopped and answers its `Ran` and
/// stays held until its `End`, one never run ends holding nothing, and a
/// grant crossing it is refused `draining`. Serving stays open until the
/// gateway has asked the `End` of every lease drained, the running command
/// included, then ends by itself.
#[tokio::test]
async fn k1_a_drain_ends_every_lease_recorded_then_serving_ends_once_answered() {
    let mut gateway = revocable(Arc::new(MemoryLedger::default()));
    gateway.started(LEASE).await;
    gateway.send(grant_command(COMMAND, &["wait"], None)).await;
    gateway.next().await;
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    gateway.send(grant_command(UNRUN, &["wait"], None)).await;
    gateway.next().await;
    gateway.settled().await;
    // Asked twice, acted on once.
    gateway.drain.send_replace(true);
    gateway.drain.send_replace(true);
    // Said first: the gateway asks for nothing more here.
    assert_eq!(gateway.said().await, FromEnvironment::Draining);
    let (mut ending, mut ran) = (None, None);
    while ending.is_none() || ran.is_none() {
        match gateway.said().await {
            frame @ FromEnvironment::Ending { .. } => ending = Some(frame),
            frame @ FromEnvironment::Ran { .. } => ran = Some(frame),
            other => panic!("unexpected {other:?}"),
        }
    }
    assert_eq!(
        ending,
        Some(FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Drained,
        })
    );
    assert!(matches!(
        ran,
        Some(FromEnvironment::Ran {
            end: CommandEnd::Drained,
            cleanup: Cleanup::Confirmed { forced: true },
            ..
        })
    ));
    // Its `Ran` leaves it held. Serving waits for its `End`, which closes
    // the publish point the gateway asked for.
    assert_eq!(
        gateway.ended(COMMAND).await,
        FromEnvironment::Ended {
            lease: COMMAND.into(),
            cleanup: Cleanup::Confirmed { forced: true },
        }
    );
    // Asked once the drain is said, a grant crosses it.
    gateway
        .send(ToEnvironment::Grant {
            lease: OTHER_LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    const CROSSED: &str = "0b7d3a1e-0000-4000-8000-0000000000c4";
    gateway.send(grant_command(CROSSED, &["echo"], None)).await;
    for lease in [OTHER_LEASE, CROSSED] {
        assert_eq!(
            gateway.said().await,
            FromEnvironment::Refused {
                lease: lease.into(),
                reason: GrantRefusal::Draining,
            }
        );
    }
    // The command never run is ended holding nothing, and its End says so.
    gateway
        .send(ToEnvironment::End {
            lease: UNRUN.into(),
        })
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ended {
            lease: UNRUN.into(),
            cleanup: Cleanup::NotHeld,
        }
    );
    // The harness's lease still waits for its End: serving is open.
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!gateway.served.is_finished());
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    // Every End answered: serving ends, and the stream with it.
    tokio::time::timeout(Duration::from_secs(5), &mut gateway.served)
        .await
        .expect("a drained host stops serving")
        .unwrap();
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    let entries = gateway.ledger.entries();
    for (lease, cleanup) in [
        (LEASE, Cleanup::Confirmed { forced: false }),
        (COMMAND, Cleanup::Confirmed { forced: true }),
        (UNRUN, Cleanup::NotHeld),
    ] {
        let draining = LedgerEntry::Draining {
            lease: lease.into(),
            peer: DIALED,
        };
        let ended = LedgerEntry::Ended {
            lease: lease.into(),
            cleanup,
            cause: EndCause::Drained,
        };
        let at = |wanted: &LedgerEntry| entries.iter().position(|entry| entry == wanted);
        assert_eq!(
            entries.iter().filter(|entry| **entry == draining).count(),
            1,
            "{entries:?}"
        );
        assert!(at(&draining) < at(&ended), "{entries:?}");
        assert_eq!(ended_entries(&gateway.ledger, lease), 1, "{entries:?}");
    }
    // Recorded as the drain it crossed, never as the owner's grant.
    assert!(entries.contains(&LedgerEntry::Refused {
        lease: OTHER_LEASE.into(),
        peer: DIALED,
        reason: GrantRefusal::Draining,
        asked: Asked::Agent {
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        },
    }));
}

/// A grant ready to read as the drain is asked is refused, never admitted:
/// the drain wins the race with a frame already waiting.
#[tokio::test]
async fn k1_a_grant_waiting_as_the_drain_is_asked_is_refused() {
    let mut gateway = revocable(Arc::new(MemoryLedger::default()));
    gateway.started(LEASE).await;
    gateway.drain.send_replace(true);
    gateway
        .send(ToEnvironment::Grant {
            lease: OTHER_LEASE.into(),
            agent: "claude".into(),
            sandbox: Sandbox::HarnessDefault,
        })
        .await;
    let mut refused = None;
    while refused.is_none() {
        match gateway.said().await {
            FromEnvironment::Draining | FromEnvironment::Ending { .. } => {}
            frame @ FromEnvironment::Refused { .. } => refused = Some(frame),
            other => panic!("admitted after the drain: {other:?}"),
        }
    }
    assert_eq!(
        refused,
        Some(FromEnvironment::Refused {
            lease: OTHER_LEASE.into(),
            reason: GrantRefusal::Draining,
        })
    );
}

/// A host holding nothing when asked to drain has nothing to wait for: it
/// stops serving at once, recording nothing.
#[tokio::test]
async fn k1_an_idle_host_asked_to_drain_stops_serving_at_once() {
    let mut gateway = revocable(Arc::new(MemoryLedger::default()));
    assert_eq!(gateway.next().await, hello());
    gateway.drain.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), &mut gateway.served)
        .await
        .expect("an idle host stops serving when drained")
        .unwrap();
    assert!(gateway.ledger.entries().is_empty());
}

/// A drain whose record cannot be kept still stops everything, and the
/// lease's end then settles nothing: it answers uncertain.
#[tokio::test]
async fn k1_a_drain_that_cannot_be_recorded_still_stops_and_answers_uncertain() {
    let ledger = Arc::new(MemoryLedger::default());
    ledger.failing_draining.store(true, Ordering::SeqCst);
    let mut gateway = revocable(ledger.clone());
    gateway.started(LEASE).await;
    gateway.drain.send_replace(true);
    // Said first: the gateway asks for nothing more here.
    assert_eq!(gateway.said().await, FromEnvironment::Draining);
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Drained,
        }
    );
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Uncertain,
        }
    );
    assert_eq!(gateway.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Uncertain,
            cause: EndCause::Drained,
        })
    );
}

/// A gateway's revocation arriving while the host drains ends nothing twice:
/// each lease keeps the drain as its cause.
#[tokio::test]
async fn k1_a_revocation_after_a_drain_ends_the_lease_once_as_drained() {
    let mut gateway = revocable(Arc::new(MemoryLedger::default()));
    gateway.started(LEASE).await;
    gateway.drain.send_replace(true);
    // Said first: the gateway asks for nothing more here.
    assert_eq!(gateway.said().await, FromEnvironment::Draining);
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Drained,
        }
    );
    gateway.send(ToEnvironment::Revoked).await;
    gateway
        .send(ToEnvironment::End {
            lease: LEASE.into(),
        })
        .await;
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    assert_eq!(ended_entries(&gateway.ledger, LEASE), 1);
    assert!(!gateway
        .ledger
        .entries()
        .iter()
        .any(|entry| matches!(entry, LedgerEntry::Revoking { .. })));
}

/// Row C21: a drain that leaves only a running command keeps serving open
/// after that command's `Ran`, until the gateway's `End`. Closing on the
/// answer would shut the publish point as a loss while that `End` was still
/// coming.
#[tokio::test]
async fn k1_a_drain_waits_for_the_end_of_a_command_that_already_answered() {
    let mut gateway = revocable(Arc::new(MemoryLedger::default()));
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(COMMAND, &["wait"], None)).await;
    gateway.next().await;
    gateway
        .send(ToEnvironment::Run {
            lease: COMMAND.into(),
        })
        .await;
    gateway.settled().await;
    gateway.drain.send_replace(true);
    // Said first: the gateway asks for nothing more here.
    assert_eq!(gateway.said().await, FromEnvironment::Draining);
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ran {
            end: CommandEnd::Drained,
            ..
        }
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!gateway.served.is_finished());
    assert_eq!(
        gateway.ended(COMMAND).await,
        FromEnvironment::Ended {
            lease: COMMAND.into(),
            cleanup: Cleanup::Confirmed { forced: true },
        }
    );
    tokio::time::timeout(Duration::from_secs(5), &mut gateway.served)
        .await
        .expect("serving ends once the command's End is answered")
        .unwrap();
    assert!(gateway.ledger.entries().contains(&LedgerEntry::Ended {
        lease: COMMAND.into(),
        cleanup: Cleanup::Confirmed { forced: true },
        cause: EndCause::Drained,
    }));
}

/// A command granted and never run, ended by a drain, keeps serving open
/// until the gateway's `Run` of it, which is answered never started: the
/// gateway hears its end rather than finding it lost with the connection.
#[tokio::test]
async fn k1_a_drain_waits_for_the_run_of_a_command_it_ended_unrun() {
    let mut gateway = revocable(Arc::new(MemoryLedger::default()));
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(UNRUN, &["wait"], None)).await;
    gateway.next().await;
    gateway.settled().await;
    gateway.drain.send_replace(true);
    // Said first: the gateway asks for nothing more here.
    assert_eq!(gateway.said().await, FromEnvironment::Draining);
    // Recorded ended, but the gateway has not heard: serving is open.
    let ended = LedgerEntry::Ended {
        lease: UNRUN.into(),
        cleanup: Cleanup::NotHeld,
        cause: EndCause::Drained,
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !gateway.ledger.entries().contains(&ended) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the drain ends the unrun command");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!gateway.served.is_finished());
    gateway
        .send(ToEnvironment::Run {
            lease: UNRUN.into(),
        })
        .await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ran {
            end: CommandEnd::NotStarted,
            cleanup: Cleanup::NotHeld,
            ..
        }
    ));
    tokio::time::timeout(Duration::from_secs(5), &mut gateway.served)
        .await
        .expect("serving ends once the unrun command's end is said")
        .unwrap();
    assert_eq!(ended_entries(&gateway.ledger, UNRUN), 1);
}

/// A command ended unrun by a drain that could not be recorded is said
/// uncertain at its `Run`, never confirmed-looking `not_held`.
#[tokio::test]
async fn k1_an_unrecorded_drain_of_an_unrun_command_answers_its_run_uncertain() {
    let ledger = Arc::new(MemoryLedger::default());
    ledger.failing_draining.store(true, Ordering::SeqCst);
    let mut gateway = revocable(ledger.clone());
    assert_eq!(gateway.next().await, hello());
    gateway.send(grant_command(UNRUN, &["wait"], None)).await;
    gateway.next().await;
    gateway.settled().await;
    gateway.drain.send_replace(true);
    // Said first: the gateway asks for nothing more here.
    assert_eq!(gateway.said().await, FromEnvironment::Draining);
    // Its `Run` is sent once the drain has ended it, not before.
    let ended = LedgerEntry::Ended {
        lease: UNRUN.into(),
        cleanup: Cleanup::Uncertain,
        cause: EndCause::Drained,
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !ledger.entries().contains(&ended) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the drain ends the unrun command");
    gateway
        .send(ToEnvironment::Run {
            lease: UNRUN.into(),
        })
        .await;
    assert!(matches!(
        gateway.said().await,
        FromEnvironment::Ran {
            end: CommandEnd::NotStarted,
            cleanup: Cleanup::Uncertain,
            ..
        }
    ));
    tokio::time::timeout(Duration::from_secs(5), &mut gateway.served)
        .await
        .expect("serving ends once the unrun command's end is said")
        .unwrap();
}

/// A lease that expired before the drain still waits for its `End`: the
/// drain does not close the connection on an end the gateway has not heard.
#[tokio::test]
async fn k1_a_drain_waits_for_the_end_of_a_lease_expired_before_it() {
    let timer = Arc::new(ManualTimer::default());
    let mut gateway = expiring(timer.clone(), Arc::new(MemoryLedger::default()));
    gateway.started_expiring(LEASE).await;
    timer.advance(MAX_LEASE);
    assert_eq!(
        gateway.said().await,
        FromEnvironment::Ending {
            lease: LEASE.into(),
            cause: EndingCause::Expired,
        }
    );
    gateway.drain.send_replace(true);
    // Said first: the gateway asks for nothing more here.
    assert_eq!(gateway.said().await, FromEnvironment::Draining);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(!gateway.served.is_finished());
    assert_eq!(
        gateway.ended(LEASE).await,
        FromEnvironment::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
        }
    );
    tokio::time::timeout(Duration::from_secs(5), &mut gateway.served)
        .await
        .expect("serving ends once the expired lease's End is answered")
        .unwrap();
}
