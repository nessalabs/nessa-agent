//! The SSH environment against a substitute for `ssh`: each connection is a
//! pipe to the environment role's own frame loop, run in process, or to a
//! scripted host. What it refuses, what it routes, and what becomes of a
//! lease whose connection is lost.
use super::{
    audit::{EnvironmentAudit, EnvironmentEvent},
    connector::{ssh_arguments, LeaseConnection, LeaseConnector},
    environment::{SshEnvironment, SshTimings},
};
use crate::conversation::application::{CommandEnvironment, Environment, LeaseRelease};
use crate::env::{LEASE_PROTOCOL, VERSION};
use crate::env_serve::application::FrameStream;
use crate::env_serve::application::{
    serve, CommandRan, CommandRunner, CommandStop, HarnessLauncher, LeaseLedger, LedgerEntry,
    ServeTimings,
};
use nessa_protocol::lease::{
    decode, encode, Cleanup, CommandEnd, Data, FromEnvironment, ToEnvironment,
};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{
        AgentProvider, CloseOutcome, HarnessCleanupFuture, HarnessControl, HarnessHost,
        HarnessLaunch, HarnessProcess, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
    },
};
use nessa_sdk::domain::{
    agent_execution::leases::{
        CommandExit, CommandRefusal, CommandWork,
    },
    agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseGrants,
        LeaseId, LeaseRefusal, LeaseTerms, LeaseWork, SandboxProfile, SshDestination,
    },
    effective_capabilities::value_objects::EffectiveCapabilities,
};
use std::{
    collections::BTreeMap,
    io,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{duplex, split, AsyncReadExt, AsyncWriteExt, DuplexStream},
    task::JoinHandle,
};

// ---- the host side ----

/// Echoes its input, except `flood`, which it answers with a mebibyte, and
/// a read starting `deaf`, after which it reads nothing more until it is
/// stopped; counts its
/// stops. A harness that flooded or went deaf has to be forced to stop, so
/// its cleanup is told apart from one that answers nothing ran.
struct Echo {
    stopped: Arc<AtomicUsize>,
}
struct EchoControl {
    stopped: Arc<AtomicUsize>,
    flooded: Arc<AtomicBool>,
    /// Lets a deaf harness go once it is stopped.
    released: Arc<tokio::sync::Notify>,
}
impl HarnessControl for EchoControl {
    fn cleanup(&mut self, _grace: Duration, _kill: Duration) -> HarnessCleanupFuture<'_> {
        self.released.notify_one();
        self.stopped.fetch_add(1, Ordering::SeqCst);
        let forced = self.flooded.load(Ordering::SeqCst);
        Box::pin(async move { Ok(CloseOutcome { forced }) })
    }
}
impl HarnessLauncher for Echo {
    fn workspace(&self) -> &str {
        "/srv/work"
    }
    fn runs(&self, agent: &str) -> bool {
        agent == "claude"
    }
    fn launch(
        &self,
        _agent: &str,
        _environment: &BTreeMap<String, String>,
    ) -> Result<HarnessProcess, AgentError> {
        let (input, mut harness_in) = duplex(4096);
        let (mut harness_out, output) = duplex(4096);
        let flooded = Arc::new(AtomicBool::new(false));
        let flooding = flooded.clone();
        let released = Arc::new(tokio::sync::Notify::new());
        let deafened = released.clone();
        tokio::spawn(async move {
            let mut buffer = [0; 256];
            while let Ok(read) = harness_in.read(&mut buffer).await {
                if buffer[..read].starts_with(b"deaf") {
                    flooding.store(true, Ordering::SeqCst);
                    deafened.notified().await;
                    return;
                }
                if &buffer[..read] == b"flood" {
                    flooding.store(true, Ordering::SeqCst);
                    for _ in 0..1024 {
                        if harness_out.write_all(&[b'x'; 1024]).await.is_err() {
                            return;
                        }
                    }
                    continue;
                }
                if read == 0 || harness_out.write_all(&buffer[..read]).await.is_err() {
                    break;
                }
            }
        });
        Ok(HarnessProcess {
            input: Box::new(input),
            output: Box::new(output),
            control: Box::new(EchoControl {
                stopped: self.stopped.clone(),
                flooded,
                released,
            }),
        })
    }
}

#[derive(Default)]
struct Ledger(Mutex<Vec<LedgerEntry>>);
impl LeaseLedger for Ledger {
    fn record(&self, entry: &LedgerEntry) -> io::Result<()> {
        self.0.lock().unwrap().push(entry.clone());
        Ok(())
    }
    fn accounted(&self, lease: &str) -> io::Result<Cleanup> {
        let entries = self.0.lock().unwrap();
        Ok(entries
            .iter()
            .rev()
            .find_map(|entry| match entry {
                LedgerEntry::Ended {
                    lease: id, cleanup, ..
                } if id == lease => Some(*cleanup),
                _ => None,
            })
            .unwrap_or(Cleanup::NotHeld))
    }
}

/// What each new connection reaches.
#[derive(Clone)]
enum Reach {
    /// The environment role's frame loop, sharing one ledger across
    /// connections, as one host's data directory does.
    Serving,
    /// A host that writes these bodies and then holds the stream open,
    /// keeping what it was sent.
    Scripted(Vec<Vec<u8>>),
    /// A host that grants the first lease it is asked for, then stops
    /// reading until `gate` opens, and then keeps every frame it reads.
    Stalling(tokio::sync::watch::Receiver<bool>),
    /// `ssh` cannot be started.
    Unreachable,
}

