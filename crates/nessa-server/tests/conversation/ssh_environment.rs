//! The SSH environment against a substitute for `ssh`: each connection is a
//! pipe to the environment role's own frame loop, run in process, or to a
//! scripted host. What it refuses, what it routes, and what becomes of a
//! lease whose connection is lost.
use super::{
    audit::InstallRefusal,
    audit::{EnvironmentAudit, EnvironmentEvent},
    connector::{ssh_arguments, LeaseConnection, LeaseConnector},
    environment::{SshEnvironment, SshTimings},
    install::{
        open_build, Build, BuildSource, HostInstaller, InstallTimings, RemoteShell, ShellFuture,
    },
    open_ssh::OpenSshConnector,
};
use crate::conversation::application::{Environment, LeaseRelease};
use crate::env::{LEASE_PROTOCOL, VERSION};
use crate::env_serve::application::FrameStream;
use crate::env_serve::application::{
    serve, HarnessLauncher, LeaseLedger, LedgerEntry, ServeTimings,
};
use crate::env_serve::install::Platform;
use nessa_protocol::lease::{decode, encode, Cleanup, Data, FromEnvironment, ToEnvironment};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{
        AgentProvider, CloseOutcome, HarnessCleanupFuture, HarnessControl, HarnessHost,
        HarnessLaunch, HarnessProcess, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
    },
};
use nessa_sdk::domain::{
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
    /// The stream ends at once with nothing said on it: no copy of this
    /// build where `ssh` looked, or nothing reached at all.
    Silent,
}

