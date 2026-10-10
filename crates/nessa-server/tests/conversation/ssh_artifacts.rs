//! Files published on a host, end to end (issue #701): the environment
//! role's frame loop and its outbox on disk, run in process; artifact
//! channels served by a scripted SFTP server over the outbox's own files;
//! and the gateway keeping each one with the real attachment service.
//!
//! What it holds the slice to: a disk image and a screenshot arrive and
//! are verified by digest and held with the lease as their cause; the
//! lease's own stream carries none of their bytes; a channel that fails is
//! resumed, and one that keeps failing, or a lease lost mid-read, is
//! refused by name with nothing held.
use super::{
    audit::{EnvironmentAudit, EnvironmentEvent},
    connector::{ArtifactChannel, ArtifactChannels, LeaseConnection, LeaseConnector},
    environment::{SshEnvironment, SshTimings},
    sftp::tests::scripted::{ScriptedSftp, SftpScript},
};
use crate::attachments::application::{AttachmentAuditRecord, AttachmentLimits};
use crate::attachments::infrastructure::ConversationHolds;
use crate::attachments_test_support::{self as attachments, Fixture, CONVERSATION};
use crate::conversation::application::{
    ArtifactKept, ArtifactOffer, ConversationAttachments, Environment, EnvironmentLease,
    PublishedArtifact,
};
use crate::env::{LEASE_PROTOCOL, VERSION};
use crate::env_serve::application::{
    serve, HarnessLauncher, PublishAnswer, PublishRefusal, PublishRequest, ServeTimings,
};
use crate::env_serve::infrastructure::{FileLedger, FileOutbox};
use nessa_protocol::lease::{Collection, CollectionRefusal};
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    permissions::ActionContext,
    providers::{
        AgentProvider, CloseOutcome, HarnessCleanupFuture, HarnessControl, HarnessHost,
        HarnessLaunch, HarnessProcess, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
    },
};
use nessa_sdk::domain::{
    agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseGrants, LeaseId, LeaseTerms, LeaseWork,
        SandboxProfile, SshDestination,
    },
    effective_capabilities::value_objects::EffectiveCapabilities,
};
use sha2::{Digest, Sha256};
use std::{
    collections::{BTreeMap, HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::{
    io::{duplex, split, AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream},
    net::UnixStream,
    sync::Notify,
    task::JoinHandle,
};

// ---- the host ----

/// A harness that runs until it is stopped and does nothing; its launch's
/// publish point is kept for the test to publish on, as the agent's
/// `nessa artifact publish` would.
struct Publisher {
    point: Arc<Mutex<Option<String>>>,
    launched: Arc<Notify>,
}
struct Idle {
    _pipes: (DuplexStream, DuplexStream),
}
impl HarnessControl for Idle {
    fn cleanup(&mut self, _grace: Duration, _kill: Duration) -> HarnessCleanupFuture<'_> {
        Box::pin(async { Ok(CloseOutcome { forced: false }) })
    }
}
impl HarnessLauncher for Publisher {
    fn workspace(&self) -> &str {
        "/unused"
    }
    fn runs(&self, _agent: &str) -> bool {
        true
    }
    fn launch(
        &self,
        _agent: &str,
        _environment: &BTreeMap<String, String>,
        publish_point: Option<&str>,
    ) -> Result<HarnessProcess, AgentError> {
        *self.point.lock().unwrap() = publish_point.map(str::to_owned);
        self.launched.notify_one();
        let (input, harness_in) = duplex(4096);
        let (harness_out, output) = duplex(4096);
        Ok(HarnessProcess {
            input: Box::new(input),
            output: Box::new(output),
            control: Box::new(Idle {
                _pipes: (harness_in, harness_out),
            }),
        })
    }
}

/// Artifact channels served over the outbox's files as they are when each
/// opens, each under the next script, or none.
struct Channels {
    outbox: PathBuf,
    scripts: Mutex<VecDeque<Channel>>,
    opens: AtomicUsize,
    /// File bytes each scripted channel sent.
    sent: Mutex<Vec<Arc<AtomicU64>>>,
}
impl Channels {
    /// File bytes sent over every channel opened, in all.
    fn sent(&self) -> u64 {
        let sent = self.sent.lock().unwrap();
        sent.iter().map(|sent| sent.load(Ordering::SeqCst)).sum()
    }
}
enum Channel {
    Scripted(SftpScript),
    /// Answers nothing, ever: a read that only the lease's end stops.
    Silent,
}
impl ArtifactChannels for Channels {
    fn open(&self) -> io::Result<ArtifactChannel> {
        self.opens.fetch_add(1, Ordering::SeqCst);
        let script = self.scripts.lock().unwrap().pop_front();
        match script {
            Some(Channel::Silent) => {
                let (client, server) = duplex(1024);
                let (from_host, to_host) = split(client);
                Ok(ArtifactChannel {
                    from_host: Box::new(from_host),
                    to_host: Box::new(to_host),
                    keep: Box::new(server),
                })
            }
            script => {
                let script = match script {
                    Some(Channel::Scripted(script)) => script,
                    _ => SftpScript::default(),
                };
                let served = ScriptedSftp::serve(files_under(&self.outbox), script);
                self.sent.lock().unwrap().push(served.sent.clone());
                Ok(ArtifactChannel {
                    from_host: Box::new(served.reader),
                    to_host: Box::new(served.writer),
                    keep: Box::new(()),
                })
            }
        }
    }
}

fn files_under(root: &Path) -> HashMap<String, Vec<u8>> {
    let mut files = HashMap::new();
    let Ok(leases) = std::fs::read_dir(root) else {
        return files;
    };
    for lease in leases.flatten() {
        for file in std::fs::read_dir(lease.path())
            .into_iter()
            .flatten()
            .flatten()
        {
            let path = file.path();
            files.insert(
                path.to_str().unwrap().to_owned(),
                std::fs::read(&path).unwrap(),
            );
        }
    }
    files
}

/// One host: `nessa env serve` over a pipe, counting what its lease
/// stream carries toward the gateway, which a test can cut.
struct Host {
    root: tempfile::TempDir,
    workspace: PathBuf,
    launcher: Arc<Publisher>,
    channels: Arc<Channels>,
    /// Bytes of lease frames the host sent the gateway.
    carried: Arc<AtomicU64>,
    relays: Mutex<Vec<JoinHandle<()>>>,
}

impl Host {
    fn new() -> Arc<Self> {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        std::fs::create_dir(&workspace).unwrap();
        let workspace = std::fs::canonicalize(workspace).unwrap();
        let outbox = root.path().join("outbox");
        Arc::new(Self {
            workspace,
            launcher: Arc::new(Publisher {
                point: Arc::default(),
                launched: Arc::default(),
            }),
            channels: Arc::new(Channels {
                outbox,
                scripts: Mutex::default(),
                opens: AtomicUsize::new(0),
                sent: Mutex::default(),
            }),
            carried: Arc::default(),
            relays: Mutex::default(),
            root,
        })
    }

    fn script(&self, channel: Channel) {
        self.channels.scripts.lock().unwrap().push_back(channel);
    }

    /// Cut the connection, as a network that went away.
    fn cut(&self) {
        for relay in self.relays.lock().unwrap().drain(..) {
            relay.abort();
        }
    }

    /// A file in the workspace.
    fn file(&self, name: &str, bytes: &[u8]) -> PathBuf {
        let path = self.workspace.join(name);
        std::fs::write(&path, bytes).unwrap();
        path
    }

    /// Publish `path` as the agent would, and the answer it is given.
    async fn publish(&self, path: &Path) -> PublishAnswer {
        let address = self.launcher.point.lock().unwrap().clone().unwrap();
        let mut stream = UnixStream::connect(address).await.unwrap();
        let mut line = serde_json::to_vec(&PublishRequest {
            path: path.to_str().unwrap().to_owned(),
            media_type: None,
        })
        .unwrap();
        line.push(b'\n');
        stream.write_all(&line).await.unwrap();
        let mut answer = String::new();
        tokio::time::timeout(
            Duration::from_secs(20),
            BufReader::new(stream).read_line(&mut answer),
        )
        .await
        .expect("an answer in time")
        .unwrap();
        serde_json::from_str(&answer).unwrap()
    }
}

struct Connector(Arc<Host>);
impl LeaseConnector for Connector {
    fn connect(&self, _host: &SshDestination) -> io::Result<LeaseConnection> {
        let host = &self.0;
        let (gateway, gateway_end) = duplex(1 << 20);
        let (host_end, served) = duplex(1 << 20);
        let (host_in, host_out) = split(served);
        let outbox = FileOutbox::open(
            host.root.path().join("outbox"),
            host.root
                .path()
                .join(format!("s{}", uuid::Uuid::new_v4().simple())),
            &host.workspace,
        )?;
        let ledger = FileLedger::open(host.root.path().join("leases.jsonl"))?;
        tokio::spawn(serve(
            host_in,
            host_out,
            VERSION,
            LEASE_PROTOCOL,
            host.launcher.clone(),
            Arc::new(ledger),
            Arc::new(outbox),
            ServeTimings {
                grace: Duration::from_millis(10),
                kill: Duration::from_millis(100),
                ..ServeTimings::default()
            },
        ));
        let (gateway_read, gateway_write) = split(gateway_end);
        let (host_read, host_write) = split(host_end);
        let carried = host.carried.clone();
        let toward_gateway = tokio::spawn(count_copy(host_read, gateway_write, carried));
        let toward_host = tokio::spawn(async move {
            let (mut from, mut to) = (gateway_read, host_write);
            let _ = tokio::io::copy(&mut from, &mut to).await;
            let _ = to.shutdown().await;
        });
        host.relays
            .lock()
            .unwrap()
            .extend([toward_gateway, toward_host]);
        let (from_environment, to_environment) = split(gateway);
        Ok(LeaseConnection {
            from_environment: Box::new(from_environment),
            to_environment: Box::new(to_environment),
            keep: Box::new(()),
            artifacts: host.channels.clone(),
        })
    }
}

async fn count_copy(
    mut from: tokio::io::ReadHalf<DuplexStream>,
    mut to: tokio::io::WriteHalf<DuplexStream>,
    carried: Arc<AtomicU64>,
) {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = match tokio::io::AsyncReadExt::read(&mut from, &mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        carried.fetch_add(read as u64, Ordering::SeqCst);
        if to.write_all(&buffer[..read]).await.is_err() {
            break;
        }
    }
    let _ = to.shutdown().await;
}

#[derive(Default)]
struct Audit(Mutex<Vec<EnvironmentEvent>>);
impl EnvironmentAudit for Audit {
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()> {
        self.0.lock().unwrap().push(event.clone());
        Ok(())
    }
}

// ---- the gateway ----

struct Binding {
    capabilities: EffectiveCapabilities,
    host: Arc<Mutex<Option<Arc<dyn HarnessHost>>>>,
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
        *self.host.lock().unwrap() = Some(host);
        Ok(Arc::new(Binding {
            capabilities: self.capabilities.clone(),
            host: self.host.clone(),
        }))
    }
}