struct Connector {
    reach: Mutex<Reach>,
    launcher: Arc<dyn HarnessLauncher>,
    ledger: Arc<Ledger>,
    stopped: Arc<AtomicUsize>,
    connects: AtomicUsize,
    /// The relays of each connection, to cut it.
    relays: Mutex<Vec<(JoinHandle<()>, JoinHandle<()>)>>,
    /// What scripted hosts were sent.
    received: Arc<Mutex<Vec<u8>>>,
    /// What a stalling host read once its gate opened.
    frames: Arc<Mutex<Vec<ToEnvironment>>>,
    /// How a serving host runs commands; `None` runs none.
    commands: Mutex<Option<Arc<dyn CommandRunner>>>,
}

impl Connector {
    fn new(reach: Reach) -> Arc<Self> {
        let stopped = Arc::new(AtomicUsize::new(0));
        Self::with(
            reach,
            Arc::new(Echo {
                stopped: stopped.clone(),
            }),
            stopped,
        )
    }
    fn with(
        reach: Reach,
        launcher: Arc<dyn HarnessLauncher>,
        stopped: Arc<AtomicUsize>,
    ) -> Arc<Self> {
        Arc::new(Self {
            reach: Mutex::new(reach),
            launcher,
            ledger: Arc::new(Ledger::default()),
            stopped,
            connects: AtomicUsize::new(0),
            relays: Mutex::new(Vec::new()),
            received: Arc::new(Mutex::new(Vec::new())),
            frames: Arc::new(Mutex::new(Vec::new())),
            commands: Mutex::new(None),
        })
    }
    /// Cut every connection both ways, as a network that went away.
    fn cut(&self) {
        for (one, other) in self.relays.lock().unwrap().drain(..) {
            one.abort();
            other.abort();
        }
    }
}

fn relay(
    mut from: tokio::io::ReadHalf<DuplexStream>,
    mut to: tokio::io::WriteHalf<DuplexStream>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let _ = tokio::io::copy(&mut from, &mut to).await;
        let _ = to.shutdown().await;
    })
}

impl LeaseConnector for Connector {
    fn connect(&self, _host: &SshDestination) -> io::Result<LeaseConnection> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        let reach = self.reach.lock().unwrap().clone();
        let (gateway, gateway_end) = duplex(1 << 20);
        let (host_end, host) = duplex(1 << 20);
        match reach {
            Reach::Unreachable => return Err(io::Error::other("no route to host")),
            Reach::Serving => {
                let (host_in, host_out) = split(host);
                tokio::spawn(serve(
                    host_in,
                    host_out,
                    VERSION,
                    LEASE_PROTOCOL,
                    self.launcher.clone(),
                    self.commands.lock().unwrap().clone(),
                    self.ledger.clone(),
                    ServeTimings {
                        grace: Duration::from_millis(10),
                        kill: Duration::from_millis(100),
                        ..ServeTimings::default()
                    },
                ));
            }
            Reach::Stalling(mut gate) => {
                let frames = self.frames.clone();
                tokio::spawn(async move {
                    let (host_in, mut host_out) = split(host);
                    let mut stream = FrameStream::new(host_in);
                    host_out
                        .write_all(
                            &encode(&FromEnvironment::Hello {
                                build: VERSION.into(),
                                protocol: LEASE_PROTOCOL.into(),
                                workspace: "/srv/work".into(),
                            })
                            .unwrap(),
                        )
                        .await
                        .unwrap();
                    while let Ok(Some(body)) = stream.next().await {
                        if let Ok(ToEnvironment::Grant { lease, .. }) = decode(&body) {
                            let granted = encode(&FromEnvironment::Granted { lease }).unwrap();
                            host_out.write_all(&granted).await.unwrap();
                            break;
                        }
                    }
                    let _ = gate.wait_for(|open| *open).await;
                    // Slowly, so the gateway's queue frees one place at a time.
                    while let Ok(Some(body)) = stream.next().await {
                        if let Ok(frame) = decode::<ToEnvironment>(&body) {
                            frames.lock().unwrap().push(frame);
                        }
                        tokio::time::sleep(Duration::from_millis(1)).await;
                    }
                    drop(host_out);
                });
            }
            Reach::Scripted(bodies) => {
                let received = self.received.clone();
                tokio::spawn(async move {
                    let (mut host_in, mut host_out) = split(host);
                    for body in bodies {
                        let framed = nessa_protocol::pairing::encode_frame(1 << 20, &body).unwrap();
                        host_out.write_all(&framed).await.unwrap();
                    }
                    let mut buffer = [0; 1024];
                    while let Ok(read) = host_in.read(&mut buffer).await {
                        if read == 0 {
                            break;
                        }
                        received.lock().unwrap().extend_from_slice(&buffer[..read]);
                    }
                });
            }
        }
        let (gateway_end_in, gateway_end_out) = split(gateway_end);
        let (host_end_in, host_end_out) = split(host_end);
        self.relays.lock().unwrap().push((
            relay(gateway_end_in, host_end_out),
            relay(host_end_in, gateway_end_out),
        ));
        let (from_environment, to_environment) = split(gateway);
        Ok(LeaseConnection {
            from_environment: Box::new(from_environment),
            to_environment: Box::new(to_environment),
            keep: Box::new(()),
        })
    }
}

#[derive(Default)]
struct Audit(Mutex<Vec<EnvironmentEvent>>, AtomicBool);
impl EnvironmentAudit for Audit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
        if self.1.load(Ordering::SeqCst) {
            return Err(io::Error::other("audit disk full"));
        }
        self.0.lock().unwrap().push(event.clone());
        Ok(())
    }
}
impl Audit {
    fn events(&self) -> Vec<EnvironmentEvent> {
        self.0.lock().unwrap().clone()
    }
}

// ---- the gateway side ----