struct Connector {
    reach: Mutex<Reach>,
    launcher: Arc<dyn HarnessLauncher>,
    ledger: Arc<Ledger>,
    stopped: Arc<AtomicUsize>,
    connects: AtomicUsize,
    /// The command each connection ran on the host.
    commands: Mutex<Vec<String>>,
    /// The relays of each connection, to cut it.
    relays: Mutex<Vec<(JoinHandle<()>, JoinHandle<()>)>>,
    /// What scripted hosts were sent.
    received: Arc<Mutex<Vec<u8>>>,
    /// What a stalling host read once its gate opened.
    frames: Arc<Mutex<Vec<ToEnvironment>>>,
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
            commands: Mutex::new(Vec::new()),
            relays: Mutex::new(Vec::new()),
            received: Arc::new(Mutex::new(Vec::new())),
            frames: Arc::new(Mutex::new(Vec::new())),
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
    fn connect(&self, _host: &SshDestination, command: String) -> io::Result<LeaseConnection> {
        self.connects.fetch_add(1, Ordering::SeqCst);
        self.commands.lock().unwrap().push(command);
        let reach = self.reach.lock().unwrap().clone();
        let (gateway, gateway_end) = duplex(1 << 20);
        let (host_end, host) = duplex(1 << 20);
        match reach {
            Reach::Unreachable => return Err(io::Error::other("no route to host")),
            Reach::Silent => drop(host),
            Reach::Serving => {
                let (host_in, host_out) = split(host);
                tokio::spawn(serve(
                    host_in,
                    host_out,
                    VERSION,
                    LEASE_PROTOCOL,
                    self.launcher.clone(),
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

/// Records events; fails every record once its flag is set, or once it holds
/// as many as its limit (zero: no limit).
#[derive(Default)]
struct Audit(Mutex<Vec<EnvironmentEvent>>, AtomicBool, AtomicUsize);
impl EnvironmentAudit for Audit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
        let limit = self.2.load(Ordering::SeqCst);
        if self.1.load(Ordering::SeqCst) || (limit > 0 && self.0.lock().unwrap().len() >= limit) {
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
        no_install(),
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
        no_install(),
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

/// The destination comes after every option, and the remote command runs
/// the copy of this build kept under its SHA-256, never a `nessa` the
/// host's search path finds: what `ssh` is run with. Which
/// destinations are accepted is `SshDestination`'s own test.
#[test]
fn ssh_is_run_with_the_destination_after_its_options() {
    let host = devbox();
    let digest = "ab".repeat(32);
    let arguments = ssh_arguments(&host, crate::env_serve::install::serve_command(&digest));
    let separator = arguments.iter().position(|word| word == "--").unwrap();
    assert_eq!(
        &arguments[separator + 1..],
        [
            "devbox".to_owned(),
            format!("sh -c 'exec \"$HOME/.nessa/env/{digest}/nessa\" env serve'")
        ]
    );
    assert!(arguments.iter().any(|word| word == "BatchMode=yes"));
    assert!(arguments.iter().any(|word| word == "ForwardAgent=no"));
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
        no_install(),
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

// ---- first use: installing this build on a host (#703) ----

/// A host's shell, scripted: what its probe and its upload answer. An
/// upload answered `installed` makes the host serve from then on, as the
/// copy it put in place would. Keeps every command and what was sent.
struct Shell {
    /// The probe's answers in turn, the last one kept.
    probes: Mutex<Vec<String>>,
    upload: String,
    /// Each command is never answered.
    hangs: bool,
    connector: Arc<Connector>,
    runs: Mutex<Vec<(String, Option<Vec<u8>>)>>,
}

impl Shell {
    fn new(probe: &str, upload: &str, connector: Arc<Connector>) -> Arc<Self> {
        Self::answering(&[probe], upload, connector)
    }
    fn answering(probes: &[&str], upload: &str, connector: Arc<Connector>) -> Arc<Self> {
        Arc::new(Self {
            probes: Mutex::new(probes.iter().map(|probe| (*probe).to_owned()).collect()),
            upload: upload.into(),
            hangs: false,
            connector,
            runs: Mutex::new(Vec::new()),
        })
    }
    fn hanging(connector: Arc<Connector>) -> Arc<Self> {
        Arc::new(Self {
            probes: Mutex::new(vec!["absent Linux x86_64 gnu".into()]),
            upload: "installed".into(),
            hangs: true,
            connector,
            runs: Mutex::new(Vec::new()),
        })
    }
    fn runs(&self) -> Vec<(String, Option<Vec<u8>>)> {
        self.runs.lock().unwrap().clone()
    }
}

impl RemoteShell for Shell {
    fn run<'a>(
        &'a self,
        _host: &'a SshDestination,
        command: String,
        input: Option<std::fs::File>,
    ) -> ShellFuture<'a> {
        Box::pin(async move {
            let sent = input.map(|mut file| {
                let mut bytes = Vec::new();
                std::io::Read::read_to_end(&mut file, &mut bytes).unwrap();
                bytes
            });
            let upload = sent.is_some();
            self.runs.lock().unwrap().push((command, sent));
            if self.hangs {
                std::future::pending::<()>().await;
            }
            if !upload {
                let mut probes = self.probes.lock().unwrap();
                let answer = if probes.len() > 1 {
                    probes.remove(0)
                } else {
                    probes[0].clone()
                };
                // A copy found after an upload is the one it put there.
                let uploaded = self.runs.lock().unwrap().iter().any(|run| run.1.is_some());
                if uploaded && answer.trim() == "present" {
                    *self.connector.reach.lock().unwrap() = Reach::Serving;
                }
                return Ok(answer);
            }
            if self.upload.trim() == "installed" {
                *self.connector.reach.lock().unwrap() = Reach::Serving;
            }
            Ok(self.upload.clone())
        })
    }
}

/// What is sent as this build.
const THIS_BUILD: &[u8] = b"this build's executable";

struct Built;
impl BuildSource for Built {
    fn open(&self) -> io::Result<Build> {
        let mut file = tempfile::tempfile()?;
        std::io::Write::write_all(&mut file, THIS_BUILD)?;
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0))?;
        Ok(Build {
            file,
            digest: this_digest(),
        })
    }
}

fn this_digest() -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(THIS_BUILD))
}

fn installer(shell: Arc<dyn RemoteShell>) -> Arc<HostInstaller> {
    installer_within(shell, Duration::from_secs(5))
}

fn installer_within(shell: Arc<dyn RemoteShell>, bound: Duration) -> Arc<HostInstaller> {
    Arc::new(HostInstaller::new(
        shell,
        Arc::new(Built),
        Platform {
            os: "linux".into(),
            arch: "x86_64".into(),
            libc: "gnu".into(),
        },
        LEASE_PROTOCOL,
        InstallTimings {
            probe: bound,
            upload: bound,
        },
    ))
}

/// The installer of an environment whose host never needs one: its probe
/// says this build is there.
fn no_install() -> Arc<HostInstaller> {
    struct Present;
    impl RemoteShell for Present {
        fn run<'a>(
            &'a self,
            _host: &'a SshDestination,
            _command: String,
            _input: Option<std::fs::File>,
        ) -> ShellFuture<'a> {
            Box::pin(async { Ok("present\n".to_owned()) })
        }
    }
    installer(Arc::new(Present))
}

