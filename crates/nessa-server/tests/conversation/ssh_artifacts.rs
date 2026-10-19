//! Files published on a host, end to end (issue #701): the environment
//! role's frame loop and its outbox on disk, run in process; artifact
//! channels served by a scripted SFTP server over the outbox's own files;
//! and the gateway keeping each one with the real attachment service.
//!
//! What it holds the slice to: a disk image and a screenshot arrive and
//! are verified by digest and held with the lease as their cause; the
//! lease's own stream carries none of their bytes; a channel that fails is
//! resumed, and one that keeps failing, or a lease lost mid-read, is
//! refused by name with nothing held. On a paired connection, which has
//! no artifact channel, the same files travel on the lease frames
//! themselves, and a lease lost or a file changed mid-read is refused by
//! name the same way. And the other way (slice D3b): a file the
//! conversation holds is written to the host over the same channels,
//! resumed by the part's length, and placed only once verified there;
//! a paired connection is never asked to fetch one.
use super::{
    audit::{EnvironmentAudit, EnvironmentEvent},
    connector::{
        ArtifactChannel, ArtifactChannels, ArtifactTransport, LeaseConnection, LeaseConnector,
    },
    environment::{SshEnvironment, SshTimings},
    install::{Build, BuildSource, HostInstaller, InstallTimings, RemoteShell, ShellFuture},
    sftp::tests::scripted::{ScriptedSftp, SftpScript},
};
use crate::attachments::application::{AttachmentAuditRecord, AttachmentLimits};
use crate::attachments::infrastructure::ConversationHolds;
use crate::attachments_test_support::{self as attachments, Fixture, CONVERSATION};
use crate::conversation::application::{
    ArtifactKept, ArtifactOffer, ConversationAttachments, Environment, EnvironmentFuture,
    EnvironmentLease, FetchAsk, FetchFailure, FetchPlaced, HeldSource, HeldSourceFailure,
    PublishAsk, PublishFailure, PublishedArtifact,
};
use crate::env::{LEASE_PROTOCOL, VERSION};
use crate::env_serve::application::{
    serve, HarnessLauncher, PeerGrants, PublishAnswer, PublishRequest, Runners, ServeTimings,
    Serving,
};
use crate::env_serve::domain::{Peer, PeerGrant};
use crate::env_serve::infrastructure::{FileInbox, FileLedger, FileOutbox};
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;

use crate::env_serve::install::Platform;
use crate::timer::TokioTimer;
use nessa_protocol::lease::{Collection, CollectionRefusal, FetchRefusal, PublishRefusal};
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
        AgentWork, EnvironmentRef, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseId, LeaseTerms,
        LeaseWork, SandboxProfile, SandboxProfiles, SshDestination,
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
    sync::{watch, Notify},
    task::JoinHandle,
};

// ---- the host ----