/// A binding that hands back itself on any host, remembering the host.
struct Binding {
    capabilities: EffectiveCapabilities,
    host: Arc<Mutex<Option<Arc<dyn HarnessHost>>>>,
    remote: bool,
}
impl AgentProvider for Binding {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("fixture", "model", "context").expect("a fixed identity is valid")
    }
    fn capabilities(&self) -> &EffectiveCapabilities {
        &self.capabilities
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async {
            Err(
                nessa_sdk::application::agent_execution::providers::ProviderOpenError::no_resources(
                    AgentError::Closed,
                ),
            )
        })
    }
    fn on_host(&self, host: Arc<dyn HarnessHost>) -> Result<Arc<dyn AgentProvider>, AgentError> {
        if !self.remote {
            return Err(AgentError::Unsupported("local only".into()));
        }
        *self.host.lock().unwrap() = Some(host);
        Ok(Arc::new(Binding {
            capabilities: self.capabilities.clone(),
            host: self.host.clone(),
            remote: true,
        }))
    }
}

/// The host a binding was last given, once it was.
type GivenHost = Arc<Mutex<Option<Arc<dyn HarnessHost>>>>;

fn binding(remote: bool) -> (Arc<Binding>, GivenHost) {
    let host = Arc::new(Mutex::new(None));
    let capabilities = crate::conversation_test_support::Provider::new(Arc::new(
        crate::conversation_test_support::ProviderFactory::default(),
    ))
    .capabilities()
    .clone();
    (
        Arc::new(Binding {
            capabilities,
            host: host.clone(),
            remote,
        }),
        host,
    )
}

fn devbox() -> SshDestination {
    SshDestination::new("devbox").unwrap()
}

fn environment(connector: Arc<Connector>, audit: Arc<Audit>) -> SshEnvironment {
    SshEnvironment::new(
        devbox(),
        connector,
        audit,
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_secs(5),
            ..SshTimings::default()
        },
    )
}

fn terms(agent: &str) -> LeaseTerms {
    LeaseTerms {
        environment: EnvironmentRef::Ssh(devbox()),
        work: LeaseWork::Agent(AgentWork::new(agent, "model").unwrap()),
        sandbox: SandboxProfile::HarnessDefault,
        grants: LeaseGrants::Opening,
        deadline: LeaseDeadline::UntilEnded,
    }
}

fn lease() -> LeaseId {
    LeaseId::new(uuid::Uuid::new_v4().to_string()).unwrap()
}

async fn read_some(output: &mut (dyn tokio::io::AsyncRead + Send + Unpin)) -> Vec<u8> {
    let mut buffer = [0; 64];
    let read = tokio::time::timeout(Duration::from_secs(5), output.read(&mut buffer))
        .await
        .expect("output in time")
        .unwrap();
    buffer[..read].to_vec()
}

fn hello_body(protocol: &str) -> Vec<u8> {
    serde_json::to_vec(&FromEnvironment::Hello {
        build: VERSION.into(),
        protocol: protocol.into(),
        workspace: "/srv/work".into(),
    })
    .unwrap()
}

/// Gate 1: a lease is granted by the host; the binding is given the host,
/// its harness runs there with its bytes carried both ways, and both the
/// harness's stop and the lease's end are answered with the host's evidence.
#[tokio::test]
async fn a_lease_runs_its_harness_on_the_host_and_ends_with_the_hosts_evidence() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    assert_eq!(
        environment.declaration().environment,
        EnvironmentRef::Ssh(devbox())
    );
    let (binding, host) = binding(true);
    let id = lease();
    let opened = environment
        .open(&id, &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host
        .lock()
        .unwrap()
        .clone()
        .expect("the binding was given the host");
    assert_eq!(host.workspace(), std::path::Path::new("/srv/work"));
    let HarnessProcess {
        mut input,
        mut output,
        mut control,
    } = host
        .start(HarnessLaunch {
            environment: BTreeMap::from([("ANTHROPIC_MODEL".into(), "m".into())]),
        })
        .unwrap();
    input.write_all(b"hello").await.unwrap();
    assert_eq!(read_some(output.as_mut()).await, b"hello");
    input.shutdown().await.unwrap();
    assert_eq!(
        control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await
            .unwrap(),
        CloseOutcome { forced: false }
    );
    assert_eq!(connector.stopped.load(Ordering::SeqCst), 1);
    assert_eq!(
        opened.hold.end(LeaseEndCause::Closed).await,
        LeaseRelease::Released(LeaseCleanup::Confirmed { forced: false })
    );
    assert!(matches!(
        audit.events()[0],
        EnvironmentEvent::Connected { .. }
    ));
    let entries = connector.ledger.0.lock().unwrap().clone();
    assert!(entries.contains(&LedgerEntry::Ended {
        lease: id.as_str().into(),
        cleanup: Cleanup::Confirmed { forced: false },
        lost: false,
    }));
    // The lease's end resolves its watch: nothing is left waiting on it.
    tokio::time::timeout(
        Duration::from_secs(5),
        opened.hold.lost().map_or_else(
            || {
                Box::pin(async {})
                    as crate::conversation::application::EnvironmentFuture<'static, ()>
            },
            |lost| lost,
        ),
    )
    .await
    .expect("an ended lease's watch resolves");
}

/// Gate 4: another lease protocol's hello is a typed refusal, recorded, and
/// nothing is sent to that host.
#[tokio::test]
async fn another_protocol_is_refused_and_sent_nothing() {
    let connector = Connector::new(Reach::Scripted(vec![hello_body("another")]));
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let (binding, host) = binding(true);
    let refused = environment.open(&lease(), &terms("claude"), binding).await;
    assert_eq!(
        refused.err(),
        Some(LeaseRefusal::EnvironmentVersionMismatch)
    );
    assert_eq!(
        audit.events(),
        [EnvironmentEvent::VersionRefused {
            host: "devbox".into(),
            build: Some(VERSION.into()),
            protocol: Some("another".into()),
        }]
    );
    assert!(host.lock().unwrap().is_none());
    tokio::task::yield_now().await;
    assert!(connector.received.lock().unwrap().is_empty());
    // A first frame that is no hello at all is another protocol too.
    let connector = Connector::new(Reach::Scripted(vec![b"{\"type\":\"granted\"}".to_vec()]));
    let environment = environment_with(connector, Arc::new(Audit::default()));
    assert_eq!(
        environment
            .open(&lease(), &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentVersionMismatch)
    );
}

