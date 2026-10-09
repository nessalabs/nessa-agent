//! The SSH environment against a substitute for `ssh`: each connection is a
//! pipe to the environment role's own frame loop, run in process, or to a
//! scripted host. What it refuses, what it routes, and what becomes of a
//! lease whose connection is lost.
use super::{
    audit::{EnvironmentAudit, EnvironmentEvent},
    connector::{ssh_arguments, LeaseConnection, LeaseConnector},
    environment::{SshEnvironment, SshTimings},
};
use crate::conversation::application::{Environment, LeaseRelease};
use crate::env::VERSION;
use crate::env_serve::application::{
    serve, HarnessLauncher, LeaseLedger, LedgerEntry, ServeTimings,
};
use nessa_protocol::lease::{Cleanup, Data, FromEnvironment};
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
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{duplex, split, AsyncReadExt, AsyncWriteExt, DuplexStream},
    task::JoinHandle,
};

// ---- the host side ----

/// Echoes its input, except `flood`, which it answers with a mebibyte;
/// counts its stops.
struct Echo {
    stopped: Arc<AtomicUsize>,
}
struct EchoControl(Arc<AtomicUsize>);
impl HarnessControl for EchoControl {
    fn cleanup(&mut self, _grace: Duration, _kill: Duration) -> HarnessCleanupFuture<'_> {
        self.0.fetch_add(1, Ordering::SeqCst);
        Box::pin(async { Ok(CloseOutcome { forced: false }) })
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
        tokio::spawn(async move {
            let mut buffer = [0; 256];
            while let Ok(read) = harness_in.read(&mut buffer).await {
                if &buffer[..read] == b"flood" {
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
            control: Box::new(EchoControl(self.stopped.clone())),
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
}

impl Connector {
    fn new(reach: Reach) -> Arc<Self> {
        let stopped = Arc::new(AtomicUsize::new(0));
        Arc::new(Self {
            reach: Mutex::new(reach),
            launcher: Arc::new(Echo {
                stopped: stopped.clone(),
            }),
            ledger: Arc::new(Ledger::default()),
            stopped,
            connects: AtomicUsize::new(0),
            relays: Mutex::new(Vec::new()),
            received: Arc::new(Mutex::new(Vec::new())),
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
                    self.launcher.clone(),
                    self.ledger.clone(),
                    ServeTimings {
                        grace: Duration::from_millis(10),
                        kill: Duration::from_millis(100),
                    },
                ));
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
struct Audit(Mutex<Vec<EnvironmentEvent>>);
impl EnvironmentAudit for Audit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
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

fn hello_body(build: &str) -> Vec<u8> {
    serde_json::to_vec(&FromEnvironment::Hello {
        build: build.into(),
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

/// Gate 4: another build's hello is a typed refusal, recorded, and nothing
/// is sent to that host.
#[tokio::test]
async fn another_build_is_refused_and_sent_nothing() {
    let connector = Connector::new(Reach::Scripted(vec![hello_body("0.0.0-other")]));
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
            build: Some("0.0.0-other".into()),
        }]
    );
    assert!(host.lock().unwrap().is_none());
    tokio::task::yield_now().await;
    assert!(connector.received.lock().unwrap().is_empty());
    // A first frame that is no hello at all is another build too.
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
    let mut empty_hello = hello_body(VERSION);
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
        hello_body(VERSION),
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
    // with the host's evidence, never left uncertain.
    assert_eq!(
        control
            .cleanup(Duration::from_millis(10), Duration::from_millis(100))
            .await
            .unwrap(),
        CloseOutcome { forced: false }
    );
    assert_eq!(connector.stopped.load(Ordering::SeqCst), 1);
}

/// A grant the host does not answer in time is refused here and ended there:
/// the host never keeps a lease nobody holds.
#[tokio::test]
async fn a_grant_not_answered_in_time_is_ended_on_the_host() {
    let connector = Connector::new(Reach::Scripted(vec![hello_body(VERSION)]));
    let environment = SshEnvironment::new(
        devbox(),
        connector.clone(),
        Arc::new(Audit::default()),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_millis(50),
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