/// A harness that runs until it is stopped and does nothing; its launch's
/// publish point is kept for the test to publish on, as the agent's
/// `nessa artifact publish` would.
struct Publisher {
    point: Arc<Mutex<Option<String>>>,
    launched: Arc<Notify>,
    /// The workspace it reports, where a relative path is resolved.
    workspace: String,
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
        &self.workspace
    }
    fn runs(&self, _agent: &str) -> bool {
        true
    }
    fn sandboxes(&self) -> SandboxProfiles {
        SandboxProfiles::HARNESS_DEFAULT
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
                // What is written lands on the host's disk, as its own SFTP
                // server would write it.
                let script = SftpScript {
                    write_through: true,
                    ..script
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
    /// Where publish points go: short, as a Unix socket's path is bounded
    /// (104 bytes on macOS) and the temporary directory there is long.
    sockets: tempfile::TempDir,
    workspace: PathBuf,
    launcher: Arc<Publisher>,
    channels: Arc<Channels>,
    /// Bytes of lease frames the host sent the gateway.
    carried: Arc<AtomicU64>,
    /// The most of them relayed to the gateway: past it, the relay holds
    /// what it read until this is raised.
    relay_limit: watch::Sender<u64>,
    relays: Mutex<Vec<JoinHandle<()>>>,
    /// The connection is a paired one: published files travel on the lease
    /// frames, with no artifact channel beside them.
    paired: bool,
}

impl Host {
    fn new() -> Arc<Self> {
        Self::with(false)
    }

    /// A host reached over a paired connection.
    fn paired() -> Arc<Self> {
        Self::with(true)
    }

    fn with(paired: bool) -> Arc<Self> {
        let root = tempfile::tempdir().unwrap();
        let workspace = root.path().join("work");
        std::fs::create_dir(&workspace).unwrap();
        let workspace = std::fs::canonicalize(workspace).unwrap();
        let outbox = root.path().join("outbox");
        Arc::new(Self {
            workspace: workspace.clone(),
            launcher: Arc::new(Publisher {
                point: Arc::default(),
                launched: Arc::default(),
                workspace: workspace.to_str().unwrap().to_owned(),
            }),
            channels: Arc::new(Channels {
                outbox,
                scripts: Mutex::default(),
                opens: AtomicUsize::new(0),
                sent: Mutex::default(),
            }),
            carried: Arc::default(),
            relay_limit: watch::channel(u64::MAX).0,
            relays: Mutex::default(),
            paired,
            root,
            sockets: tempfile::Builder::new().prefix("ns").tempdir().unwrap(),
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

/// The installer of a host that serves already: its probe finds this build,
/// so nothing is ever sent.
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
    struct Unread;
    impl BuildSource for Unread {
        fn open(&self) -> io::Result<Build> {
            Ok(Build {
                file: tempfile::tempfile()?,
                digest: "0".repeat(64),
            })
        }
    }
    Arc::new(HostInstaller::new(
        Arc::new(Present),
        Arc::new(Unread),
        Platform::this_build(),
        LEASE_PROTOCOL,
        InstallTimings::default(),
    ))
}

/// SSH with no grants kept: everything the host runs.
struct NoGrants;
impl PeerGrants for NoGrants {
    fn entry(&self, _peer: &Peer) -> Result<Option<PeerGrant>, String> {
        Ok(None)
    }
}

struct Connector(Arc<Host>);
impl LeaseConnector for Connector {
    fn connect(&self, _host: &SshDestination, _command: String) -> io::Result<LeaseConnection> {
        let host = &self.0;
        let (gateway, gateway_end) = duplex(1 << 20);
        let (host_end, served) = duplex(1 << 20);
        let (host_in, host_out) = split(served);
        let outbox = FileOutbox::open(
            host.root.path().join("outbox"),
            host.sockets
                .path()
                .join(&uuid::Uuid::new_v4().simple().to_string()[..8]),
            &host.workspace,
        )?;
        let ledger = FileLedger::open(host.root.path().join("leases.jsonl"))?;
        let inbox = FileInbox::open(host.root.path().join("inbox"), &host.workspace)?;
        tokio::spawn(serve(
            host_in,
            host_out,
            VERSION,
            LEASE_PROTOCOL,
            Serving {
                peer: Peer::Ssh,
                grants: Arc::new(NoGrants),
                policy: Arc::new(
                    crate::env_serve::infrastructure::CedarLeasePolicy::new().unwrap(),
                ),
            },
            Runners {
                harnesses: host.launcher.clone(),
                commands: None,
            },
            Arc::new(ledger),
            Arc::new(outbox),
            Some(Arc::new(inbox)),
            ServeTimings {
                grace: Duration::from_millis(10),
                kill: Duration::from_millis(100),
                ..ServeTimings::default()
            },
            Arc::new(TokioTimer::new()),
            tokio::sync::watch::channel(false).1,
        ));
        let (gateway_read, gateway_write) = split(gateway_end);
        let (host_read, host_write) = split(host_end);
        let carried = host.carried.clone();
        let toward_gateway = tokio::spawn(count_copy(
            host_read,
            gateway_write,
            carried,
            host.relay_limit.subscribe(),
        ));
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
            artifacts: match host.paired {
                true => ArtifactTransport::Frames,
                false => ArtifactTransport::Channels(host.channels.clone()),
            },
        })
    }
}

async fn count_copy(
    mut from: tokio::io::ReadHalf<DuplexStream>,
    mut to: tokio::io::WriteHalf<DuplexStream>,
    carried: Arc<AtomicU64>,
    mut limit: watch::Receiver<u64>,
) {
    let mut buffer = vec![0; 64 * 1024];
    loop {
        let read = match tokio::io::AsyncReadExt::read(&mut from, &mut buffer).await {
            Ok(0) | Err(_) => break,
            Ok(read) => read,
        };
        let total = carried.fetch_add(read as u64, Ordering::SeqCst) + read as u64;
        // Held until the limit allows it, or the relay is cut.
        if limit.wait_for(|limit| total <= *limit).await.is_err() {
            break;
        }
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
    captured_host: Option<Arc<dyn HarnessHost>>,
    capabilities: EffectiveCapabilities,
    host: Arc<Mutex<Option<Arc<dyn HarnessHost>>>>,
}
impl AgentProvider for Binding {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.captured_host
            .as_ref()
            .map_or_else(HarnessHostIdentity::local, HarnessHostIdentity::hosted)
    }
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
        *self.host.lock().unwrap() = Some(host.clone());
        Ok(Arc::new(Binding {
            captured_host: Some(host),
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
    held: EnvironmentLease,
    _process: HarnessProcess,
    kept: tokio::sync::mpsc::UnboundedReceiver<Result<ArtifactKept, CollectionRefusal>>,
    /// The request each offer named, in the order offered.
    requests: tokio::sync::mpsc::UnboundedReceiver<Option<String>>,
}

async fn leased(host: &Arc<Host>, fixture: &Fixture) -> Leased {
    leased_pausing(host, fixture, None).await
}

/// [`leased`], whose first record, once asked, says so on `entered` and
/// waits for `go`.
async fn leased_pausing(
    host: &Arc<Host>,
    fixture: &Fixture,
    pause: Option<(Arc<tokio::sync::Notify>, tokio::sync::oneshot::Receiver<()>)>,
) -> Leased {
    let mut pause = pause;
    let environment = SshEnvironment::new(
        devbox(),
        Arc::new(Connector(host.clone())),
        Arc::new(Audit::default()),
        no_install(),
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
        captured_host: None,
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
    let keeper = ConversationHolds::new(fixture.service.clone(), fixture.reads());
    let (results, kept) = tokio::sync::mpsc::unbounded_channel();
    let (named, requests) = tokio::sync::mpsc::unbounded_channel();
    let id = lease.as_str().to_owned();
    tokio::spawn(async move {
        while let Some(ArtifactOffer {
            file,
            bytes,
            answer,
            request,
        }) = offers.recv().await
        {
            let _ = named.send(request);
            let result = keeper
                .keep_published(PublishedArtifact {
                    organization_id: attachments::organization("org"),
                    conversation_id: attachments::conversation(CONVERSATION),
                    lease: id.clone(),
                    requested_by: issuer(),
                    file,
                    bytes,
                    record: {
                        let pause = pause.take();
                        Box::new(move || {
                            Box::pin(async move {
                                if let Some((entered, go)) = pause {
                                    entered.notify_one();
                                    let _ = go.await;
                                }
                                true
                            })
                        })
                    },
                })
                .await;
            answer
                .answer(match result {
                    Ok(ArtifactKept::Held) => Collection::Held,
                    Ok(ArtifactKept::AlreadyHeld) => Collection::AlreadyHeld,
                    Err(reason) => Collection::Refused { reason },
                })
                .await;
            let _ = results.send(result);
        }
    });
    Leased {
        lease,
        held,
        _process: process,
        kept,
        requests,
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
    // The host lost the lease with its connection, before any answer: it
    // says it does not know, rather than that nothing was kept.
    assert_eq!(publisher.await.unwrap(), PublishAnswer::Unanswered);
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(staged(&host), 0);
}

/// A file kept while its lease's End is asked is told to the host as held:
/// the End waits for its answer, so the host never answers it as ended.
#[tokio::test]
async fn a_file_kept_as_its_lease_ends_is_answered_held_before_the_end() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let entered = Arc::new(tokio::sync::Notify::new());
    let (go, wait) = tokio::sync::oneshot::channel();
    let mut leased = leased_pausing(&host, &fixture, Some((entered.clone(), wait))).await;
    let bytes = pattern(100_000, 7);
    let file = host.file("Nessa.dmg", &bytes);
    let publisher = {
        let host = host.clone();
        tokio::spawn(async move { host.publish(&file).await })
    };
    tokio::time::timeout(Duration::from_secs(5), entered.notified())
        .await
        .expect("the file was kept and its record asked");
    let ending = {
        let hold = leased.held.hold.clone();
        tokio::spawn(async move { hold.end(LeaseEndCause::Stopped).await })
    };
    // Give the End every chance to go first.
    tokio::time::sleep(Duration::from_millis(100)).await;
    go.send(()).unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), publisher)
            .await
            .expect("the publisher is answered")
            .unwrap(),
        PublishAnswer::Held {
            digest: hex(&bytes),
            size: bytes.len() as u64,
        }
    );
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));
    tokio::time::timeout(Duration::from_secs(10), ending)
        .await
        .expect("the lease ends once its file is answered")
        .unwrap();
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

fn ask(request: &str, path: &str) -> PublishAsk {
    PublishAsk {
        request: request.into(),
        path: path.into(),
        media_type: None,
    }
}

/// Slice D3a: a publish the gateway asks for on the agent's behalf is
/// offered on the lease's one offers receiver, naming its request, read
/// over the artifact channel and verified like any; the lease's own stream
/// carries its path and digest, never its bytes.
#[tokio::test]
async fn a_gateway_asked_publish_travels_the_same_channel_and_the_control_stream_carries_no_bytes()
{
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let image = pattern(2 * 1024 * 1024 + 5, 9);
    host.file("Nessa.dmg", &image);
    let before = host.carried.load(Ordering::SeqCst);
    let publish = leased
        .held
        .hold
        .publish(ask("call-1", "Nessa.dmg"))
        .expect("a host's lease publishes");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), publish)
            .await
            .expect("offered in time"),
        Ok(())
    );
    assert_eq!(leased.requests.recv().await, Some(Some("call-1".into())));
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));
    let carried = host.carried.load(Ordering::SeqCst) - before;
    assert!(carried < 4096, "the lease stream carried {carried} bytes");
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 1);
    let held = fixture.store.held();
    let [hold] = held.as_slice() else {
        panic!("one hold, got {}", held.len())
    };
    assert_eq!(hold.lease(), Some(leased.lease.as_str()));
    assert_eq!(
        fixture
            .store
            .blob(attachments::digest_of(&image))
            .as_deref(),
        Some(image.as_slice())
    );
    // A harness's own publish on the same lease names no request.
    let png = host.file("simulator.png", &pattern(1000, 3));
    assert!(matches!(
        host.publish(&png).await,
        PublishAnswer::Held { .. }
    ));
    assert_eq!(leased.requests.recv().await, Some(None));
    assert_eq!(staged(&host), 0);
}