fn devbox() -> SshDestination {
    SshDestination::new("devbox").unwrap()
}

/// Who asked for the lease's work: the conversation's owner.
fn issuer() -> ActionContext {
    ActionContext::new("owner", "phone", "request-1").unwrap()
}

/// A lease on `host` with one harness running under it, its offers kept
/// by `fixture`'s attachment service as the conversation would keep them;
/// what each offer became is sent on the returned channel.
struct Leased {
    lease: LeaseId,
    _held: EnvironmentLease,
    _process: HarnessProcess,
    kept: tokio::sync::mpsc::UnboundedReceiver<Result<ArtifactKept, CollectionRefusal>>,
}

async fn leased(host: &Arc<Host>, fixture: &Fixture) -> Leased {
    let environment = SshEnvironment::new(
        devbox(),
        Arc::new(Connector(host.clone())),
        Arc::new(Audit::default()),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_secs(5),
            ..SshTimings::default()
        },
    );
    let given = Arc::new(Mutex::new(None));
    let capabilities = crate::conversation_test_support::Provider::new(Arc::new(
        crate::conversation_test_support::ProviderFactory::default(),
    ))
    .capabilities()
    .clone();
    let binding = Arc::new(Binding {
        capabilities,
        host: given.clone(),
    });
    let lease = LeaseId::new(uuid::Uuid::new_v4().to_string()).unwrap();
    let terms = LeaseTerms {
        environment: EnvironmentRef::Ssh(devbox()),
        work: LeaseWork::Agent(AgentWork::new("claude", "model").unwrap()),
        sandbox: SandboxProfile::HarnessDefault,
        grants: LeaseGrants::Opening,
        deadline: LeaseDeadline::UntilEnded,
    };
    let held = environment.open(&lease, &terms, binding).await.unwrap();
    let launched = host.launcher.launched.notified();
    let process = given
        .lock()
        .unwrap()
        .clone()
        .unwrap()
        .start(HarnessLaunch::default())
        .unwrap();
    tokio::time::timeout(Duration::from_secs(5), launched)
        .await
        .expect("the harness launched");
    let mut offers = held.hold.artifacts().expect("a host's lease publishes");
    assert!(held.hold.artifacts().is_none(), "handed out once");
    let keeper = ConversationHolds::new(fixture.service.clone());
    let (results, kept) = tokio::sync::mpsc::unbounded_channel();
    let id = lease.as_str().to_owned();
    tokio::spawn(async move {
        while let Some(ArtifactOffer {
            file,
            bytes,
            answer,
        }) = offers.recv().await
        {
            let result = keeper
                .keep_published(PublishedArtifact {
                    organization_id: attachments::organization("org"),
                    conversation_id: attachments::conversation(CONVERSATION),
                    lease: id.clone(),
                    requested_by: issuer(),
                    file,
                    bytes,
                    record: Box::new(|| Box::pin(async { true })),
                })
                .await;
            answer.answer(match result {
                Ok(ArtifactKept::Held) => Collection::Held,
                Ok(ArtifactKept::AlreadyHeld) => Collection::AlreadyHeld,
                Err(reason) => Collection::Refused { reason },
            });
            let _ = results.send(result);
        }
    });
    Leased {
        lease,
        _held: held,
        _process: process,
        kept,
    }
}