/// The hello compares the lease protocol, not the package version: a host
/// of the same package version whose frames or their meaning differ is
/// another protocol, refused before anything is sent to it.
#[tokio::test]
async fn the_same_package_speaking_another_lease_protocol_is_refused() {
    let body = serde_json::json!({
        "type": "hello",
        "build": VERSION,
        "protocol": "another",
        "workspace": "/srv/work",
    });
    let connector = Connector::new(Reach::Scripted(vec![body.to_string().into_bytes()]));
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let refused = environment
        .open(&lease(), &terms("claude"), binding_only())
        .await;
    assert_eq!(
        refused.err(),
        Some(LeaseRefusal::EnvironmentVersionMismatch)
    );
    tokio::task::yield_now().await;
    assert!(connector.received.lock().unwrap().is_empty());
}

fn environment_with(connector: Arc<Connector>, audit: Arc<Audit>) -> SshEnvironment {
    environment(connector, audit)
}

fn binding_only() -> Arc<dyn AgentProvider> {
    binding(true).0
}

#[tokio::test]
async fn a_host_serving_another_gateway_is_busy() {
    let unavailable = serde_json::to_vec(&FromEnvironment::Unavailable {
        reason: nessa_protocol::lease::Unavailability::Busy,
    })
    .unwrap();
    let mut empty_hello = hello_body(LEASE_PROTOCOL);
    empty_hello = String::from_utf8(empty_hello)
        .unwrap()
        .replace("/srv/work", "")
        .into_bytes();
    let connector = Connector::new(Reach::Scripted(vec![empty_hello, unavailable]));
    let audit = Arc::new(Audit::default());
    let environment = environment(connector, audit.clone());
    assert_eq!(
        environment
            .open(&lease(), &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentBusy)
    );
    assert_eq!(
        audit.events(),
        [EnvironmentEvent::Busy {
            host: "devbox".into()
        }]
    );
}