fn installing(connector: Arc<Connector>, shell: Arc<Shell>, audit: Arc<Audit>) -> SshEnvironment {
    SshEnvironment::new(
        devbox(),
        connector,
        audit,
        installer(shell),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_secs(5),
            ..SshTimings::default()
        },
    )
}

fn refused(lease: &LeaseId, reason: InstallRefusal, seen: Option<&str>) -> EnvironmentEvent {
    EnvironmentEvent::InstallRefused {
        host: "devbox".into(),
        lease: lease.as_str().into(),
        reason,
        seen: seen.map(Into::into),
    }
}

/// A host serving this build is not touched: no probe, no upload, nothing
/// recorded but the connection.
#[tokio::test]
async fn a_host_with_this_build_is_not_touched() {
    let connector = Connector::new(Reach::Serving);
    let shell = Shell::new("present", "installed", connector.clone());
    let audit = Arc::new(Audit::default());
    let environment = installing(connector.clone(), shell.clone(), audit.clone());
    let (binding, _host) = binding(true);
    assert!(environment
        .open(&lease(), &terms("claude"), binding)
        .await
        .is_ok());
    assert!(shell.runs().is_empty());
    assert_eq!(connector.connects.load(Ordering::SeqCst), 1);
    assert!(audit
        .events()
        .iter()
        .all(|event| matches!(event, EnvironmentEvent::Connected { .. })));
}

/// A host with no copy of this build gets one: probed, sent with its
/// digest, recorded before a byte goes and after it verified, then served.
#[tokio::test]
async fn a_host_without_this_build_gets_it_installed_and_then_serves() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new(
        "absent Linux x86_64 gnu\n",
        "installed\n",
        connector.clone(),
    );
    let audit = Arc::new(Audit::default());
    let environment = installing(connector.clone(), shell.clone(), audit.clone());
    let (binding, _host) = binding(true);
    let lease = lease();
    environment
        .open(&lease, &terms("claude"), binding)
        .await
        .map(|_| ())
        .unwrap();
    let runs = shell.runs();
    assert_eq!(runs.len(), 2);
    assert_eq!(
        runs[0],
        (
            crate::env_serve::install::probe_command(LEASE_PROTOCOL, &this_digest()),
            None
        )
    );
    assert_eq!(
        runs[1],
        (
            crate::env_serve::install::upload_command(LEASE_PROTOCOL, &this_digest()),
            Some(THIS_BUILD.to_vec())
        )
    );
    assert_eq!(connector.connects.load(Ordering::SeqCst), 2);
    let serve = crate::env_serve::install::serve_command(&this_digest());
    assert_eq!(*connector.commands.lock().unwrap(), [serve.clone(), serve]);
    let installed = |started: bool| {
        let (protocol, digest) = (LEASE_PROTOCOL.to_owned(), this_digest());
        let (host, lease) = ("devbox".to_owned(), lease.as_str().to_owned());
        match started {
            true => EnvironmentEvent::InstallStarted {
                host,
                lease,
                protocol,
                digest,
            },
            false => EnvironmentEvent::Installed {
                host,
                lease,
                protocol,
                digest,
            },
        }
    };
    assert_eq!(
        audit.events(),
        vec![
            installed(true),
            installed(false),
            EnvironmentEvent::Connected {
                host: "devbox".into(),
                workspace: "/srv/work".into(),
            },
        ]
    );
}