/// A file the host will not stage is answered by its reason, and nothing
/// is offered.
#[tokio::test]
async fn a_gateway_asked_publish_the_host_refuses_is_answered_by_its_reason() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let outside = host.root.path().join("secret");
    std::fs::write(&outside, b"not the agent's").unwrap();
    for (path, reason) in [
        (outside.to_str().unwrap(), PublishRefusal::OutsideWorkspace),
        ("../secret", PublishRefusal::OutsideWorkspace),
        ("missing.png", PublishRefusal::Unreadable),
    ] {
        let publish = leased.held.hold.publish(ask(path, path)).unwrap();
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(10), publish)
                .await
                .unwrap(),
            Err(PublishFailure::NotPublished(reason)),
            "{path}"
        );
    }
    assert!(leased.requests.try_recv().is_err());
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
}

/// A publish asked once the lease ended, or once its connection was lost,
/// is answered so at once, and nothing is offered.
#[tokio::test]
async fn a_gateway_asked_publish_after_the_lease_ended_or_was_lost_is_answered_so() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let ended = leased(&host, &fixture).await;
    host.file("a.png", &pattern(100, 1));
    ended.held.hold.end(LeaseEndCause::Stopped).await;
    assert_eq!(
        ended
            .held
            .hold
            .publish(ask("call-1", "a.png"))
            .unwrap()
            .await,
        Err(PublishFailure::NotPublished(PublishRefusal::LeaseEnded))
    );

    let host = Host::new();
    let lost = leased(&host, &fixture).await;
    host.file("a.png", &pattern(100, 1));
    host.cut();
    let cause = tokio::time::timeout(Duration::from_secs(10), lost.held.hold.ends().unwrap())
        .await
        .expect("the loss is seen");
    assert_eq!(
        cause,
        nessa_sdk::domain::agent_execution::leases::LeaseEndCause::Lost
    );
    assert_eq!(
        lost.held
            .hold
            .publish(ask("call-1", "a.png"))
            .unwrap()
            .await,
        Err(PublishFailure::Unanswered)
    );
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
}