fn hex(bytes: &[u8]) -> String {
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

fn pattern(len: usize, seed: u8) -> Vec<u8> {
    (0..len)
        .map(|index| (index as u8).wrapping_mul(31).wrapping_add(seed))
        .collect()
}

/// Whether the outbox still stages anything.
fn staged(host: &Host) -> usize {
    files_under(&host.root.path().join("outbox")).len()
}

#[tokio::test]
async fn a_disk_image_and_a_screenshot_arrive_verified_and_held_under_the_lease() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let image = pattern(3 * 1024 * 1024 + 17, 1);
    let screenshot = pattern(180_000, 2);
    let dmg = host.file("Nessa.dmg", &image);
    let png = host.file("simulator.png", &screenshot);

    let before = host.carried.load(Ordering::SeqCst);
    assert_eq!(
        host.publish(&dmg).await,
        PublishAnswer::Held {
            digest: hex(&image),
            size: image.len() as u64,
        }
    );
    assert_eq!(
        host.publish(&png).await,
        PublishAnswer::Held {
            digest: hex(&screenshot),
            size: screenshot.len() as u64,
        }
    );
    // The lease's own stream carried their descriptions, never their bytes.
    let carried = host.carried.load(Ordering::SeqCst) - before;
    assert!(carried < 4096, "the lease stream carried {carried} bytes");
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));

    let held = fixture.store.held();
    assert_eq!(held.len(), 2);
    for (bytes, media_type) in [
        (&image, "application/x-apple-diskimage"),
        (&screenshot, "image/png"),
    ] {
        let hold = held
            .iter()
            .find(|hold| hold.stored().digest() == attachments::digest_of(bytes))
            .expect("held");
        assert_eq!(hold.lease(), Some(leased.lease.as_str()));
        assert_eq!(hold.stored().media_type().as_str(), media_type);
        assert_eq!(
            fixture.store.blob(attachments::digest_of(bytes)).as_deref(),
            Some(bytes.as_slice())
        );
    }
    // Each creation is audited with the lease as its cause.
    let created: Vec<_> = fixture
        .audit
        .taken_all()
        .into_iter()
        .filter_map(|record| match record {
            AttachmentAuditRecord::HoldCreated { hold } => Some(hold),
            _ => None,
        })
        .collect();
    assert_eq!(created.len(), 2);
    assert!(created
        .iter()
        .all(|hold| hold.lease() == Some(leased.lease.as_str())));
    // Answered, the host lets go of its staged copies.
    assert_eq!(staged(&host), 0);
}