/// A host on a system or processor this build does not run on is refused
/// with that reason, and is sent nothing.
#[tokio::test]
async fn a_host_this_build_cannot_run_on_is_refused_and_sent_nothing() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new("absent Darwin arm64 other", "installed", connector.clone());
    let audit = Arc::new(Audit::default());
    let environment = installing(connector.clone(), shell.clone(), audit.clone());
    let lease = lease();
    assert_eq!(
        environment
            .open(&lease, &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentPlatformUnsupported)
    );
    assert_eq!(shell.runs().len(), 1);
    assert_eq!(
        audit.events(),
        vec![refused(
            &lease,
            InstallRefusal::Platform,
            Some("macos aarch64 other")
        )]
    );
}

/// What the host refuses is refused here with its reason: bytes that are
/// not the ones sent and a copy that does not run fail the install, a copy
/// speaking another protocol is another version. None is connected to.
#[tokio::test]
async fn an_install_the_host_refuses_is_refused_with_its_reason() {
    let cases = [
        (
            "refused fingerprint 00ff",
            LeaseRefusal::EnvironmentInstallFailed,
            (InstallRefusal::Fingerprint, Some("00ff")),
        ),
        (
            "refused version fedcba9876543210",
            LeaseRefusal::EnvironmentVersionMismatch,
            (InstallRefusal::Version, Some("fedcba9876543210")),
        ),
        (
            "refused unrunnable",
            LeaseRefusal::EnvironmentInstallFailed,
            (InstallRefusal::Unrunnable, None),
        ),
        (
            "refused digest_tool",
            LeaseRefusal::EnvironmentInstallFailed,
            (InstallRefusal::DigestTool, None),
        ),
        (
            "failed publish",
            LeaseRefusal::EnvironmentInstallFailed,
            (InstallRefusal::Failed, Some("publish")),
        ),
    ];
    for (answer, refusal, (reason, seen)) in cases {
        let connector = Connector::new(Reach::Silent);
        let shell = Shell::new("absent Linux x86_64 gnu", answer, connector.clone());
        let audit = Arc::new(Audit::default());
        let environment = installing(connector.clone(), shell.clone(), audit.clone());
        let lease = lease();
        assert_eq!(
            environment
                .open(&lease, &terms("claude"), binding_only())
                .await
                .err(),
            Some(refusal),
            "{answer}"
        );
        assert_eq!(connector.connects.load(Ordering::SeqCst), 1, "{answer}");
        let events = audit.events();
        assert!(
            matches!(&events[0], EnvironmentEvent::InstallStarted { lease: started, .. } if started == lease.as_str()),
            "{answer}"
        );
        assert_eq!(events[1..], [refused(&lease, reason, seen)], "{answer}");
    }
}

/// A host whose probe says this build is there, though its stream ended
/// with nothing said, is not written to: the trouble is the host's.
#[tokio::test]
async fn a_host_that_has_this_build_and_does_not_serve_is_unreachable_and_untouched() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new("present", "installed", connector.clone());
    let audit = Arc::new(Audit::default());
    let environment = installing(connector.clone(), shell.clone(), audit.clone());
    assert_eq!(
        environment
            .open(&lease(), &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentUnreachable)
    );
    assert_eq!(shell.runs().len(), 1);
    assert!(audit.events().is_empty());
}