// ---- fetching (slice D3b) ----

/// A held file's bytes, as the conversation's hold answers them: let go
/// after `let_go_after` ranges when set.
struct Held {
    bytes: Vec<u8>,
    reads: AtomicUsize,
    let_go_after: Option<usize>,
}

impl Held {
    fn of(bytes: &[u8]) -> Arc<Self> {
        Arc::new(Self {
            bytes: bytes.to_vec(),
            reads: AtomicUsize::new(0),
            let_go_after: None,
        })
    }
}

impl HeldSource for Held {
    fn read(
        &self,
        offset: u64,
        max: usize,
    ) -> EnvironmentFuture<'_, Result<Vec<u8>, HeldSourceFailure>> {
        Box::pin(async move {
            let read = self.reads.fetch_add(1, Ordering::SeqCst);
            if self.let_go_after.is_some_and(|after| read >= after) {
                return Err(HeldSourceFailure::NotHeld);
            }
            let start = offset as usize;
            Ok(self.bytes[start..(start + max).min(self.bytes.len())].to_vec())
        })
    }
}

/// A fetch of `bytes` to `destination`, said to be the file `named`.
fn fetch_ask(
    request: &str,
    destination: &str,
    named: &[u8],
    source: Arc<dyn HeldSource>,
) -> FetchAsk {
    use nessa_sdk::application::agent_execution::sessions::PublishedFile;
    use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
    FetchAsk {
        request: request.into(),
        destination: destination.into(),
        file: PublishedFile::new(
            Sha256Digest::parse(&format!("sha256:{}", hex(named))).unwrap(),
            MediaType::parse("application/octet-stream").unwrap(),
            named.len() as u64,
        )
        .unwrap(),
        source,
        delivered: Arc::default(),
    }
}

