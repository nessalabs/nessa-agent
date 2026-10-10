//! The environment role's frame loop, driven in process over a pipe: what it
//! answers, what it records, and what it does when the gateway goes away.
use super::*;
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
        agent == "claude"
    }
    fn launch(
        &self,
        _agent: &str,
        environment: &BTreeMap<String, String>,
    ) -> Result<HarnessProcess, AgentError> {
        self.launched.lock().unwrap().push(environment.clone());
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
/// run until they are stopped (anything else), and remember what ran.
#[derive(Default)]
struct FakeCommands {
    ran: Mutex<Vec<CommandWork>>,
}

impl CommandRunner for FakeCommands {
    fn run(
        &self,
        command: CommandWork,
        mut stop: watch::Receiver<Option<CommandStop>>,
    ) -> Pin<Box<dyn Future<Output = CommandRan> + Send + 'static>> {
        self.ran.lock().unwrap().push(command.clone());
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
}

impl MemoryLedger {
    fn entries(&self) -> Vec<LedgerEntry> {
        self.entries.lock().unwrap().clone()
    }
}

impl LeaseLedger for MemoryLedger {
    fn record(&self, entry: &LedgerEntry) -> io::Result<()> {
        if self.failing.load(Ordering::SeqCst) {
            return Err(io::Error::other("disk gone"));
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

struct Gateway {
    frames: FrameStream<ReadHalf<DuplexStream>>,
    writer: WriteHalf<DuplexStream>,
    served: JoinHandle<()>,
    ledger: Arc<MemoryLedger>,
    stopped: Arc<AtomicUsize>,
    launcher: Arc<EchoLauncher>,
    commands: Arc<FakeCommands>,
}

impl Gateway {
    fn start() -> Self {
        Self::with(Arc::new(MemoryLedger::default()))
    }
    fn with(ledger: Arc<MemoryLedger>) -> Self {
        Self::serving(ledger, true)
    }
    fn serving(ledger: Arc<MemoryLedger>, runs_commands: bool) -> Self {
        let (gateway, environment) = duplex(1 << 20);
        let (environment_in, environment_out) = split(environment);
        let stopped = Arc::new(AtomicUsize::new(0));
        let launcher = Arc::new(EchoLauncher {
            stopped: stopped.clone(),
            launched: Mutex::new(Vec::new()),
            slow: Mutex::new(Duration::ZERO),
            deaf: AtomicBool::new(false),
            leisurely: AtomicBool::new(false),
        });
        let commands = Arc::new(FakeCommands::default());
        let runner = runs_commands.then(|| commands.clone() as Arc<dyn CommandRunner>);
        let served = tokio::spawn(serve(
            environment_in,
            environment_out,
            "1.2.3",
            "protocol",
            Runners {
                harnesses: launcher.clone(),
                commands: runner,
            },
            ledger.clone(),
            ServeTimings {
                grace: Duration::from_millis(10),
                kill: Duration::from_millis(100),
                ..ServeTimings::default()
            },
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
        }
    }
    async fn send(&mut self, frame: ToEnvironment) {
        write_frame(&mut self.writer, &frame).await.unwrap();
    }
    async fn next(&mut self) -> FromEnvironment {
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
        })
        .await;
        assert_eq!(
            self.next().await,
            FromEnvironment::Granted {
                lease: lease.into()
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
                agent: "claude".into(),
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
                lost: false,
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
            lost: true,
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
            agent: "claude".into(),
        },
        LedgerEntry::Ended {
            lease: "ended".into(),
            cleanup: Cleanup::Confirmed { forced: true },
            lost: true,
        },
        LedgerEntry::Granted {
            lease: "unfinished".into(),
            agent: "claude".into(),
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
#[tokio::test(start_paused = true)]
async fn a_silent_gateway_is_lost_and_serving_ends() {
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
    // Its stream stays open; nothing more arrives on it.
    let Gateway {
        writer: _writer,
        frames: _frames,
        served,
        ledger,
        stopped,
        ..
    } = gateway;
    tokio::time::timeout(Duration::from_secs(120), served)
        .await
        .expect("serving ends once the gateway is silent past its bound")
        .unwrap();
    assert_eq!(stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        ledger.entries().last(),
        Some(&LedgerEntry::Ended {
            lease: LEASE.into(),
            cleanup: Cleanup::Confirmed { forced: false },
            lost: true,
        })
    );
}

/// A gateway's keepalives are what keep a quiet connection served.
#[tokio::test(start_paused = true)]
async fn keepalives_keep_a_quiet_gateway_served() {
    let mut gateway = Gateway::start();
    assert_eq!(gateway.next().await, hello());
    gateway.granted(LEASE).await;
    for _ in 0..8 {
        tokio::time::sleep(nessa_protocol::lease::KEEPALIVE_INTERVAL).await;
        gateway.send(ToEnvironment::Keepalive).await;
    }
    assert!(!gateway.served.is_finished());
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
            lease: COMMAND.into()
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
    let entries = gateway.ledger.entries();
    assert_eq!(
        entries[..4],
        [
            LedgerEntry::CommandGranted {
                lease: COMMAND.into(),
                argv: vec!["echo".into(), "a".into(), "b".into()],
                cwd: Some("sub".into()),
                timeout_ms: 1_000,
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
                lost: false,
            },
        ]
    );
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
        reason: GrantRefusal::InvalidCommand,
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
        lost: true,
    }));
    assert!(entries.contains(&LedgerEntry::Ended {
        lease: OTHER.into(),
        cleanup: Cleanup::NotHeld,
        lost: true,
    }));
}