/// An install that cannot be recorded is not made: nothing is sent.
#[tokio::test]
async fn an_install_that_cannot_be_recorded_sends_nothing() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new("absent Linux x86_64 gnu", "installed", connector.clone());
    let audit = Arc::new(Audit::default());
    audit.1.store(true, Ordering::SeqCst);
    let environment = installing(connector.clone(), shell.clone(), audit.clone());
    assert_eq!(
        environment
            .open(&lease(), &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentInstallFailed)
    );
    assert_eq!(shell.runs().len(), 1, "probed, and nothing uploaded");
}

/// An install whose outcome cannot be recorded is not used: the lease is
/// refused, whether the host said installed or the copy was found after a
/// lost answer, and nothing past the start is recorded.
#[tokio::test]
async fn an_install_whose_outcome_cannot_be_recorded_is_not_used() {
    for (probes, upload) in [
        (&["absent Linux x86_64 gnu"][..], "installed"),
        (
            &["absent Linux x86_64 gnu", "present"][..],
            "Connection to devbox closed by remote host.",
        ),
    ] {
        let connector = Connector::new(Reach::Silent);
        let shell = Shell::answering(probes, upload, connector.clone());
        let audit = Arc::new(Audit::default());
        audit.2.store(1, Ordering::SeqCst);
        let environment = installing(connector.clone(), shell.clone(), audit.clone());
        assert_eq!(
            environment
                .open(&lease(), &terms("claude"), binding_only())
                .await
                .err(),
            Some(LeaseRefusal::EnvironmentInstallFailed),
            "{upload}"
        );
        assert_eq!(
            connector.connects.load(Ordering::SeqCst),
            1,
            "not served again"
        );
        let events = audit.events();
        assert_eq!(events.len(), 1);
        assert!(matches!(events[0], EnvironmentEvent::InstallStarted { .. }));
    }
}

/// A refusal that cannot be recorded is answered as a failed install, never
/// as the host's own reason: the audit is what failed.
#[tokio::test]
async fn an_install_refusal_that_cannot_be_recorded_is_a_failed_install() {
    for (probe, upload, limit) in [
        ("absent Darwin arm64 other", "installed", 0),
        (
            "absent Linux x86_64 gnu",
            "refused version fedcba9876543210",
            1,
        ),
    ] {
        let connector = Connector::new(Reach::Silent);
        let shell = Shell::new(probe, upload, connector.clone());
        let audit = Arc::new(Audit::default());
        if limit == 0 {
            audit.1.store(true, Ordering::SeqCst);
        } else {
            audit.2.store(limit, Ordering::SeqCst);
        }
        let environment = installing(connector.clone(), shell.clone(), audit.clone());
        assert_eq!(
            environment
                .open(&lease(), &terms("claude"), binding_only())
                .await
                .err(),
            Some(LeaseRefusal::EnvironmentInstallFailed),
            "{upload}"
        );
        assert_eq!(audit.events().len(), limit, "{upload}");
    }
}

/// A build whose executable is no longer the bytes it is served by (the
/// path replaced by an update) sends nothing: those bytes would be kept
/// where another build is looked for.
#[tokio::test]
async fn an_executable_replaced_since_it_was_measured_is_not_sent() {
    struct Replaced(AtomicUsize);
    impl BuildSource for Replaced {
        fn open(&self) -> io::Result<Build> {
            let opened = Built.open()?;
            match self.0.fetch_add(1, Ordering::SeqCst) {
                0 => Ok(opened),
                _ => Ok(Build {
                    digest: "0".repeat(64),
                    ..opened
                }),
            }
        }
    }
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new("absent Linux x86_64 gnu", "installed", connector.clone());
    let audit = Arc::new(Audit::default());
    let environment = SshEnvironment::new(
        devbox(),
        connector.clone(),
        audit.clone(),
        Arc::new(HostInstaller::new(
            shell.clone(),
            Arc::new(Replaced(AtomicUsize::new(0))),
            Platform {
                os: "linux".into(),
                arch: "x86_64".into(),
                libc: "gnu".into(),
            },
            LEASE_PROTOCOL,
            InstallTimings::default(),
        )),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_secs(5),
            ..SshTimings::default()
        },
    );
    let lease = lease();
    assert_eq!(
        environment
            .open(&lease, &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentInstallFailed)
    );
    assert_eq!(shell.runs().len(), 1, "probed, and nothing uploaded");
    assert_eq!(
        audit.events(),
        [refused(&lease, InstallRefusal::Source, None)]
    );
}