async fn fetched(leased: &Leased, ask: FetchAsk) -> Result<FetchPlaced, FetchFailure> {
    let fetch = leased.held.hold.fetch(ask).expect("a host's lease fetches");
    tokio::time::timeout(Duration::from_secs(20), fetch)
        .await
        .expect("answered in time")
}

/// The parts the host's inbox holds now.
fn parts(host: &Host) -> usize {
    files_under(&host.root.path().join("inbox")).len()
}

/// Its parts, once the host let them go.
async fn parts_let_go(host: &Host) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while parts(host) > 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host lets its parts go");
}

/// A file the conversation holds is written over an artifact channel, never
/// the lease's own stream, verified on the host and placed where nothing
/// was; asked again, it is already there and nothing is written.
#[tokio::test]
async fn a_fetched_file_is_written_over_an_artifact_channel_and_placed_verified() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let leased = leased(&host, &fixture).await;
    std::fs::create_dir(host.workspace.join("assets")).unwrap();
    let image = pattern(2 * 1024 * 1024 + 5, 9);
    let before = host.carried.load(Ordering::SeqCst);
    let placed = host.workspace.join("assets/Nessa.dmg");
    let ask = fetch_ask("call-1", "assets/Nessa.dmg", &image, Held::of(&image));
    let delivered = ask.delivered.clone();
    assert_eq!(
        fetched(&leased, ask).await,
        Ok(FetchPlaced::Placed {
            path: placed.to_str().unwrap().into()
        })
    );
    assert!(
        delivered.load(Ordering::SeqCst),
        "said delivered before placed"
    );
    assert_eq!(std::fs::read(&placed).unwrap(), image);
    let carried = host.carried.load(Ordering::SeqCst) - before;
    assert!(carried < 4096, "the lease stream carried {carried} bytes");
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 1);
    assert_eq!(parts(&host), 0);
    let source = Held::of(&image);
    assert_eq!(
        fetched(
            &leased,
            fetch_ask("call-2", placed.to_str().unwrap(), &image, source.clone())
        )
        .await,
        Ok(FetchPlaced::AlreadyPlaced {
            path: placed.to_str().unwrap().into()
        })
    );
    assert_eq!(source.reads.load(Ordering::SeqCst), 0);
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 1);
}

/// A channel cut mid-write is resumed on a new one from what the part
/// holds, and the file still verifies.
#[tokio::test]
async fn a_channel_cut_mid_write_is_resumed_by_the_parts_length_and_still_verifies() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let leased = leased(&host, &fixture).await;
    let image = pattern(1024 * 1024 + 3, 4);
    host.script(Channel::Scripted(SftpScript {
        cut_writes_after: Some(300_000),
        ..SftpScript::default()
    }));
    let placed = host.workspace.join("image.bin");
    assert_eq!(
        fetched(
            &leased,
            fetch_ask("call-1", "image.bin", &image, Held::of(&image))
        )
        .await,
        Ok(FetchPlaced::Placed {
            path: placed.to_str().unwrap().into()
        })
    );
    assert_eq!(std::fs::read(&placed).unwrap(), image);
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 2);
}

/// The conversation letting go of the file mid-write stops it there: the
/// host is told nothing more comes, lets the part go, and places nothing.
#[tokio::test]
async fn a_hold_let_go_mid_write_stops_it_not_held_and_nothing_is_placed() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let leased = leased(&host, &fixture).await;
    let image = pattern(1024 * 1024, 2);
    let source = Arc::new(Held {
        bytes: image.clone(),
        reads: AtomicUsize::new(0),
        let_go_after: Some(3),
    });
    assert_eq!(
        fetched(&leased, fetch_ask("call-1", "image.bin", &image, source)).await,
        Err(FetchFailure::NotHeld)
    );
    assert!(!host.workspace.join("image.bin").exists());
    parts_let_go(&host).await;
}