#[tokio::test]
async fn an_agent_the_host_or_its_binding_cannot_run_is_refused() {
    let connector = Connector::new(Reach::Serving);
    let environment = environment(connector, Arc::new(Audit::default()));
    assert_eq!(
        environment
            .open(&lease(), &terms("codex"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::AgentUnavailable)
    );
    let (local_only, _) = binding(false);
    assert_eq!(
        environment
            .open(&lease(), &terms("claude"), local_only)
            .await
            .err(),
        Some(LeaseRefusal::AgentUnavailable)
    );
}

/// Gate 2 and row L10: the connection lost resolves the lease's watch, the
/// harness can no longer be stopped from here, and the lease's end asks a
/// new connection what the host recorded when it ended the lease as lost.
#[tokio::test]
async fn a_lost_connection_is_seen_and_the_end_is_accounted_on_a_new_one() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let (binding, host) = binding(true);
    let id = lease();
    let opened = environment
        .open(&id, &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let mut process = host.start(HarnessLaunch::default()).unwrap();
    process.input.write_all(b"x").await.unwrap();
    assert_eq!(read_some(process.output.as_mut()).await, b"x");
    let lost = opened.hold.lost().expect("a host's lease can be lost");
    connector.cut();
    tokio::time::timeout(Duration::from_secs(5), lost)
        .await
        .expect("the loss is seen");
    assert!(matches!(
        host.start(HarnessLaunch::default()),
        Err(AgentError::Closed)
    ));
    assert!(matches!(
        process
            .control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await,
        Err(AgentError::CleanupUncertain)
    ));
    // The host ended it as lost, stopping the harness, and recorded that.
    tokio::time::timeout(Duration::from_secs(5), async {
        while connector.stopped.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host stops what the lost lease ran");
    assert_eq!(
        opened.hold.end(LeaseEndCause::Lost).await,
        LeaseRelease::Released(LeaseCleanup::Confirmed { forced: false })
    );
    assert_eq!(connector.connects.load(Ordering::SeqCst), 2);
    assert!(audit.events().contains(&EnvironmentEvent::ConnectionLost {
        host: "devbox".into(),
        leases: vec![id.as_str().into()],
    }));
    assert!(connector
        .ledger
        .0
        .lock()
        .unwrap()
        .contains(&LedgerEntry::Ended {
            lease: id.as_str().into(),
            cleanup: Cleanup::Confirmed { forced: false },
            lost: true,
        }));
}

/// Gate 2: with the host unreachable after the loss, there is no evidence,
/// and the lease's end says so.
#[tokio::test]
async fn with_the_host_unreachable_a_lost_lease_has_no_evidence() {
    let connector = Connector::new(Reach::Serving);
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let id = lease();
    let opened = environment
        .open(&id, &terms("claude"), binding_only())
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let lost = opened.hold.lost().unwrap();
    *connector.reach.lock().unwrap() = Reach::Unreachable;
    connector.cut();
    lost.await;
    assert_eq!(
        opened.hold.end(LeaseEndCause::Lost).await,
        LeaseRelease::Unanswered
    );
    assert_eq!(environment.account(&id).await, None);
}

/// Gate 3 and row L9: a frame naming a lease or harness the gateway does not
/// hold, or one it cannot read, is dropped and recorded.
#[tokio::test]
async fn frames_naming_nothing_held_are_dropped_with_evidence() {
    let output = serde_json::to_vec(&FromEnvironment::Output {
        lease: "never-granted".into(),
        channel: 1,
        data: Data(b"late".to_vec()),
    })
    .unwrap();
    let granted = serde_json::to_vec(&FromEnvironment::Granted {
        lease: "never-asked".into(),
    })
    .unwrap();
    let connector = Connector::new(Reach::Scripted(vec![
        hello_body(LEASE_PROTOCOL),
        output,
        b"not json".to_vec(),
        granted,
    ]));
    let audit = Arc::new(Audit::default());
    let environment = environment(connector, audit.clone());
    // The grant is never answered by this host; the opening is bounded.
    let opening = tokio::spawn(async move {
        environment
            .open(&lease(), &terms("claude"), binding_only())
            .await
            .err()
    });
    let drops = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let drops: Vec<_> = audit
                .events()
                .into_iter()
                .filter_map(|event| match event {
                    EnvironmentEvent::FrameDropped { lease, frame, .. } => Some((lease, frame)),
                    _ => None,
                })
                .collect();
            if drops.len() == 3 {
                break drops;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("every drop recorded");
    assert_eq!(
        drops,
        [
            (Some("never-granted".into()), "output".into()),
            (None, "unreadable".into()),
            (Some("never-asked".into()), "granted".into()),
        ]
    );
    opening.abort();
}

/// A binding that stops reading its harness's output does not grow the
/// gateway's memory: past the queue the output ends, the overflow is
/// recorded, and the harness is stopped on the host.
#[tokio::test]
async fn output_a_binding_does_not_read_is_bounded_and_stops_the_harness() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let (binding, host) = binding(true);
    let id = lease();
    let _opened = environment
        .open(&id, &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let HarnessProcess {
        mut input,
        output: _unread,
        mut control,
    } = host.start(HarnessLaunch::default()).unwrap();
    // Far more output than the pipe and the queue hold, never read.
    input.write_all(b"flood").await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while connector.stopped.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the harness is stopped");
    assert!(audit.events().iter().any(
        |event| matches!(event, EnvironmentEvent::OutputOverflow { lease, channel, .. }
            if lease == id.as_str() && *channel == 1)
    ));
    // The binding's own stop, after the host already stopped it, is answered
    // with the host's evidence (forced, as the flood made it), never left
    // uncertain and never as a harness that held nothing.
    assert_eq!(
        control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await
            .unwrap(),
        CloseOutcome { forced: true }
    );
    assert_eq!(connector.stopped.load(Ordering::SeqCst), 1);
}

/// A harness that stops reading its input fails its channel rather than
/// taking a truncated stream: the host stops it and says so, the binding's
/// writes then fail and its output ends, and its own stop is answered with
/// the host's evidence of the forced stop.
#[tokio::test]
async fn input_a_harness_does_not_read_fails_its_channel() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let (binding, host) = binding(true);
    let id = lease();
    let _opened = environment
        .open(&id, &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let HarnessProcess {
        mut input,
        mut output,
        mut control,
    } = host.start(HarnessLaunch::default()).unwrap();
    input.write_all(b"deaf").await.unwrap();
    // Written until a write fails: far more than every queue on the way
    // holds, if none ever does.
    let failed = tokio::time::timeout(Duration::from_secs(20), async {
        for _ in 0..(64 * 1024) {
            if input.write_all(&[b'x'; 1024]).await.is_err() {
                return true;
            }
        }
        false
    })
    .await
    .unwrap_or(false);
    assert!(failed, "the binding's writes fail once the host stopped it");
    let mut rest = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), output.read_to_end(&mut rest))
        .await
        .expect("its output ends")
        .unwrap();
    let entries = connector.ledger.0.lock().unwrap().clone();
    assert!(
        entries.iter().any(|entry| matches!(
            entry,
            LedgerEntry::InputOverflow { lease, channel: 1 } if lease == id.as_str()
        )),
        "{entries:?} {:?}",
        audit.events()
    );
    assert_eq!(
        control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await
            .unwrap(),
        CloseOutcome { forced: true }
    );
    assert_eq!(connector.stopped.load(Ordering::SeqCst), 1);
}

/// A grant the host does not answer in time is refused here and ended there:
/// the host never keeps a lease nobody holds.
#[tokio::test]
async fn a_grant_not_answered_in_time_is_ended_on_the_host() {
    let connector = Connector::new(Reach::Scripted(vec![hello_body(LEASE_PROTOCOL)]));
    let environment = SshEnvironment::new(
        devbox(),
        connector.clone(),
        Arc::new(Audit::default()),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_millis(50),
            ..SshTimings::default()
        },
    );
    let id = lease();
    assert_eq!(
        environment
            .open(&id, &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentUnreachable)
    );
    let end = nessa_protocol::lease::encode(&nessa_protocol::lease::ToEnvironment::End {
        lease: id.as_str().into(),
    })
    .unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let received = connector.received.lock().unwrap().clone();
            if received.windows(end.len()).any(|window| window == end) {
                break;
            }
            drop(received);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the unanswered lease is ended on the host");
}

/// The destination comes after every option and the remote command is
/// fixed words: what `ssh` is run with. Which destinations are accepted is
/// `SshDestination`'s own test.
#[test]
fn ssh_is_run_with_the_destination_after_its_options() {
    let host = devbox();
    let arguments = ssh_arguments(&host);
    let separator = arguments.iter().position(|word| *word == "--").unwrap();
    assert_eq!(
        &arguments[separator + 1..],
        ["devbox", "nessa", "env", "serve"]
    );
    assert!(arguments.contains(&"BatchMode=yes"));
    assert!(arguments.contains(&"ForwardAgent=no"));
}

/// A launch whose variables do not fit in one frame is refused to its
/// binding before anything is queued; the connection other leases share
/// goes on carrying theirs.
#[tokio::test]
async fn a_launch_too_large_for_a_frame_is_refused_and_the_connection_goes_on() {
    let connector = Connector::new(Reach::Serving);
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let (binding, host) = binding(true);
    let _opened = environment
        .open(&lease(), &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let huge = "x".repeat(nessa_protocol::lease::MAX_FRAME_BYTES);
    assert!(matches!(
        host.start(HarnessLaunch {
            environment: BTreeMap::from([("CODEX_CONFIG".into(), huge.into())]),
        }),
        Err(AgentError::InvalidInput(_))
    ));
    let mut process = host.start(HarnessLaunch::default()).unwrap();
    process.input.write_all(b"still").await.unwrap();
    assert_eq!(read_some(process.output.as_mut()).await, b"still");
    assert_eq!(connector.connects.load(Ordering::SeqCst), 1);
}

/// Row L10: a connection lost before anything asks for the lease's watch is
/// still a lease that was lost. The watch is the lease's own from its grant,
/// so it resolves at once, never answering that the lease cannot be lost.
#[tokio::test]
async fn a_connection_lost_before_its_watch_is_asked_for_is_still_seen() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let opened = environment
        .open(&lease(), &terms("claude"), binding_only())
        .await
        .unwrap_or_else(|_| panic!("granted"));
    // Lost between the opening and the service asking for the watch.
    connector.cut();
    tokio::time::timeout(Duration::from_secs(5), async {
        while !audit
            .events()
            .iter()
            .any(|event| matches!(event, EnvironmentEvent::ConnectionLost { .. }))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the loss is recorded");
    let lost = opened
        .hold
        .lost()
        .expect("a lease on a host can be lost, and this one was");
    tokio::time::timeout(Duration::from_secs(5), lost)
        .await
        .expect("an earlier loss resolves the watch at once");
}

/// A harness's stop follows everything its binding wrote before it: the
/// last bytes reach the harness, its input is closed, and only then is it
/// stopped, so an ordinary stop drops nothing and records no drop.
#[tokio::test]
async fn a_stop_follows_the_last_input_and_drops_nothing() {
    let connector = Connector::new(Reach::Serving);
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let (binding, host) = binding(true);
    let _opened = environment
        .open(&lease(), &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let HarnessProcess {
        mut input,
        mut output,
        mut control,
    } = host.start(HarnessLaunch::default()).unwrap();
    // As the binding's process scope stops it: the last bytes, its input
    // dropped, then the cleanup.
    input.write_all(b"bye").await.unwrap();
    drop(input);
    assert_eq!(
        control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await
            .unwrap(),
        CloseOutcome { forced: false }
    );
    let mut echoed = Vec::new();
    tokio::time::timeout(Duration::from_secs(5), output.read_to_end(&mut echoed))
        .await
        .expect("the output ends")
        .unwrap();
    assert_eq!(echoed, b"bye", "the last bytes reached the harness");
    let drops: Vec<_> = connector
        .ledger
        .0
        .lock()
        .unwrap()
        .iter()
        .filter(|entry| matches!(entry, LedgerEntry::Dropped { .. }))
        .cloned()
        .collect();
    assert!(drops.is_empty(), "an ordinary stop dropped {drops:?}");
}

/// A harness's stop racing its lease's end is answered with the host's
/// evidence, whichever the host handles first: the lease's end stops every
/// harness under it, and says what that took.
#[tokio::test]
async fn a_stop_racing_the_lease_end_is_answered_with_the_hosts_evidence() {
    let connector = Connector::new(Reach::Serving);
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let (binding, host) = binding(true);
    let opened = environment
        .open(&lease(), &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let HarnessProcess {
        mut input,
        mut output,
        mut control,
    } = host.start(HarnessLaunch::default()).unwrap();
    input.write_all(b"x").await.unwrap();
    assert_eq!(read_some(output.as_mut()).await, b"x");
    let hold = opened.hold.clone();
    let ending = tokio::spawn(async move { hold.end(LeaseEndCause::Stopped).await });
    tokio::task::yield_now().await;
    let stopped = control
        .cleanup(Duration::from_millis(10), Duration::from_millis(100))
        .await;
    assert_eq!(stopped.unwrap(), CloseOutcome { forced: false });
    assert_eq!(
        ending.await.unwrap(),
        LeaseRelease::Released(LeaseCleanup::Confirmed { forced: false })
    );
    // Asked after the lease ended, the stop is answered from what the host
    // recorded of it, never left uncertain.
    let (second, host) = self::binding(true);
    let opened = environment
        .open(&lease(), &terms("claude"), second)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let mut process = host.start(HarnessLaunch::default()).unwrap();
    process.input.write_all(b"y").await.unwrap();
    assert_eq!(read_some(process.output.as_mut()).await, b"y");
    assert_eq!(
        opened.hold.end(LeaseEndCause::Stopped).await,
        LeaseRelease::Released(LeaseCleanup::Confirmed { forced: false })
    );
    assert_eq!(
        process
            .control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await
            .unwrap(),
        CloseOutcome { forced: false }
    );
}

/// A harness's stop and a lease's end are delivered even when the host's
/// queue is full when they are asked: a stop let go of, an end abandoned,
/// and an end whose caller stopped waiting before it was queued all reach
/// the host once it reads again, each after the input written before it.
#[tokio::test]
async fn a_stop_or_end_asked_while_the_queue_is_full_still_reaches_the_host() {
    let (open_gate, gate) = tokio::sync::watch::channel(false);
    let connector = Connector::new(Reach::Stalling(gate));
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let (binding, host) = binding(true);
    let id = lease();
    let opened = environment
        .open(&id, &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let HarnessProcess {
        mut input,
        output: _output,
        control,
    } = host.start(HarnessLaunch::default()).unwrap();
    // More input than the pipes and the host's queue hold.
    let written = Arc::new(AtomicUsize::new(0));
    let counting = written.clone();
    let writer = tokio::spawn(async move {
        let chunk = vec![b'i'; 64 * 1024];
        for _ in 0..512 {
            if input.write_all(&chunk).await.is_err() {
                return;
            }
            counting.fetch_add(chunk.len(), Ordering::SeqCst);
        }
    });
    let mut last = usize::MAX;
    loop {
        tokio::time::sleep(Duration::from_millis(200)).await;
        let now = written.load(Ordering::SeqCst);
        if now == last {
            break;
        }
        last = now;
    }
    // The harness let go of while its input waits for room: its Stop is
    // asked of its pump, which is blocked sending input.
    drop(control);
    // The lease's end, given up on before the queue had room for it.
    assert!(
        tokio::time::timeout(
            Duration::from_millis(50),
            opened.hold.end(LeaseEndCause::Closed)
        )
        .await
        .is_err(),
        "the full queue holds the end back"
    );
    drop(opened);
    open_gate.send_replace(true);
    let (input_after_stop, stop_before_end) =
        tokio::time::timeout(Duration::from_secs(30), async {
            loop {
                {
                    let frames = connector.frames.lock().unwrap();
                    let ended = frames.iter().any(
                    |frame| matches!(frame, ToEnvironment::End { lease } if lease == id.as_str()),
                );
                    let stop = frames
                        .iter()
                        .position(|frame| matches!(frame, ToEnvironment::Stop { .. }));
                    let end = frames.iter().position(
                    |frame| matches!(frame, ToEnvironment::End { lease } if lease == id.as_str()),
                );
                    if let (true, Some(stop), Some(end)) = (ended, stop, end) {
                        break (
                            frames[stop..]
                                .iter()
                                .any(|frame| matches!(frame, ToEnvironment::Input { .. })),
                            stop < end,
                        );
                    }
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .expect("the stop and the end reach the host");
    writer.abort();
    assert!(
        !input_after_stop,
        "nothing of the harness's input follows its stop"
    );
    assert!(
        stop_before_end,
        "the lease's End follows the Stop already asked of its harness"
    );
}

/// The host's own worst case for a stop is its grace, three forced steps,
/// and one more for its output to end: the binding waits past all of them
/// for the host's evidence rather than giving up first.
#[tokio::test(start_paused = true)]
async fn a_stop_is_waited_for_through_every_step_the_host_takes() {
    struct Slow;
    struct SlowControl;
    impl HarnessControl for SlowControl {
        fn cleanup(&mut self, grace: Duration, kill: Duration) -> HarnessCleanupFuture<'_> {
            Box::pin(async move {
                tokio::time::sleep(grace + kill * 3).await;
                Ok(CloseOutcome { forced: true })
            })
        }
    }
    impl HarnessLauncher for Slow {
        fn workspace(&self) -> &str {
            "/srv/work"
        }
        fn runs(&self, _agent: &str) -> bool {
            true
        }
        fn launch(
            &self,
            _agent: &str,
            _environment: &BTreeMap<String, String>,
        ) -> Result<HarnessProcess, AgentError> {
            let (input, _harness_in) = duplex(64);
            let (harness_out, output) = duplex(64);
            // Its output never ends by itself: the host waits for it.
            tokio::spawn(async move {
                let _held = harness_out;
                std::future::pending::<()>().await;
            });
            Ok(HarnessProcess {
                input: Box::new(input),
                output: Box::new(output),
                control: Box::new(SlowControl),
            })
        }
    }
    let connector = Connector::with(
        Reach::Serving,
        Arc::new(Slow),
        Arc::new(AtomicUsize::new(0)),
    );
    let environment = environment(connector, Arc::new(Audit::default()));
    let (binding, host) = binding(true);
    let _opened = environment
        .open(&lease(), &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let mut process = host.start(HarnessLaunch::default()).unwrap();
    assert_eq!(
        process
            .control
            .cleanup(Duration::from_millis(10), Duration::from_secs(60))
            .await
            .unwrap(),
        CloseOutcome { forced: true }
    );
}

/// A connection with nothing to say still tells the host the gateway is
/// there: past its keepalive interval the writer sends a keepalive, which
/// the host's silence bound counts on.
#[tokio::test]
async fn an_idle_connection_sends_keepalives() {
    let connector = Connector::new(Reach::Scripted(vec![hello_body(LEASE_PROTOCOL)]));
    let environment = SshEnvironment::new(
        devbox(),
        connector.clone(),
        Arc::new(Audit::default()),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_secs(5),
            keepalive: Duration::from_millis(20),
        },
    );
    // The grant is never answered: the connection is otherwise idle.
    let opening = tokio::spawn(async move {
        environment
            .open(&lease(), &terms("claude"), binding_only())
            .await
            .err()
    });
    let keepalive = encode(&ToEnvironment::Keepalive).unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let received = connector.received.lock().unwrap().clone();
            if received
                .windows(keepalive.len())
                .filter(|window| *window == keepalive)
                .count()
                >= 2
            {
                break;
            }
            drop(received);
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("keepalives while idle");
    opening.abort();
}

/// An output overflow whose audit record cannot be written still stops the
/// harness, but is no success: the binding's stop is uncertain and the
/// lease ends without evidence, so it is Interrupted, never Ended unaudited.
#[tokio::test]
async fn an_overflow_whose_audit_fails_is_not_settled_as_confirmed() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let (binding, host) = binding(true);
    let opened = environment
        .open(&lease(), &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let host = host.lock().unwrap().clone().unwrap();
    let HarnessProcess {
        mut input,
        output: _unread,
        mut control,
    } = host.start(HarnessLaunch::default()).unwrap();
    audit.1.store(true, Ordering::SeqCst);
    input.write_all(b"flood").await.unwrap();
    tokio::time::timeout(Duration::from_secs(10), async {
        while connector.stopped.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the harness is still stopped");
    assert!(matches!(
        control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await,
        Err(AgentError::CleanupUncertain)
    ));
    assert_eq!(
        opened.hold.end(LeaseEndCause::Closed).await,
        LeaseRelease::Unanswered
    );
}

/// A lost connection whose audit record cannot be written leaves its lease
/// without evidence: what a new connection says of it does not settle it.
#[tokio::test]
async fn a_loss_whose_audit_fails_leaves_the_lease_unanswered() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let environment = environment(connector.clone(), audit.clone());
    let opened = environment
        .open(&lease(), &terms("claude"), binding_only())
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let lost = opened.hold.lost().unwrap();
    audit.1.store(true, Ordering::SeqCst);
    connector.cut();
    tokio::time::timeout(Duration::from_secs(5), lost)
        .await
        .expect("the loss is seen");
    // A new connection can be recorded again.
    audit.1.store(false, Ordering::SeqCst);
    assert_eq!(
        opened.hold.end(LeaseEndCause::Lost).await,
        LeaseRelease::Unanswered
    );
}

// ---- command leases (issue #700) ----

/// Runs `echo` at once, and anything else until it is stopped.
struct Commands;
impl CommandRunner for Commands {
    fn run(
        &self,
        command: CommandWork,
        mut stop: tokio::sync::watch::Receiver<Option<CommandStop>>,
    ) -> std::pin::Pin<Box<dyn std::future::Future<Output = CommandRan> + Send + 'static>> {
        Box::pin(async move {
            if command.program() == "echo" {
                let said: Vec<&str> = command.argv()[1..].iter().map(|a| &**a).collect();
                return CommandRan {
                    end: CommandEnd::Exited { code: 0 },
                    stdout: said.join(" ").into_bytes(),
                    stderr: b"warned".to_vec(),
                    dropped_bytes: 2,
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

fn serving_commands() -> Arc<Connector> {
    let connector = Connector::new(Reach::Serving);
    *connector.commands.lock().unwrap() = Some(Arc::new(Commands));
    connector
}

fn work(argv: &[&str]) -> CommandWork {
    CommandWork::new(argv.iter().map(|a| (*a).to_owned()).collect(), None, 10_000).unwrap()
}

#[tokio::test]
async fn a_command_runs_on_the_host_and_answers_how_it_ended_and_what_it_printed() {
    let connector = serving_commands();
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let commands = host.commands().expect("an SSH host runs commands");
    // Not known reachable until a connection is open.
    assert_eq!(commands.reachable(), None);
    let hold = commands
        .grant(&lease(), &work(&["echo", "hi", "there"]))
        .await
        .expect("granted");
    assert_eq!(commands.reachable(), Some(true));
    let (_stop, stopped) = tokio::sync::watch::channel(None);
    let result = tokio::time::timeout(Duration::from_secs(5), hold.run(stopped))
        .await
        .expect("answered");
    assert_eq!(result.exit, CommandExit::Exited { code: 0 });
    assert_eq!(result.stdout, b"hi there");
    assert_eq!(result.stderr, b"warned");
    assert_eq!(result.dropped_bytes, 2);
    assert_eq!(result.cleanup, Some(LeaseCleanup::Confirmed { forced: false }));
    // The host ended the lease itself, and recorded it.
    let ledger = connector.ledger.0.lock().unwrap().clone();
    assert!(
        ledger.iter().any(|entry| matches!(entry, LedgerEntry::Ended { .. })),
        "{ledger:?}"
    );
}

#[tokio::test]
async fn a_stopped_command_is_stopped_on_the_host_and_answers_the_stops_cause() {
    let connector = serving_commands();
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let commands = host.commands().unwrap();
    let hold = commands.grant(&lease(), &work(&["sleep", "60"])).await.unwrap();
    let (stop, stopped) = tokio::sync::watch::channel(None);
    let running = tokio::spawn(hold.run(stopped));
    tokio::time::sleep(Duration::from_millis(50)).await;
    stop.send_replace(Some(LeaseEndCause::Revoked));
    let result = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("a stop is answered")
        .unwrap();
    assert_eq!(
        result.exit,
        CommandExit::Stopped {
            cause: LeaseEndCause::Revoked
        }
    );
    assert_eq!(result.stdout, b"partial");
    assert_eq!(result.cleanup, Some(LeaseCleanup::Confirmed { forced: true }));
}

#[tokio::test]
async fn a_command_is_refused_by_a_host_that_runs_none_or_cannot_be_reached() {
    // Serving, but not configured to run commands.
    let host = environment(Connector::new(Reach::Serving), Arc::new(Audit::default()));
    let refused = host
        .commands()
        .unwrap()
        .grant(&lease(), &work(&["ls"]))
        .await
        .err();
    assert_eq!(refused, Some(CommandRefusal::CommandsUnavailable));
    let host = environment(Connector::new(Reach::Unreachable), Arc::new(Audit::default()));
    let refused = host
        .commands()
        .unwrap()
        .grant(&lease(), &work(&["ls"]))
        .await
        .err();
    assert_eq!(
        refused,
        Some(CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable))
    );
}

#[tokio::test]
async fn a_granted_command_never_run_is_ended_on_the_host() {
    let connector = serving_commands();
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let hold = host
        .commands()
        .unwrap()
        .grant(&lease(), &work(&["echo"]))
        .await
        .unwrap();
    drop(hold);
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let ended = connector
                .ledger
                .0
                .lock()
                .unwrap()
                .iter()
                .any(|entry| matches!(entry, LedgerEntry::Ended { .. }));
            if ended {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host ends a lease whose hold was let go");
}

#[tokio::test]
async fn a_command_whose_connection_is_lost_is_unanswered_with_what_the_host_recorded() {
    let connector = serving_commands();
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let hold = host
        .commands()
        .unwrap()
        .grant(&lease(), &work(&["sleep", "60"]))
        .await
        .unwrap();
    let (_stop, stopped) = tokio::sync::watch::channel(None);
    let running = tokio::spawn(hold.run(stopped));
    tokio::time::sleep(Duration::from_millis(50)).await;
    connector.cut();
    let result = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("a loss is answered")
        .unwrap();
    assert_eq!(result.exit, CommandExit::Unanswered);
}