/// An upload whose answer is lost, and whose copy the probe then does not
/// find, is not recorded refused: the host may still be running it and
/// place the copy later, so it is recorded unsettled, and the lease is
/// refused as unreachable.
#[tokio::test]
async fn an_upload_neither_answered_nor_found_is_unsettled_not_refused() {
    for answer in ["Connection closed by remote host", ""] {
        let connector = Connector::new(Reach::Silent);
        let shell = Shell::new("absent Linux x86_64 gnu", answer, connector.clone());
        let audit = Arc::new(Audit::default());
        let environment = installing(connector.clone(), shell.clone(), audit.clone());
        let lease = lease();
        assert_eq!(
            environment
                .open(&lease, &terms("claude"), binding_only())
                .await
                .err(),
            Some(LeaseRefusal::EnvironmentUnreachable),
            "{answer:?}"
        );
        assert_eq!(shell.runs().len(), 3, "probe, upload, probe");
        let events = audit.events();
        assert_eq!(events.len(), 2, "{answer:?}");
        assert!(matches!(events[0], EnvironmentEvent::InstallStarted { .. }));
        assert_eq!(
            events[1],
            EnvironmentEvent::InstallUnsettled {
                host: "devbox".into(),
                lease: lease.as_str().into(),
                protocol: LEASE_PROTOCOL.into(),
                digest: this_digest(),
            }
        );
    }
}

/// An upload whose answer is lost after the host put the copy in place is
/// asked of the host again: found there, it is recorded found (no digest
/// claimed, as another gateway of this protocol may have put it there) and
/// served, never recorded refused.
#[tokio::test]
async fn an_upload_whose_answer_was_lost_is_found_installed_by_the_probe() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::answering(
        &["absent Linux x86_64 gnu", "present"],
        "Connection to devbox closed by remote host.",
        connector.clone(),
    );
    let audit = Arc::new(Audit::default());
    let environment = installing(connector.clone(), shell.clone(), audit.clone());
    let opened = environment
        .open(&lease(), &terms("claude"), binding_only())
        .await;
    assert!(opened.is_ok());
    assert_eq!(shell.runs().len(), 3, "probe, upload, probe");
    let kinds: Vec<_> = audit
        .events()
        .iter()
        .map(|event| match event {
            EnvironmentEvent::InstallStarted { .. } => "started",
            EnvironmentEvent::InstallFound { .. } => "found",
            EnvironmentEvent::Connected { .. } => "connected",
            _ => "other",
        })
        .collect();
    assert_eq!(kinds, ["started", "found", "connected"]);
}

/// A host that answers neither the probe nor the upload in time is
/// unreachable; neither wait is left open.
#[tokio::test]
async fn a_probe_or_upload_not_answered_in_time_is_unreachable() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::hanging(connector.clone());
    let audit = Arc::new(Audit::default());
    let environment = SshEnvironment::new(
        devbox(),
        connector.clone(),
        audit.clone(),
        installer_within(shell.clone(), Duration::from_millis(50)),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_secs(5),
            ..SshTimings::default()
        },
    );
    let refused = tokio::time::timeout(
        Duration::from_secs(5),
        environment.open(&lease(), &terms("claude"), binding_only()),
    )
    .await
    .expect("bounded by the install's own timings");
    assert_eq!(refused.err(), Some(LeaseRefusal::EnvironmentUnreachable));
    assert_eq!(shell.runs().len(), 1, "the probe, unanswered: nothing sent");
    assert!(audit.events().is_empty());
}