/// What the host will not place is answered by its reason: a destination
/// it bounds, one taken, or bytes that are not the file named.
#[tokio::test]
async fn a_fetch_the_host_refuses_is_answered_by_its_reason_and_replaces_nothing() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let leased = leased(&host, &fixture).await;
    let bytes = pattern(1000, 5);
    host.file("notes.txt", b"the agent's own");
    let outside = host.root.path().join("secret");
    for (destination, reason) in [
        (outside.to_str().unwrap(), FetchRefusal::OutsideWorkspace),
        ("../secret", FetchRefusal::OutsideWorkspace),
        ("missing/a.bin", FetchRefusal::NoDirectory),
        ("notes.txt", FetchRefusal::Occupied),
    ] {
        let ask = fetch_ask(destination, destination, &bytes, Held::of(&bytes));
        let delivered = ask.delivered.clone();
        assert_eq!(
            fetched(&leased, ask).await,
            Err(FetchFailure::Refused(reason)),
            "{destination}"
        );
        assert!(!delivered.load(Ordering::SeqCst), "{destination}");
    }
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
    assert_eq!(
        std::fs::read(host.workspace.join("notes.txt")).unwrap(),
        b"the agent's own"
    );
    assert!(!outside.exists());
    // Bytes that are not the file named never land.
    let other = pattern(1000, 6);
    assert_eq!(
        fetched(
            &leased,
            fetch_ask("call-x", "a.bin", &bytes, Held::of(&other))
        )
        .await,
        Err(FetchFailure::Refused(FetchRefusal::Mismatch))
    );
    assert!(!host.workspace.join("a.bin").exists());
    assert_eq!(parts(&host), 0);
}

/// A fetch asked once the lease ended, or once its connection was lost, is
/// answered so at once; one under way when the connection is lost ends as
/// the lease ended, with nothing placed.
#[tokio::test]
async fn a_fetch_after_or_during_the_leases_end_is_answered_so_and_places_nothing() {
    let bytes = pattern(1000, 7);
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let ended = leased(&host, &fixture).await;
    ended.held.hold.end(LeaseEndCause::Stopped).await;
    assert_eq!(
        fetched(
            &ended,
            fetch_ask("call-1", "a.bin", &bytes, Held::of(&bytes))
        )
        .await,
        Err(FetchFailure::Refused(FetchRefusal::LeaseEnded))
    );

    let host = Host::new();
    let lost = leased(&host, &fixture).await;
    host.cut();
    tokio::time::timeout(Duration::from_secs(10), lost.held.hold.ends().unwrap())
        .await
        .expect("the loss is seen");
    assert_eq!(
        fetched(
            &lost,
            fetch_ask("call-1", "a.bin", &bytes, Held::of(&bytes))
        )
        .await,
        Err(FetchFailure::Unanswered)
    );

    let host = Host::new();
    let cut = leased(&host, &fixture).await;
    host.script(Channel::Silent);
    let fetch = cut
        .held
        .hold
        .fetch(fetch_ask("call-1", "a.bin", &bytes, Held::of(&bytes)))
        .unwrap();
    let fetch = tokio::spawn(fetch);
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.channels.opens.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the write began");
    host.cut();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), fetch)
            .await
            .expect("answered in time")
            .unwrap(),
        Err(FetchFailure::Refused(FetchRefusal::LeaseEnded))
    );
    assert!(!host.workspace.join("a.bin").exists());
}

/// A call that stops waiting after its Fetch is sent and before the host
/// accepts it leaves nothing behind: its drop told the host `Undelivered`,
/// the host lets the part go, and its place under the in-flight bound is
/// free again, however often that happens.
#[tokio::test]
async fn a_fetch_given_up_before_its_acceptance_leaves_no_part_and_frees_its_place() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let leased = leased(&host, &fixture).await;
    let bytes = pattern(1000, 8);
    let ledger = host.root.path().join("leases.jsonl");
    // Twice as many as the bound, one after another: each is let go before
    // the next is asked, so none is ever refused busy.
    for n in 0..2 * nessa_protocol::lease::MAX_FETCHES_IN_FLIGHT {
        let request = format!("gone-{n}");
        let mut fetch = leased
            .held
            .hold
            .fetch(fetch_ask(&request, "a.bin", &bytes, Held::of(&bytes)))
            .unwrap();
        // Polled once, so its Fetch is queued, then dropped unanswered.
        tokio::select! {
            biased;
            _ = &mut fetch => panic!("answered before the host could accept it"),
            () = std::future::ready(()) => {}
        }
        drop(fetch);
        let unanswered = format!(
            r#""kind":"fetch_unanswered","lease":"{}","request":"{request}#"#,
            leased.lease.as_str()
        );
        tokio::time::timeout(Duration::from_secs(5), async {
            while !std::fs::read_to_string(&ledger)
                .unwrap_or_default()
                .contains(&unanswered)
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            panic!(
                "{request} is recorded unanswered: {}",
                std::fs::read_to_string(&ledger).unwrap_or_default()
            )
        });
        parts_let_go(&host).await;
    }
    // As many as the bound, at once: each one is taken.
    let fetches: Vec<_> = (0..nessa_protocol::lease::MAX_FETCHES_IN_FLIGHT)
        .map(|n| {
            tokio::spawn(
                leased
                    .held
                    .hold
                    .fetch(fetch_ask(
                        &format!("kept-{n}"),
                        &format!("kept-{n}.bin"),
                        &bytes,
                        Held::of(&bytes),
                    ))
                    .unwrap(),
            )
        })
        .collect();
    for (n, fetch) in fetches.into_iter().enumerate() {
        assert!(
            matches!(
                tokio::time::timeout(Duration::from_secs(20), fetch)
                    .await
                    .unwrap()
                    .unwrap(),
                Ok(FetchPlaced::Placed { .. })
            ),
            "kept-{n}"
        );
    }
    assert!(!host.workspace.join("a.bin").exists());
    assert_eq!(parts(&host), 0);
}