#[tokio::test]
async fn a_channel_cut_mid_read_is_resumed_and_the_file_still_verifies() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let bytes = pattern(400_000, 3);
    let file = host.file("build.zip", &bytes);
    host.script(Channel::Scripted(SftpScript {
        cut_after: Some(150_000),
        ..SftpScript::default()
    }));
    host.script(Channel::Scripted(SftpScript {
        cut_after: Some(100_000),
        ..SftpScript::default()
    }));
    assert_eq!(
        host.publish(&file).await,
        PublishAnswer::Held {
            digest: hex(&bytes),
            size: bytes.len() as u64,
        }
    );
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 3);
    // Each channel read on from where the last one reached, never again
    // from the start: all that is read twice is the packet each cut broke.
    let sent = host.channels.sent();
    let cut_short = 2 * u64::from(super::sftp::READ_CHUNK);
    assert!(
        (bytes.len() as u64..bytes.len() as u64 + cut_short).contains(&sent),
        "{sent} bytes sent for a file of {}",
        bytes.len()
    );
    assert_eq!(
        fixture
            .store
            .blob(attachments::digest_of(&bytes))
            .as_deref(),
        Some(bytes.as_slice())
    );
}

#[tokio::test]
async fn a_channel_that_keeps_failing_is_refused_by_name_and_nothing_is_held() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let file = host.file("build.zip", &pattern(200_000, 4));
    for _ in 0..4 {
        host.script(Channel::Scripted(SftpScript {
            cut_after: Some(10_000),
            ..SftpScript::default()
        }));
    }
    assert_eq!(
        host.publish(&file).await,
        PublishAnswer::Refused {
            reason: CollectionRefusal::ChannelUnavailable
        }
    );
    assert_eq!(
        leased.kept.recv().await,
        Some(Err(CollectionRefusal::ChannelUnavailable))
    );
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 4);
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert!(fixture
        .audit
        .taken_all()
        .iter()
        .any(|record| matches!(record, AttachmentAuditRecord::PublishRejected { .. })));
    assert_eq!(staged(&host), 0);
}