/// Accounting for a lease asks the host what it recorded; a host with no
/// copy of this build could answer only that it holds nothing, so none is
/// installed for it.
#[tokio::test]
async fn an_account_never_installs() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new("absent Linux x86_64 gnu", "installed", connector.clone());
    let audit = Arc::new(Audit::default());
    let environment = installing(connector.clone(), shell.clone(), audit.clone());
    assert_eq!(environment.account(&lease()).await, None);
    assert!(shell.runs().is_empty());
    assert!(audit.events().is_empty());
}

/// First use over a real `ssh` and real hosts, run by hand where there are
/// some (the CI runners have none): `NESSA_SSH_FIRST_USE_HOSTS` names
/// OpenSSH destinations, comma-separated, each without this build installed
/// and with a `config.json` naming Claude's harness, and
/// `NESSA_SSH_FIRST_USE_BUILD` the `nessa` built from this source. Each host
/// is installed, served and leased; a second connection finds it there and
/// touches nothing.
#[tokio::test]
#[ignore = "needs real hosts: NESSA_SSH_FIRST_USE_HOSTS and NESSA_SSH_FIRST_USE_BUILD"]
async fn first_use_over_real_ssh() {
    struct File(std::path::PathBuf);
    impl BuildSource for File {
        fn open(&self) -> io::Result<Build> {
            open_build(&self.0)
        }
    }
    async fn open(host: &SshDestination, installer: Arc<HostInstaller>, audit: Arc<Audit>) {
        let environment = SshEnvironment::new(
            host.clone(),
            Arc::new(OpenSshConnector),
            audit,
            installer,
            SshTimings::default(),
        );
        let opened = environment
            .open(&lease(), &terms("claude"), binding(true).0)
            .await
            .unwrap_or_else(|refusal| panic!("{}: {refusal:?}", host.as_str()));
        let released = opened.hold.end(LeaseEndCause::Closed).await;
        assert!(
            matches!(released, LeaseRelease::Released(_)),
            "{released:?}"
        );
    }
    let hosts = std::env::var("NESSA_SSH_FIRST_USE_HOSTS").unwrap();
    let build = std::path::PathBuf::from(std::env::var("NESSA_SSH_FIRST_USE_BUILD").unwrap());
    for host in hosts.split(',') {
        let host = SshDestination::new(host).unwrap();
        let installer = Arc::new(HostInstaller::new(
            Arc::new(OpenSshConnector),
            Arc::new(File(build.clone())),
            Platform::this_build(),
            LEASE_PROTOCOL,
            InstallTimings::default(),
        ));
        let first = Arc::new(Audit::default());
        open(&host, installer.clone(), first.clone()).await;
        let kinds: Vec<_> = first
            .events()
            .iter()
            .map(|event| match event {
                EnvironmentEvent::InstallStarted { .. } => "started",
                EnvironmentEvent::Installed { .. } => "installed",
                EnvironmentEvent::Connected { .. } => "connected",
                _ => "other",
            })
            .collect();
        assert_eq!(
            kinds,
            ["started", "installed", "connected"],
            "{}",
            host.as_str()
        );
        let second = Arc::new(Audit::default());
        open(&host, installer.clone(), second.clone()).await;
        assert!(
            second
                .events()
                .iter()
                .all(|event| matches!(event, EnvironmentEvent::Connected { .. })),
            "{}: {:?}",
            host.as_str(),
            second.events()
        );
        eprintln!(
            "{}: installed, served, leased; then found and not touched",
            host.as_str()
        );
    }
}