/// A call given up once its acceptance has arrived but before it read it
/// leaves nothing behind either: the host lets the part go and records it
/// unanswered, and its place is free again, however often that happens.
#[tokio::test]
async fn a_fetch_given_up_with_its_acceptance_unread_lets_its_part_go() {
    let host = Host::new();
    let fixture = Fixture::new(AttachmentLimits::default());
    let leased = leased(&host, &fixture).await;
    let bytes = pattern(1000, 5);
    let ledger = host.root.path().join("leases.jsonl");
    let recorded = |kind: &str, request: &str| {
        let line = format!(
            r#""kind":"{kind}","lease":"{}","request":"{request}#"#,
            leased.lease.as_str()
        );
        let ledger = ledger.clone();
        async move {
            tokio::time::timeout(Duration::from_secs(5), async {
                while !std::fs::read_to_string(&ledger)
                    .unwrap_or_default()
                    .contains(&line)
                {
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
            })
            .await
            .unwrap_or_else(|_| panic!("{line} is recorded"));
        }
    };
    // Twice as many as the bound, one after another: a part kept would
    // hold its place, and a later one would be refused busy.
    for n in 0..2 * nessa_protocol::lease::MAX_FETCHES_IN_FLIGHT {
        let request = format!("unread-{n}");
        let mut fetch = leased
            .held
            .hold
            .fetch(fetch_ask(&request, "a.bin", &bytes, Held::of(&bytes)))
            .unwrap();
        // Polled once, so its Fetch is queued and it waits for an answer.
        tokio::select! {
            biased;
            _ = &mut fetch => panic!("answered before the host could accept it"),
            () = std::future::ready(()) => {}
        }
        // The host accepted it, and its acceptance is given time to reach
        // the call's waiter, never read since the call is not polled.
        recorded("fetch_accepted", &request).await;
        tokio::time::sleep(Duration::from_millis(50)).await;
        drop(fetch);
        recorded("fetch_unanswered", &request).await;
        parts_let_go(&host).await;
    }
    assert!(!host.workspace.join("a.bin").exists());
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
}

/// A paired connection, which has no artifact channel to write a file
/// over, is never asked to fetch one: the agent is told it is unavailable
/// there before any frame goes, and the host makes and records nothing.
#[tokio::test]
async fn a_paired_connection_is_never_asked_to_fetch() {
    let host = Host::paired();
    let fixture = Fixture::new(AttachmentLimits::default());
    let leased = leased(&host, &fixture).await;
    let bytes = pattern(1000, 3);
    assert!(leased
        .held
        .hold
        .fetch(fetch_ask("call-2", "a.bin", &bytes, Held::of(&bytes)))
        .is_none());
    assert_eq!(parts(&host), 0);
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
    let ledger = std::fs::read_to_string(host.root.path().join("leases.jsonl")).unwrap_or_default();
    assert!(!ledger.contains("call-2"), "{ledger}");
    assert!(!host.workspace.join("a.bin").exists());
}

// ---- a paired connection: the lease frames carry the bytes ----

/// Row I9: on a paired connection, which has no artifact channel, a file a
/// harness publishes and one the gateway asks for travel on the lease
/// frames themselves, are verified by their digest and held under the
/// lease; no channel is ever opened, and the host lets go of its copies.
#[tokio::test]
async fn a_paired_connection_carries_a_published_file_on_its_lease_frames_and_it_verifies() {
    let host = Host::paired();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let image = pattern(3 * 1024 * 1024 + 17, 1);
    let dmg = host.file("Nessa.dmg", &image);
    let before = host.carried.load(Ordering::SeqCst);
    assert_eq!(
        host.publish(&dmg).await,
        PublishAnswer::Held {
            digest: hex(&image),
            size: image.len() as u64,
        }
    );
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));
    // Its bytes came over the lease stream: no channel was opened.
    let carried = host.carried.load(Ordering::SeqCst) - before;
    assert!(
        carried >= image.len() as u64,
        "the lease stream carried {carried} bytes of a file of {}",
        image.len()
    );
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
    // One the gateway asks for, ending short of a whole range, too.
    let report = pattern(70_000, 2);
    host.file("report.pdf", &report);
    let publish = leased
        .held
        .hold
        .publish(ask("call-1", "report.pdf"))
        .expect("a paired lease publishes");
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), publish)
            .await
            .expect("offered in time"),
        Ok(())
    );
    assert_eq!(leased.kept.recv().await, Some(Ok(ArtifactKept::Held)));
    assert_eq!(host.channels.opens.load(Ordering::SeqCst), 0);
    let held = fixture.store.held();
    assert_eq!(held.len(), 2);
    for bytes in [&image, &report] {
        let hold = held
            .iter()
            .find(|hold| hold.stored().digest() == attachments::digest_of(bytes))
            .expect("held");
        assert_eq!(hold.lease(), Some(leased.lease.as_str()));
        assert_eq!(
            fixture.store.blob(attachments::digest_of(bytes)).as_deref(),
            Some(bytes.as_slice())
        );
    }
    // The host recorded each publish and the gateway's answer to it.
    let ledger = std::fs::read_to_string(host.root.path().join("leases.jsonl")).unwrap();
    let kinds: Vec<(String, serde_json::Value)> = ledger
        .lines()
        .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
        .filter(|entry| entry["lease"] == leased.lease.as_str())
        .filter(|entry| entry["kind"] == "published" || entry["kind"] == "collected")
        .map(|entry| {
            (
                entry["kind"].as_str().unwrap().to_owned(),
                entry["outcome"].clone(),
            )
        })
        .collect();
    assert_eq!(
        kinds,
        [
            ("published".into(), serde_json::Value::Null),
            ("collected".into(), serde_json::json!({"outcome": "held"})),
            ("published".into(), serde_json::Value::Null),
            ("collected".into(), serde_json::json!({"outcome": "held"})),
        ],
        "{ledger}"
    );
    assert_eq!(staged(&host), 0);
}

