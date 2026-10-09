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
struct EchoLauncher {
    stopped: Arc<AtomicUsize>,
    launched: Mutex<Vec<BTreeMap<String, String>>>,
}

struct EchoControl {
    stopped: Arc<AtomicUsize>,
}

impl HarnessControl for EchoControl {
    fn cleanup(&mut self, _grace: Duration, _kill: Duration) -> HarnessCleanupFuture<'_> {
        self.stopped.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(CloseOutcome { forced: false }) })
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
        tokio::spawn(async move {
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
        let (gateway, environment) = duplex(1 << 20);
        let (environment_in, environment_out) = split(environment);
        let stopped = Arc::new(AtomicUsize::new(0));
        let launcher = Arc::new(EchoLauncher {
            stopped: stopped.clone(),
            launched: Mutex::new(Vec::new()),
        });
        let served = tokio::spawn(serve(
            environment_in,
            environment_out,
            "1.2.3",
            launcher.clone(),
            ledger.clone(),
            ServeTimings {
                grace: Duration::from_millis(10),
                kill: Duration::from_millis(100),
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
async fn a_host_that_cannot_serve_says_its_build_then_why() {
    let (gateway, environment) = duplex(4096);
    refuse(
        environment,
        Hello {
            build: "1.2.3".into(),
            workspace: String::new(),
        },
        Unavailability::Busy,
    )
    .await
    .unwrap();
    let mut frames = FrameStream::new(gateway);
    let hello = read_hello(&frames.next().await.unwrap().unwrap()).unwrap();
    assert_eq!(hello.build, "1.2.3");
    assert_eq!(
        decode::<FromEnvironment>(&frames.next().await.unwrap().unwrap()).unwrap(),
        FromEnvironment::Unavailable {
            reason: Unavailability::Busy
        }
    );
    assert_eq!(frames.next().await.unwrap(), None);
}

use nessa_protocol::lease::read_hello;