#[tokio::test]
async fn a_lease_lost_mid_read_fails_as_ended_and_leaves_no_partial_hold() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let file = host.file("Nessa.dmg", &pattern(500_000, 5));
    host.script(Channel::Silent);
    let publisher = {
        let host = host.clone();
        tokio::spawn(async move { host.publish(&file).await })
    };
    // Wait until the read is under way on its silent channel.
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.channels.opens.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the read started");
    host.cut();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), leased.kept.recv())
            .await
            .expect("an outcome in time"),
        Some(Err(CollectionRefusal::LeaseEnded))
    );
    // The host ended the lease with its connection, and said so.
    assert!(matches!(
        publisher.await.unwrap(),
        PublishAnswer::NotPublished {
            reason: PublishRefusal::LeaseEnded
        } | PublishAnswer::Refused {
            reason: CollectionRefusal::LeaseEnded
        }
    ));
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(staged(&host), 0);
}

#[tokio::test]
async fn a_file_the_conversation_already_holds_is_not_read_again() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let bytes = pattern(90_000, 6);
    let file = host.file("simulator.png", &bytes);
    assert!(matches!(
        host.publish(&file).await,
        PublishAnswer::Held { .. }
    ));
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));
    let opens = host.channels.opens.load(Ordering::SeqCst);
    assert_eq!(
        host.publish(&file).await,
        PublishAnswer::AlreadyHeld {
            digest: hex(&bytes),
            size: bytes.len() as u64,
        }
    );
    assert_eq!(
        leased.kept.recv().await,
        Some(Ok(ArtifactKept::AlreadyHeld))
    );
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), opens);
    assert_eq!(fixture.store.held().len(), 1);
}

#[tokio::test]
async fn a_file_outside_the_workspace_never_reaches_the_gateway() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let _leased = leased(&host, &fixture).await;
    let outside = host.root.path().join("secret");
    std::fs::write(&outside, b"not the agent's").unwrap();
    assert_eq!(
        host.publish(&outside).await,
        PublishAnswer::NotPublished {
            reason: PublishRefusal::OutsideWorkspace
        }
    );
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
    assert!(fixture.store.held().is_empty());
}