/// A paired connection lost mid-read ends the lease, and the read with it:
/// the file is refused as ended, with nothing held, and the host, who can
/// no longer hear that, records its publish unanswered.
#[tokio::test]
async fn a_paired_lease_lost_mid_read_fails_as_ended_and_leaves_no_partial_hold() {
    let host = Host::paired();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let file = host.file("Nessa.dmg", &pattern(1024 * 1024, 5));
    let limit = host.carried.load(Ordering::SeqCst) + 100_000;
    host.relay_limit.send_replace(limit);
    let publisher = {
        let host = host.clone();
        tokio::spawn(async move { host.publish(&file).await })
    };
    // Wait until the read is under way, its bytes held back by the relay.
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.carried.load(Ordering::SeqCst) <= limit {
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
    assert_eq!(publisher.await.unwrap(), PublishAnswer::Unanswered);
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(staged(&host), 0);
}

/// A staged copy that is cut short while the gateway reads it is not the
/// file published: refused as a mismatch, nothing held, and the host told.
#[tokio::test]
async fn a_staged_copy_cut_short_mid_read_on_a_paired_connection_is_a_mismatch() {
    let host = Host::paired();
    let fixture = Fixture::new(AttachmentLimits::default());
    let mut leased = leased(&host, &fixture).await;
    let file = host.file("Nessa.dmg", &pattern(1024 * 1024, 6));
    // The gateway asks at most a few ranges past what reached it, so with
    // what reaches it held under this, nothing past half the file is read
    // before the copy is cut there.
    let limit = host.carried.load(Ordering::SeqCst) + 100_000;
    host.relay_limit.send_replace(limit);
    let publisher = {
        let host = host.clone();
        tokio::spawn(async move { host.publish(&file).await })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while host.carried.load(Ordering::SeqCst) <= limit {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the read started");
    let copies = files_under(&host.root.path().join("outbox"));
    let [copy] = copies.keys().collect::<Vec<_>>()[..] else {
        panic!("one staged copy, got {copies:?}")
    };
    std::fs::OpenOptions::new()
        .write(true)
        .open(copy)
        .unwrap()
        .set_len(512 * 1024)
        .unwrap();
    host.relay_limit.send_replace(u64::MAX);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(10), leased.kept.recv())
            .await
            .expect("an outcome in time"),
        Some(Err(CollectionRefusal::Mismatch))
    );
    assert_eq!(
        publisher.await.unwrap(),
        PublishAnswer::Refused {
            reason: CollectionRefusal::Mismatch
        }
    );
    assert!(fixture.store.held().is_empty());
    assert_eq!(fixture.store.pending(), 0);
    assert_eq!(staged(&host), 0);
}
