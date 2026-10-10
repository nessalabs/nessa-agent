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
        agent == "claude"
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
                LedgerEntry::Granted { lease: id, .. } if id == lease => {
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
}

impl Gateway {
    fn start() -> Self {
        Self::with(Arc::new(MemoryLedger::default()))
    }
    fn with(ledger: Arc<MemoryLedger>) -> Self {
        Self::over(ledger, Arc::new(NoOutbox))
    }
    fn over(ledger: Arc<MemoryLedger>, outbox: Arc<dyn ArtifactOutbox>) -> Self {
        let (gateway, environment) = duplex(1 << 20);
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
        let served = tokio::spawn(serve(
            environment_in,
            environment_out,
            "1.2.3",
            "protocol",
            launcher.clone(),
            ledger.clone(),
            outbox,
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
    ) -> Result<nessa_protocol::lease::StagedArtifact, crate::env_serve::application::PublishRefusal>
    {
        Err(crate::env_serve::application::PublishRefusal::StagingFailed)
    }
    fn discard(&self, _lease: &str, _artifact: u32) {}
    fn close(&self, _lease: &str) {}
}

/// A host that stages every file as the same copy, and keeps the sending
/// end of each publish point it opened for the test to publish through.
#[derive(Default)]
struct RecordingOutbox {
    points: Mutex<HashMap<String, mpsc::Sender<crate::env_serve::application::PublishCall>>>,
    discarded: Mutex<Vec<(String, u32)>>,
    closed: Mutex<Vec<String>>,
    /// When set, the next staging says it began, then waits to be let go.
    stall: Mutex<Option<(std::sync::mpsc::Sender<()>, std::sync::mpsc::Receiver<()>)>>,
}

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
        _request: &crate::env_serve::application::PublishRequest,
    ) -> Result<nessa_protocol::lease::StagedArtifact, crate::env_serve::application::PublishRefusal>
    {
        let stall = self.stall.lock().unwrap().take();
        if let Some((began, go)) = stall {
            began.send(()).unwrap();
            let _ = go.recv();
        }
        Ok(Self::staged(artifact))
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
        let gateway = Self::over(Arc::new(MemoryLedger::default()), outbox.clone());
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
            reason: crate::env_serve::application::PublishRefusal::LeaseEnded,
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
            reason: crate::env_serve::application::PublishRefusal::Busy,
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
