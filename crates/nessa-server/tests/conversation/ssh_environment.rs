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
        Build, BuildSource, HostInstaller, InstallTimings, OwnExecutable, ReleaseCopy, RemoteShell,
        ShellFuture,
    },
    link::{HostLink, Liveness},
    open_ssh::OpenSshConnector,
};
use crate::conversation::application::{Environment, GrantedTerms, LeaseRelease};
use crate::env::{LEASE_PROTOCOL, VERSION};
use crate::env_serve::application::FrameStream;
use crate::env_serve::application::{
    serve, CommandRan, CommandRunner, CommandStop, EndCause, HarnessLauncher, LeaseLedger,
    LedgerEntry, PeerGrants, Runners, ServeTimings, Serving,
};
use crate::env_serve::domain::{Agents, CommandGrant, Peer, PeerGrant};
use crate::env_serve::install::Platform;
use crate::timer::TokioTimer;
use nessa_protocol::lease::{
    decode, encode, Cleanup, Collection, CommandEnd, Data, FromEnvironment,
    GrantedTerms as WireTerms, Sandbox, StagedArtifact, ToEnvironment, COMMAND_STOP_WAIT,
    KEEPALIVE_INTERVAL, SILENCE_LIMIT,
};
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
use nessa_sdk::application::agent_execution::providers::{ApprovalMode, ModelChange};
use nessa_sdk::application::agent_execution::traces::LocalTraceStore;
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{
        AgentProvider, CloseOutcome, HarnessCleanupFuture, HarnessControl, HarnessHost,
        HarnessLaunch, HarnessProcess, ProviderIdentity, ProviderOpenFuture, ProviderOpenRequest,
    },
};
use nessa_sdk::domain::model_metadata::value_objects::EffortLevel;
use nessa_sdk::domain::{
    agent_execution::leases::{
        AgentWork, EnvironmentRef, LeaseCleanup, LeaseDeadline, LeaseEndCause, LeaseGrants,
        LeaseId, LeaseRefusal, LeaseTerms, LeaseWork, SandboxProfile, SandboxProfiles,
        SshDestination,
    },
    agent_execution::leases::{CommandExit, CommandRefusal, CommandWork},
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
    fn sandboxes(&self) -> SandboxProfiles {
        SandboxProfiles::HARNESS_DEFAULT
    }
    fn launch(
        &self,
        _agent: &str,
        _environment: &BTreeMap<String, String>,
        _publish_point: Option<&str>,
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

/// What the host's owner grants SSH: its entry, as a test sets it; none
/// keeps everything.
#[derive(Default)]
struct SshGrant(Mutex<Option<PeerGrant>>);
impl PeerGrants for SshGrant {
    fn entry(&self, peer: &Peer) -> Result<Option<PeerGrant>, String> {
        Ok(match peer {
            Peer::Ssh => self.0.lock().unwrap().clone(),
            Peer::Gateway { .. } => None,
        })
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
    /// A host that grants the first command lease it is asked for, then
    /// says it is ending it, as no host may, and keeps reading.
    EndsItsCommand,
    /// A host that grants a command, and on Run publishes one file and then
    /// says it ran. It answers reads of that file and ends the lease when
    /// asked. Every frame it reads is kept.
    PublishesOnRun,
    /// A host that grants a command and, on Run, says it ran. It keeps
    /// reading, keepalives included, and never answers `Ended`.
    AnswersRan,
    /// A host that grants every lease, says it is ending one as expired at
    /// once when `early`, and, told it is revoked, says each is ending as
    /// expired and then ended; keeps every frame it reads.
    Expiring { early: bool },
    /// A host that answers each Grant it reads only once `gate` opens,
    /// granting it, and, told it is revoked, says each lease it granted
    /// ended; keeps every frame it reads.
    GrantingLate(tokio::sync::watch::Receiver<bool>),
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
    /// How a serving host runs commands; `None` runs none.
    runner: Mutex<Option<Arc<dyn CommandRunner>>>,
    /// What a serving host's owner grants this gateway, over SSH.
    grants: Arc<SshGrant>,
    /// Asks every serving host to drain.
    drain: tokio::sync::watch::Sender<bool>,
    end_pause: Mutex<Option<Arc<PreparationEndPause>>>,
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
            runner: Mutex::new(None),
            grants: Arc::default(),
            drain: tokio::sync::watch::channel(false).0,
            end_pause: Mutex::new(None),
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
                    Serving {
                        peer: Peer::Ssh,
                        grants: self.grants.clone(),
                        policy: Arc::new(
                            crate::env_serve::infrastructure::CedarLeasePolicy::new().unwrap(),
                        ),
                    },
                    Runners {
                        harnesses: self.launcher.clone(),
                        commands: self.runner.lock().unwrap().clone(),
                    },
                    self.ledger.clone(),
                    Arc::new(NoOutbox),
                    None,
                    ServeTimings {
                        grace: Duration::from_millis(10),
                        kill: Duration::from_millis(100),
                        ..ServeTimings::default()
                    },
                    Arc::new(TokioTimer::new()),
                    self.drain.subscribe(),
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
                            let granted = encode(&FromEnvironment::Granted {
                                lease,
                                terms: WireTerms {
                                    sandbox: Sandbox::HarnessDefault,
                                    timeout_ms: None,
                                    lease_ms: None,
                                },
                            })
                            .unwrap();
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
            Reach::EndsItsCommand => {
                tokio::spawn(async move {
                    let (host_in, mut host_out) = split(host);
                    let mut stream = FrameStream::new(host_in);
                    let hello = encode(&FromEnvironment::Hello {
                        build: VERSION.into(),
                        protocol: LEASE_PROTOCOL.into(),
                        workspace: "/srv/work".into(),
                    })
                    .unwrap();
                    host_out.write_all(&hello).await.unwrap();
                    while let Ok(Some(body)) = stream.next().await {
                        if let Ok(ToEnvironment::GrantCommand {
                            lease, timeout_ms, ..
                        }) = decode(&body)
                        {
                            for frame in [
                                FromEnvironment::Granted {
                                    lease: lease.clone(),
                                    terms: WireTerms {
                                        sandbox: Sandbox::None,
                                        timeout_ms: Some(timeout_ms),
                                        lease_ms: None,
                                    },
                                },
                                FromEnvironment::Ending {
                                    lease,
                                    cause: nessa_protocol::lease::EndingCause::Expired,
                                },
                            ] {
                                host_out.write_all(&encode(&frame).unwrap()).await.unwrap();
                            }
                        }
                    }
                });
            }
            Reach::PublishesOnRun => {
                let frames = self.frames.clone();
                tokio::spawn(async move {
                    let (host_in, mut host_out) = split(host);
                    let mut stream = FrameStream::new(host_in);
                    let hello = encode(&FromEnvironment::Hello {
                        build: VERSION.into(),
                        protocol: LEASE_PROTOCOL.into(),
                        workspace: "/srv/work".into(),
                    })
                    .unwrap();
                    host_out.write_all(&hello).await.unwrap();
                    let file = b"note";
                    while let Ok(Some(body)) = stream.next().await {
                        let Ok(frame) = decode::<ToEnvironment>(&body) else {
                            continue;
                        };
                        frames.lock().unwrap().push(frame.clone());
                        let said = match frame {
                            ToEnvironment::GrantCommand {
                                lease, timeout_ms, ..
                            } => Some(FromEnvironment::Granted {
                                lease,
                                terms: WireTerms {
                                    sandbox: Sandbox::None,
                                    timeout_ms: Some(timeout_ms),
                                    lease_ms: None,
                                },
                            }),
                            ToEnvironment::Run { lease } => {
                                host_out
                                    .write_all(
                                        &encode(&FromEnvironment::Published {
                                            lease: lease.clone(),
                                            artifact: 0,
                                            request: None,
                                            file: StagedArtifact {
                                                name: "note.txt".into(),
                                                media_type: "text/plain".into(),
                                                size: file.len() as u64,
                                                digest: "ab".repeat(32),
                                                path: "/srv/work/note.txt".into(),
                                            },
                                        })
                                        .unwrap(),
                                    )
                                    .await
                                    .unwrap();
                                Some(FromEnvironment::Ran {
                                    lease,
                                    end: CommandEnd::Exited { code: 0 },
                                    stdout: Data(Vec::new()),
                                    stderr: Data(Vec::new()),
                                    dropped_bytes: 0,
                                    cleanup: Cleanup::Confirmed { forced: false },
                                })
                            }
                            ToEnvironment::ReadArtifact {
                                lease,
                                artifact,
                                offset,
                                length,
                            } => {
                                let start = usize::try_from(offset).unwrap_or(file.len());
                                let len = usize::try_from(length).unwrap_or(0);
                                let end = start.saturating_add(len).min(file.len());
                                let start = start.min(file.len());
                                Some(FromEnvironment::ArtifactRead {
                                    lease,
                                    artifact,
                                    offset,
                                    data: Data(file[start..end].to_vec()),
                                })
                            }
                            ToEnvironment::End { lease } => Some(FromEnvironment::Ended {
                                lease,
                                cleanup: Cleanup::Confirmed { forced: false },
                            }),
                            _ => None,
                        };
                        if let Some(said) = said {
                            host_out.write_all(&encode(&said).unwrap()).await.unwrap();
                        }
                    }
                });
            }
            Reach::AnswersRan => {
                tokio::spawn(async move {
                    let (host_in, mut host_out) = split(host);
                    let mut stream = FrameStream::new(host_in);
                    let hello = encode(&FromEnvironment::Hello {
                        build: VERSION.into(),
                        protocol: LEASE_PROTOCOL.into(),
                        workspace: "/srv/work".into(),
                    })
                    .unwrap();
                    host_out.write_all(&hello).await.unwrap();
                    while let Ok(Some(body)) = stream.next().await {
                        let Ok(frame) = decode::<ToEnvironment>(&body) else {
                            continue;
                        };
                        let said = match frame {
                            ToEnvironment::GrantCommand {
                                lease, timeout_ms, ..
                            } => Some(FromEnvironment::Granted {
                                lease,
                                terms: WireTerms {
                                    sandbox: Sandbox::None,
                                    timeout_ms: Some(timeout_ms),
                                    lease_ms: None,
                                },
                            }),
                            ToEnvironment::Run { lease } => Some(FromEnvironment::Ran {
                                lease,
                                end: CommandEnd::Exited { code: 0 },
                                stdout: Data(Vec::new()),
                                stderr: Data(Vec::new()),
                                dropped_bytes: 0,
                                cleanup: Cleanup::Confirmed { forced: false },
                            }),
                            ToEnvironment::Keepalive | ToEnvironment::End { .. } => None,
                            _ => None,
                        };
                        if let Some(said) = said {
                            host_out.write_all(&encode(&said).unwrap()).await.unwrap();
                        }
                    }
                });
            }
            Reach::Expiring { early } => {
                let frames = self.frames.clone();
                tokio::spawn(async move {
                    let (host_in, mut host_out) = split(host);
                    let mut stream = FrameStream::new(host_in);
                    let hello = encode(&FromEnvironment::Hello {
                        build: VERSION.into(),
                        protocol: LEASE_PROTOCOL.into(),
                        workspace: "/srv/work".into(),
                    })
                    .unwrap();
                    host_out.write_all(&hello).await.unwrap();
                    let mut granted = Vec::new();
                    while let Ok(Some(body)) = stream.next().await {
                        let Ok(frame) = decode::<ToEnvironment>(&body) else {
                            continue;
                        };
                        frames.lock().unwrap().push(frame.clone());
                        let ending = |lease: &str| FromEnvironment::Ending {
                            lease: lease.into(),
                            cause: nessa_protocol::lease::EndingCause::Expired,
                        };
                        let mut said = Vec::new();
                        match frame {
                            ToEnvironment::Grant { lease, .. } => {
                                said.push(FromEnvironment::Granted {
                                    lease: lease.clone(),
                                    terms: WireTerms {
                                        sandbox: Sandbox::HarnessDefault,
                                        timeout_ms: None,
                                        lease_ms: None,
                                    },
                                });
                                if early {
                                    said.push(ending(&lease));
                                }
                                granted.push(lease);
                            }
                            ToEnvironment::Revoked => {
                                for lease in &granted {
                                    said.push(ending(lease));
                                    said.push(FromEnvironment::Ended {
                                        lease: lease.clone(),
                                        cleanup: Cleanup::Confirmed { forced: false },
                                    });
                                }
                            }
                            _ => {}
                        }
                        for frame in said {
                            host_out.write_all(&encode(&frame).unwrap()).await.unwrap();
                        }
                    }
                });
            }
            Reach::GrantingLate(mut gate) => {
                let frames = self.frames.clone();
                tokio::spawn(async move {
                    let (host_in, mut host_out) = split(host);
                    let mut stream = FrameStream::new(host_in);
                    let hello = encode(&FromEnvironment::Hello {
                        build: VERSION.into(),
                        protocol: LEASE_PROTOCOL.into(),
                        workspace: "/srv/work".into(),
                    })
                    .unwrap();
                    host_out.write_all(&hello).await.unwrap();
                    let mut granted = Vec::new();
                    while let Ok(Some(body)) = stream.next().await {
                        let Ok(frame) = decode::<ToEnvironment>(&body) else {
                            continue;
                        };
                        frames.lock().unwrap().push(frame.clone());
                        let mut said = Vec::new();
                        match frame {
                            ToEnvironment::Grant { lease, .. } => {
                                let _ = gate.wait_for(|open| *open).await;
                                said.push(FromEnvironment::Granted {
                                    lease: lease.clone(),
                                    terms: WireTerms {
                                        sandbox: Sandbox::HarnessDefault,
                                        timeout_ms: None,
                                        lease_ms: None,
                                    },
                                });
                                granted.push(lease);
                            }
                            ToEnvironment::Revoked => {
                                for lease in &granted {
                                    said.push(FromEnvironment::Ended {
                                        lease: lease.clone(),
                                        cleanup: Cleanup::Confirmed { forced: false },
                                    });
                                }
                            }
                            ToEnvironment::End { lease } if granted.contains(&lease) => {
                                granted.retain(|granted| *granted != lease);
                                said.push(FromEnvironment::Ended {
                                    lease,
                                    cleanup: Cleanup::Confirmed { forced: false },
                                });
                            }
                            _ => {}
                        }
                        for frame in said {
                            host_out.write_all(&encode(&frame).unwrap()).await.unwrap();
                        }
                    }
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
            match self.end_pause.lock().unwrap().clone() {
                Some(pause) => relay_preparation_end(gateway_end_in, host_end_out, pause),
                None => relay(gateway_end_in, host_end_out),
            },
            relay(host_end_in, gateway_end_out),
        ));
        let (from_environment, to_environment) = split(gateway);
        Ok(LeaseConnection {
            from_environment: Box::new(from_environment),
            to_environment: Box::new(to_environment),
            keep: Box::new(()),
            artifacts: super::connector::ArtifactTransport::Channels(Arc::new(NoChannels)),
        })
    }
}

/// An SSH connection whose artifact channels never open: these tests
/// carry no file.
struct NoChannels;
impl super::connector::ArtifactChannels for NoChannels {
    fn open(&self) -> io::Result<super::connector::ArtifactChannel> {
        Err(io::Error::other("no artifact channels"))
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
    captured_host: Option<Arc<dyn HarnessHost>>,
    capabilities: EffectiveCapabilities,
    host: Arc<Mutex<Option<Arc<dyn HarnessHost>>>>,
    remote: bool,
}
impl AgentProvider for Binding {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.captured_host
            .as_ref()
            .map_or_else(HarnessHostIdentity::local, HarnessHostIdentity::hosted)
    }
    fn model_change_context(&self) -> Option<String> {
        Some("fixture-host-context".into())
    }
    fn with_model_configuration(
        &self,
        mode: Option<ApprovalMode>,
        effort: Option<EffortLevel>,
        _traces: Option<Arc<LocalTraceStore>>,
    ) -> Result<Arc<dyn AgentProvider>, AgentError> {
        if mode.is_some() || effort.is_some() {
            return Err(AgentError::Unsupported("fixture settings".into()));
        }
        Ok(Arc::new(Self {
            captured_host: self.captured_host.clone(),
            capabilities: self.capabilities.clone(),
            host: self.host.clone(),
            remote: self.remote,
        }))
    }
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("fixture", self.capabilities.model().model_id(), "context")
            .expect("a fixed identity is valid")
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
        *self.host.lock().unwrap() = Some(host.clone());
        Ok(Arc::new(Binding {
            captured_host: Some(host),
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
            captured_host: None,
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
        cause: EndCause::Asked,
    }));
    // The lease's end resolves its watch: nothing is left waiting on it.
    tokio::time::timeout(
        Duration::from_secs(5),
        opened.hold.ends().expect("a host's lease can end there"),
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
    let lost = opened.hold.ends().expect("a host's lease can be lost");
    connector.cut();
    let cause = tokio::time::timeout(Duration::from_secs(5), lost)
        .await
        .expect("the loss is seen");
    assert_eq!(cause, LeaseEndCause::Lost);
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
            cause: EndCause::Lost,
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
    let lost = opened.hold.ends().unwrap();
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
        terms: WireTerms {
            sandbox: Sandbox::HarnessDefault,
            timeout_ms: None,
            lease_ms: None,
        },
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

/// Row I7 and L9: a host saying it is ending a lease this gateway does not
/// hold is dropped and recorded, never applied to another lease.
#[tokio::test]
async fn an_ending_for_a_lease_not_held_is_dropped_with_evidence() {
    let ending = serde_json::to_vec(&FromEnvironment::Ending {
        lease: "never-granted".into(),
        cause: nessa_protocol::lease::EndingCause::Expired,
    })
    .unwrap();
    let connector = Connector::new(Reach::Scripted(vec![hello_body(LEASE_PROTOCOL), ending]));
    let audit = Arc::new(Audit::default());
    let environment = environment(connector, audit.clone());
    // The grant is never answered by this host; the opening is bounded.
    let opening = tokio::spawn(async move {
        environment
            .open(&lease(), &terms("claude"), binding_only())
            .await
            .err()
    });
    let drop = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let drop = audit.events().into_iter().find_map(|event| match event {
                EnvironmentEvent::FrameDropped { lease, frame, .. } => Some((lease, frame)),
                _ => None,
            });
            if let Some(drop) = drop {
                break drop;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the drop recorded");
    assert_eq!(drop, (Some("never-granted".into()), "ending".into()));
    opening.abort();
}

/// Row I7: a command's lease is bounded by its timeout alone, so a host
/// saying it is ending one is dropped and recorded, never applied to it.
#[tokio::test]
async fn an_ending_for_a_command_lease_is_dropped_with_evidence() {
    let audit = Arc::new(Audit::default());
    let host = environment(Connector::new(Reach::EndsItsCommand), audit.clone());
    let id = lease();
    let _hold = host
        .commands()
        .unwrap()
        .grant(&id, &work(&["ls"]))
        .await
        .expect("granted")
        .hold;
    let drop = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            let drop = audit.events().into_iter().find_map(|event| match event {
                EnvironmentEvent::FrameDropped { lease, frame, .. } => Some((lease, frame)),
                _ => None,
            });
            if let Some(drop) = drop {
                break drop;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the drop recorded");
    assert_eq!(drop, (Some(id.as_str().to_owned()), "ending".into()));
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
    let arguments = ssh_arguments(&host, crate::env_serve::install::serve_command(&[&digest]));
    let separator = arguments.iter().position(|word| word == "--").unwrap();
    assert_eq!(
        &arguments[separator + 1..],
        [
            "devbox".to_owned(),
            format!(
                "sh -c 'for d in {digest}; do p=\"$HOME/.nessa/env/$d/nessa\"; \
                 if [ -x \"$p\" ]; then exec \"$p\" env serve; fi; done'"
            )
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
        .ends()
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

/// A fetch given up just before its lease's End is asked tells the host
/// so first: its `Undelivered` is queued ahead of the End, so the host never
/// settles it as answered by the lease's end.
#[tokio::test]
async fn a_fetch_given_up_just_before_its_lease_ends_is_undelivered_before_the_end() {
    use crate::conversation::application::{
        EnvironmentFuture, FetchAsk, HeldSource, HeldSourceFailure,
    };
    use nessa_sdk::application::agent_execution::sessions::PublishedFile;
    use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
    /// Never read: the fetch is given up before the host accepts it.
    struct Unread;
    impl HeldSource for Unread {
        fn read(
            &self,
            _offset: u64,
            _max: usize,
        ) -> EnvironmentFuture<'_, Result<Vec<u8>, HeldSourceFailure>> {
            Box::pin(async { Err(HeldSourceFailure::NotHeld) })
        }
    }
    let (_open, gate) = tokio::sync::watch::channel(true);
    let connector = Connector::new(Reach::Stalling(gate));
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let (binding, _host) = binding(true);
    let id = lease();
    let opened = environment
        .open(&id, &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    let mut fetch = opened
        .hold
        .fetch(FetchAsk {
            request: "given-up".into(),
            destination: "a.bin".into(),
            file: PublishedFile::new(
                Sha256Digest::parse(&format!("sha256:{}", "0".repeat(64))).unwrap(),
                MediaType::parse("application/octet-stream").unwrap(),
                1,
            )
            .unwrap(),
            source: Arc::new(Unread),
            delivered: Arc::default(),
        })
        .expect("a host's lease fetches");
    // Polled once, so its Fetch is queued; then given up, and the lease's
    // End asked at once, with nothing awaited between.
    tokio::select! {
        biased;
        _ = &mut fetch => panic!("answered before the host could accept it"),
        () = std::future::ready(()) => {}
    }
    drop(fetch);
    let _ = tokio::time::timeout(
        Duration::from_millis(100),
        opened.hold.end(LeaseEndCause::Closed),
    )
    .await;
    let (undelivered, end) = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            {
                let frames = connector.frames.lock().unwrap();
                let undelivered = frames.iter().position(|frame| {
                    matches!(frame, ToEnvironment::Undelivered { request, .. } if request.starts_with("given-up#"))
                });
                let end = frames.iter().position(
                    |frame| matches!(frame, ToEnvironment::End { lease } if lease == id.as_str()),
                );
                if let (Some(undelivered), Some(end)) = (undelivered, end) {
                    break (undelivered, end);
                }
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the Undelivered and the End reach the host");
    assert!(undelivered < end, "the Undelivered goes ahead of the End");
}

/// A fetch asked again under the call of one given up before the host's
/// last word is a new ask with a name of its own on the wire, so nothing
/// the host says of the first can be taken as the second's.
#[tokio::test]
async fn a_fetch_asked_again_under_a_call_given_up_is_named_apart_on_the_wire() {
    use crate::conversation::application::{
        EnvironmentFuture, FetchAsk, HeldSource, HeldSourceFailure,
    };
    use nessa_sdk::application::agent_execution::sessions::PublishedFile;
    use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
    /// Never read: each fetch is given up before the host accepts it.
    struct Unread;
    impl HeldSource for Unread {
        fn read(
            &self,
            _offset: u64,
            _max: usize,
        ) -> EnvironmentFuture<'_, Result<Vec<u8>, HeldSourceFailure>> {
            Box::pin(async { Err(HeldSourceFailure::NotHeld) })
        }
    }
    let ask = || FetchAsk {
        request: "reused".into(),
        destination: "a.bin".into(),
        file: PublishedFile::new(
            Sha256Digest::parse(&format!("sha256:{}", "0".repeat(64))).unwrap(),
            MediaType::parse("application/octet-stream").unwrap(),
            1,
        )
        .unwrap(),
        source: Arc::new(Unread),
        delivered: Arc::default(),
    };
    let (_open, gate) = tokio::sync::watch::channel(true);
    let connector = Connector::new(Reach::Stalling(gate));
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let (binding, _host) = binding(true);
    let opened = environment
        .open(&lease(), &terms("claude"), binding)
        .await
        .unwrap_or_else(|_| panic!("granted"));
    for _ in 0..2 {
        let mut fetch = opened.hold.fetch(ask()).expect("a host's lease fetches");
        // Polled once, so its Fetch is queued; then given up unanswered.
        tokio::select! {
            biased;
            _ = &mut fetch => panic!("answered before the host could accept it"),
            () = std::future::ready(()) => {}
        }
        drop(fetch);
    }
    let named = tokio::time::timeout(Duration::from_secs(10), async {
        loop {
            let named: Vec<String> = connector
                .frames
                .lock()
                .unwrap()
                .iter()
                .filter_map(|frame| match frame {
                    ToEnvironment::Fetch { request, .. } => Some(request.clone()),
                    _ => None,
                })
                .collect();
            if named.len() == 2 {
                break named;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("both are asked");
    assert!(
        named.iter().all(|name| name.starts_with("reused#")),
        "{named:?}"
    );
    assert_ne!(named[0], named[1], "each ask has a name of its own");
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
        fn sandboxes(&self) -> SandboxProfiles {
            SandboxProfiles::HARNESS_DEFAULT
        }
        fn launch(
            &self,
            _agent: &str,
            _environment: &BTreeMap<String, String>,
            _publish_point: Option<&str>,
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
            ..SshTimings::default()
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

/// A host that goes quiet without closing its stream, as one cut off by the
/// network, is taken for gone past the silence bound: what waits on it is
/// answered then, not at its own longer bound.
#[tokio::test]
async fn a_host_silent_past_its_bound_is_gone() {
    let connector = Connector::new(Reach::Scripted(vec![hello_body(LEASE_PROTOCOL)]));
    let environment = SshEnvironment::new(
        devbox(),
        connector,
        Arc::new(Audit::default()),
        no_install(),
        SshTimings {
            connect: Duration::from_secs(30),
            answer: Duration::from_secs(30),
            silence: Duration::from_millis(50),
            ..SshTimings::default()
        },
    );
    // The grant is never answered, and the stream stays open.
    let refused = tokio::time::timeout(
        Duration::from_secs(10),
        environment.open(&lease(), &terms("claude"), binding_only()),
    )
    .await
    .expect("answered at the silence bound, not the answer's");
    assert_eq!(refused.err(), Some(LeaseRefusal::EnvironmentUnreachable));
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
    let lost = opened.hold.ends().unwrap();
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

#[test]
fn an_artifact_channel_rides_the_lease_connection_and_asks_for_no_forwarding() {
    let host = devbox();
    let arguments = super::connector::channel_arguments(&host, "/tmp/c0");
    for option in [
        "ControlMaster=no",
        "ProxyCommand=false",
        "BatchMode=yes",
        "ClearAllForwardings=yes",
        "ForwardAgent=no",
        "ForwardX11=no",
    ] {
        let at = arguments.iter().position(|word| *word == option);
        assert!(at.is_some_and(|at| arguments[at - 1] == "-o"), "{option}");
    }
    let separator = arguments.iter().position(|word| *word == "--").unwrap();
    assert_eq!(&arguments[separator + 1..], ["devbox", "sftp"]);
    assert_eq!(&arguments[..2], ["-S", "/tmp/c0"]);
}

// ---- command leases (issue #700) ----

/// Runs `echo` at once, and anything else until it is stopped.
struct Commands;
impl CommandRunner for Commands {
    fn run(
        &self,
        command: CommandWork,
        mut stop: tokio::sync::watch::Receiver<Option<CommandStop>>,
        _publish_point: Option<String>,
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
    *connector.runner.lock().unwrap() = Some(Arc::new(Commands));
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
        .expect("granted")
        .hold;
    assert_eq!(commands.reachable(), Some(true));
    let (_stop, stopped) = tokio::sync::watch::channel(None);
    let result = tokio::time::timeout(Duration::from_secs(5), hold.run(10_000, stopped))
        .await
        .expect("answered");
    assert_eq!(result.exit, CommandExit::Exited { code: 0 });
    assert_eq!(result.stdout, b"hi there");
    assert_eq!(result.stderr, b"warned");
    assert_eq!(result.dropped_bytes, 2);
    assert_eq!(
        result.cleanup,
        Some(LeaseCleanup::Confirmed { forced: false })
    );
    // The host ended the lease itself, and recorded it.
    let ledger = connector.ledger.0.lock().unwrap().clone();
    assert!(
        ledger
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::Ended { .. })),
        "{ledger:?}"
    );
}

/// Row C17: a command that has finished keeps its route until the host
/// answers End. A file already offered holds End back until it is
/// answered, and End is then asked, so the host closes the publish point.
#[tokio::test]
async fn a_finished_command_keeps_its_publish_route_until_the_host_ends_it() {
    let connector = Connector::new(Reach::PublishesOnRun);
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let commands = host.commands().expect("an SSH host runs commands");
    let mut hold = commands
        .grant(&lease(), &work(&["echo", "note"]))
        .await
        .expect("granted")
        .hold;
    let mut offers = hold.publications().expect("the command publishes");
    let (_stop, stopped) = tokio::sync::watch::channel(None);
    let running = tokio::spawn(async move { hold.run(10_000, stopped).await });
    let offer = tokio::time::timeout(Duration::from_secs(5), offers.recv())
        .await
        .expect("the file was offered")
        .expect("the file was offered");
    let result = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("answered")
        .expect("the command ran");
    assert_eq!(result.exit, CommandExit::Exited { code: 0 });
    // The publication stream ends with `Ran`, without waiting for `Ended`.
    assert!(tokio::time::timeout(Duration::from_secs(1), offers.recv())
        .await
        .expect("the stream ended")
        .is_none());
    // `Ran` is routed. End waits while the offered file is still unanswered.
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(
        !connector
            .frames
            .lock()
            .unwrap()
            .iter()
            .any(|frame| matches!(frame, ToEnvironment::End { .. })),
        "End was asked while a file was still unanswered"
    );
    offer.answer.answer(Collection::Held).await;
    let asked = tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            if connector
                .frames
                .lock()
                .unwrap()
                .iter()
                .any(|frame| matches!(frame, ToEnvironment::End { .. }))
            {
                break;
            }
            tokio::task::yield_now().await;
        }
    })
    .await;
    assert!(asked.is_ok(), "End was asked once the file was answered");
}

#[tokio::test]
async fn a_stopped_command_is_stopped_on_the_host_and_answers_the_stops_cause() {
    let connector = serving_commands();
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let commands = host.commands().unwrap();
    let hold = commands
        .grant(&lease(), &work(&["sleep", "60"]))
        .await
        .unwrap()
        .hold;
    let (stop, stopped) = tokio::sync::watch::channel(None);
    let running = tokio::spawn(hold.run(10_000, stopped));
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
    assert_eq!(
        result.cleanup,
        Some(LeaseCleanup::Confirmed { forced: true })
    );
}

/// Row K1: a command its host stopped as it drained, the gateway asking no
/// stop, is a system stop, never a lost one.
#[tokio::test]
async fn k1_a_command_its_host_drained_is_stopped_by_the_system_not_lost() {
    let connector = serving_commands();
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let commands = host.commands().unwrap();
    let hold = commands
        .grant(&lease(), &work(&["sleep", "60"]))
        .await
        .unwrap()
        .hold;
    let (_stop, stopped) = tokio::sync::watch::channel(None);
    let running = tokio::spawn(hold.run(10_000, stopped));
    // Drained once it runs, not before: one never run ends not started.
    tokio::time::timeout(Duration::from_secs(5), async {
        while !connector
            .ledger
            .0
            .lock()
            .unwrap()
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::CommandStarted { .. }))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the command starts on the host");
    connector.drain.send_replace(true);
    let result = tokio::time::timeout(Duration::from_secs(5), running)
        .await
        .expect("a drained command is answered")
        .unwrap();
    assert_eq!(
        result.exit,
        CommandExit::Stopped {
            cause: LeaseEndCause::Stopped
        }
    );
    assert_eq!(
        result.cleanup,
        Some(LeaseCleanup::Confirmed { forced: true })
    );
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
    let host = environment(
        Connector::new(Reach::Unreachable),
        Arc::new(Audit::default()),
    );
    let refused = host
        .commands()
        .unwrap()
        .grant(&lease(), &work(&["ls"]))
        .await
        .err();
    assert_eq!(
        refused,
        Some(CommandRefusal::Environment(
            LeaseRefusal::EnvironmentUnreachable
        ))
    );
}

/// Rows I4 and I5 over SSH: the host's owner narrows what SSH is granted.
/// An agent left out is refused not granted, a command's timeout comes back
/// narrowed as the terms granted, and with commands left out a command is
/// refused not granted; each refusal is the host's own, mapped as such.
#[tokio::test]
async fn what_the_hosts_owner_did_not_grant_is_refused_and_a_narrowed_timeout_is_answered() {
    let connector = serving_commands();
    *connector.grants.0.lock().unwrap() = Some(PeerGrant::new(
        Agents::Named(Default::default()),
        Some(CommandGrant::new(2_000).unwrap()),
    ));
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    assert_eq!(
        host.open(&lease(), &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentNotGranted)
    );
    let commands = host.commands().unwrap();
    let granted = commands
        .grant(&lease(), &work(&["echo", "narrowed"]))
        .await
        .expect("granted");
    assert_eq!(
        granted.granted,
        GrantedTerms {
            sandbox: SandboxProfile::None,
            timeout_ms: Some(2_000),
            lease_ms: None,
        }
    );
    drop(granted);
    *connector.grants.0.lock().unwrap() = Some(PeerGrant::nothing());
    assert_eq!(
        commands.grant(&lease(), &work(&["ls"])).await.err(),
        Some(CommandRefusal::Environment(
            LeaseRefusal::EnvironmentNotGranted
        ))
    );
}

/// Row I4: a sandbox the host cannot enforce is refused by the host, and
/// read here as that, not as the host being unreachable.
#[tokio::test]
async fn a_sandbox_the_host_cannot_enforce_is_refused_as_such() {
    let host = environment(Connector::new(Reach::Serving), Arc::new(Audit::default()));
    let unsandboxed = LeaseTerms {
        sandbox: SandboxProfile::None,
        ..terms("claude")
    };
    assert_eq!(
        host.open(&lease(), &unsandboxed, binding_only())
            .await
            .err(),
        Some(LeaseRefusal::SandboxUnavailable)
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
        .unwrap()
        .hold;
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
        .unwrap()
        .hold;
    let (_stop, stopped) = tokio::sync::watch::channel(None);
    let running = tokio::spawn(hold.run(10_000, stopped));
    tokio::time::sleep(Duration::from_millis(50)).await;
    connector.cut();
    let result = tokio::time::timeout(Duration::from_secs(10), running)
        .await
        .expect("a loss is answered")
        .unwrap();
    assert_eq!(result.exit, CommandExit::Unanswered);
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
            crate::env_serve::install::probe_command(LEASE_PROTOCOL, &[&this_digest()]),
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
    let serve = crate::env_serve::install::serve_command(&[&this_digest()]);
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

/// The executable is the one opened at start: an update that puts another
/// file at its path afterwards changes neither what is measured nor what is
/// sent, each send reading it from its start.
#[test]
fn an_executable_replaced_at_its_path_is_still_the_one_measured_and_sent() {
    use sha2::Digest;
    let directory = tempfile::tempdir().unwrap();
    let path = directory.path().join("nessa");
    std::fs::write(&path, THIS_BUILD).unwrap();
    let executable = OwnExecutable::opened_at(&path);
    let replacement = directory.path().join("nessa.new");
    std::fs::write(&replacement, b"a later build").unwrap();
    std::fs::rename(&replacement, &path).unwrap();
    for _ in 0..2 {
        let Build { mut file, digest } = executable.open().unwrap();
        assert_eq!(digest, format!("{:x}", sha2::Sha256::digest(THIS_BUILD)));
        let mut sent = Vec::new();
        std::io::Read::read_to_end(&mut file, &mut sent).unwrap();
        assert_eq!(sent, THIS_BUILD);
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

// ---- another platform's release build (#734) ----

/// What is sent as the release build for another platform.
const RELEASE_BUILD: &[u8] = b"the release's build for macOS on Apple silicon";

fn release_digest() -> String {
    use sha2::Digest;
    format!("{:x}", sha2::Sha256::digest(RELEASE_BUILD))
}

/// Opens `bytes`, measured as they are; counts the opens.
struct Bytes(Vec<u8>, AtomicUsize);
impl BuildSource for Bytes {
    fn open(&self) -> io::Result<Build> {
        use sha2::Digest;
        self.1.fetch_add(1, Ordering::SeqCst);
        let mut file = tempfile::tempfile()?;
        std::io::Write::write_all(&mut file, &self.0)?;
        std::io::Seek::seek(&mut file, std::io::SeekFrom::Start(0))?;
        Ok(Build {
            file,
            digest: format!("{:x}", sha2::Sha256::digest(&self.0)),
        })
    }
}

fn apple_silicon() -> Platform {
    Platform::of_target("aarch64-apple-darwin").unwrap()
}

/// The installer of a Linux gateway that is a release pinning `bytes`,
/// opened by `source`, as its macOS on Apple silicon build.
fn release_installer(shell: Arc<dyn RemoteShell>, source: Arc<dyn BuildSource>) -> HostInstaller {
    HostInstaller::new(
        shell,
        Arc::new(Built),
        Platform::of_target("x86_64-unknown-linux-gnu").unwrap(),
        LEASE_PROTOCOL,
        InstallTimings {
            probe: Duration::from_secs(5),
            upload: Duration::from_secs(5),
        },
    )
    .with_releases(vec![ReleaseCopy {
        platform: apple_silicon(),
        digest: release_digest(),
        build: source,
    }])
}

fn releasing(
    connector: Arc<Connector>,
    shell: Arc<Shell>,
    audit: Arc<Audit>,
    source: Arc<dyn BuildSource>,
) -> SshEnvironment {
    SshEnvironment::new(
        devbox(),
        connector,
        audit,
        Arc::new(release_installer(shell, source)),
        SshTimings {
            connect: Duration::from_secs(5),
            answer: Duration::from_secs(5),
            ..SshTimings::default()
        },
    )
}

/// A host this build does not run on gets the release build this one pins
/// for its platform: probed and served by each copy's digest, this build's
/// first, sent with the release build's digest, recorded, then served.
#[tokio::test]
async fn a_host_of_another_platform_gets_the_release_build_pinned_for_it() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new(
        "absent Darwin arm64 other\n",
        "installed\n",
        connector.clone(),
    );
    let audit = Arc::new(Audit::default());
    let source = Arc::new(Bytes(RELEASE_BUILD.to_vec(), AtomicUsize::new(0)));
    let environment = releasing(
        connector.clone(),
        shell.clone(),
        audit.clone(),
        source.clone(),
    );
    let (binding, _host) = binding(true);
    let lease = lease();
    environment
        .open(&lease, &terms("claude"), binding)
        .await
        .map(|_| ())
        .unwrap();
    let builds = [this_digest(), release_digest()];
    let builds: Vec<&str> = builds.iter().map(String::as_str).collect();
    assert_eq!(
        shell.runs(),
        [
            (
                crate::env_serve::install::probe_command(LEASE_PROTOCOL, &builds),
                None
            ),
            (
                crate::env_serve::install::upload_command(LEASE_PROTOCOL, &release_digest()),
                Some(RELEASE_BUILD.to_vec())
            ),
        ]
    );
    let serve = crate::env_serve::install::serve_command(&builds);
    assert_eq!(*connector.commands.lock().unwrap(), [serve.clone(), serve]);
    let (host, lease, protocol) = (
        "devbox".to_owned(),
        lease.as_str().to_owned(),
        LEASE_PROTOCOL.to_owned(),
    );
    assert_eq!(
        audit.events(),
        [
            EnvironmentEvent::InstallStarted {
                host: host.clone(),
                lease: lease.clone(),
                protocol: protocol.clone(),
                digest: release_digest(),
            },
            EnvironmentEvent::Installed {
                host,
                lease,
                protocol,
                digest: release_digest(),
            },
            EnvironmentEvent::Connected {
                host: "devbox".into(),
                workspace: "/srv/work".into(),
            },
        ]
    );
}

/// A release build that is not the bytes pinned (the download was
/// replaced, or damaged) is refused before a byte is sent, with the digest
/// it had; one that could not be downloaded is refused too.
#[tokio::test]
async fn a_release_build_that_is_not_the_pinned_one_is_refused_and_sent_nothing() {
    let cases: [(Arc<dyn BuildSource>, Option<String>); 2] = [
        (
            Arc::new(Bytes(b"something else".to_vec(), AtomicUsize::new(0))),
            Some({
                use sha2::Digest;
                format!("{:x}", sha2::Sha256::digest(b"something else"))
            }),
        ),
        (
            {
                struct Unreachable;
                impl BuildSource for Unreachable {
                    fn open(&self) -> io::Result<Build> {
                        Err(io::Error::other("the release answered 404"))
                    }
                }
                Arc::new(Unreachable)
            },
            None,
        ),
    ];
    for (source, seen) in cases {
        let connector = Connector::new(Reach::Silent);
        let shell = Shell::new("absent Darwin arm64 other", "installed", connector.clone());
        let audit = Arc::new(Audit::default());
        let environment = releasing(connector.clone(), shell.clone(), audit.clone(), source);
        let lease = lease();
        assert_eq!(
            environment
                .open(&lease, &terms("claude"), binding_only())
                .await
                .err(),
            Some(LeaseRefusal::EnvironmentInstallFailed),
            "{seen:?}"
        );
        assert_eq!(shell.runs().len(), 1, "probed, and nothing uploaded");
        assert_eq!(
            audit.events(),
            [refused(&lease, InstallRefusal::Release, seen.as_deref())]
        );
    }
}

/// A release build whose copy on the host speaks another protocol is
/// refused by the host's own check, as this build's would be.
#[tokio::test]
async fn a_release_build_speaking_another_protocol_is_refused_by_the_host() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new(
        "absent Darwin arm64 other",
        "refused version fedcba9876543210",
        connector.clone(),
    );
    let audit = Arc::new(Audit::default());
    let source = Arc::new(Bytes(RELEASE_BUILD.to_vec(), AtomicUsize::new(0)));
    let environment = releasing(connector.clone(), shell.clone(), audit.clone(), source);
    let lease = lease();
    assert_eq!(
        environment
            .open(&lease, &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentVersionMismatch)
    );
    assert_eq!(
        audit.events().last(),
        Some(&refused(
            &lease,
            InstallRefusal::Version,
            Some("fedcba9876543210")
        ))
    );
}

/// A host whose platform neither this build nor any it pins runs on is
/// refused as before; so is every host of another platform for a build
/// that pins none (a development build), and one whose pins do not read.
#[tokio::test]
async fn a_platform_nothing_is_pinned_for_is_still_refused() {
    let connector = Connector::new(Reach::Silent);
    let shell = Shell::new("absent Linux aarch64 gnu", "installed", connector.clone());
    let audit = Arc::new(Audit::default());
    let source = Arc::new(Bytes(RELEASE_BUILD.to_vec(), AtomicUsize::new(0)));
    let environment = releasing(
        connector.clone(),
        shell.clone(),
        audit.clone(),
        source.clone(),
    );
    let lease = lease();
    assert_eq!(
        environment
            .open(&lease, &terms("claude"), binding_only())
            .await
            .err(),
        Some(LeaseRefusal::EnvironmentPlatformUnsupported)
    );
    assert_eq!(source.1.load(Ordering::SeqCst), 0, "nothing downloaded");
    assert_eq!(
        audit.events(),
        [refused(
            &lease,
            InstallRefusal::Platform,
            Some("linux aarch64 gnu")
        )]
    );
    struct NeverFetched;
    impl super::release::ReleaseDownload for NeverFetched {
        fn fetch(&self, _: &str, _: u64, _: &mut std::fs::File) -> io::Result<()> {
            unreachable!("nothing is pinned")
        }
    }
    let cache = tempfile::tempdir().unwrap();
    let pin = format!("aarch64-apple-darwin={}:46", release_digest());
    for (spec, from) in [
        (None, None),
        (Some(pin.as_str()), None),
        (None, Some("https://example.test")),
        (
            Some("aarch64-apple-darwin=nope"),
            Some("https://example.test"),
        ),
    ] {
        assert!(
            ReleaseCopy::pinned(spec, from, Arc::new(NeverFetched), cache.path()).is_empty(),
            "{spec:?} {from:?}"
        );
    }
    let pinned = ReleaseCopy::pinned(
        Some(&pin),
        Some("https://example.test"),
        Arc::new(NeverFetched),
        cache.path(),
    );
    assert_eq!(pinned.len(), 1);
    assert_eq!(pinned[0].platform, apple_silicon());
    assert_eq!(pinned[0].digest, release_digest());
}

/// Runs each command with this machine's own `sh` in a home of its own, as
/// `sshd` hands it to a login shell.
struct LocalShell(std::path::PathBuf);
impl RemoteShell for LocalShell {
    fn run<'a>(
        &'a self,
        _host: &'a SshDestination,
        command: String,
        input: Option<std::fs::File>,
    ) -> ShellFuture<'a> {
        let home = self.0.clone();
        Box::pin(async move {
            tokio::task::spawn_blocking(move || {
                let stdin = match input {
                    Some(file) => std::process::Stdio::from(file),
                    None => std::process::Stdio::null(),
                };
                let output = std::process::Command::new("sh")
                    .arg("-c")
                    .arg(&command)
                    .env_clear()
                    .env("HOME", &home)
                    .env("PATH", std::env::var_os("PATH").unwrap_or_default())
                    .stdin(stdin)
                    .output()?;
                Ok(String::from_utf8_lossy(&output.stdout).into_owned())
            })
            .await
            .map_err(io::Error::other)?
        })
    }
}

/// A stand-in `nessa` that answers `env protocol` with `protocol`.
fn stand_in(protocol: &str) -> Vec<u8> {
    format!("#!/bin/sh\n[ \"$1 $2\" = \"env protocol\" ] && echo {protocol}\n").into_bytes()
}

/// The gate of #734 against this machine's own `sh` and tools, which the
/// gateway treats as another platform's host: a gateway on another platform
/// installs the release build it pins for this one, verified by the host's
/// own scripts, and its serve command then finds that copy; a download
/// that is not the pinned build is refused before anything is sent; a copy
/// that speaks another protocol is refused by the host.
#[tokio::test]
async fn a_gateway_of_another_platform_installs_its_release_build_with_the_hosts_own_scripts() {
    let here = Platform::this_build();
    let elsewhere = if here.os == "macos" {
        Platform::of_target("x86_64-unknown-linux-gnu").unwrap()
    } else {
        apple_silicon()
    };
    let installer = |home: &std::path::Path, pinned: &[u8], sent: Vec<u8>| {
        use sha2::Digest;
        HostInstaller::new(
            Arc::new(LocalShell(home.into())),
            Arc::new(Built),
            elsewhere.clone(),
            LEASE_PROTOCOL,
            InstallTimings {
                probe: Duration::from_secs(30),
                upload: Duration::from_secs(30),
            },
        )
        .with_releases(vec![ReleaseCopy {
            platform: here.clone(),
            digest: format!("{:x}", sha2::Sha256::digest(pinned)),
            build: Arc::new(Bytes(sent, AtomicUsize::new(0))),
        }])
    };
    let release = stand_in(LEASE_PROTOCOL);
    let home = tempfile::tempdir().unwrap();
    let gateway = installer(home.path(), &release, release.clone());
    let audit = Audit::default();
    assert_eq!(
        gateway.ensure(&devbox(), "lease-1", &audit).await,
        Ok(super::install::Installation::Installed)
    );
    assert_eq!(
        gateway.ensure(&devbox(), "lease-2", &audit).await,
        Ok(super::install::Installation::Present),
        "found there by the probe, and not sent again"
    );
    let serve = gateway.serve_command().await.unwrap();
    let said = LocalShell(home.path().into())
        .run(&devbox(), serve.replace("env serve", "env protocol"), None)
        .await
        .unwrap();
    assert_eq!(
        said.trim(),
        LEASE_PROTOCOL,
        "the serve command runs that copy"
    );

    let home = tempfile::tempdir().unwrap();
    let replaced = installer(home.path(), &release, stand_in("0123456789abcdef"));
    assert_eq!(
        replaced.ensure(&devbox(), "lease-3", &audit).await,
        Err(LeaseRefusal::EnvironmentInstallFailed)
    );
    assert!(!home.path().join(".nessa").exists(), "nothing was sent");

    let other = stand_in("0123456789abcdef");
    let home = tempfile::tempdir().unwrap();
    let speaking = installer(home.path(), &other, other.clone());
    assert_eq!(
        speaking.ensure(&devbox(), "lease-4", &audit).await,
        Err(LeaseRefusal::EnvironmentVersionMismatch)
    );
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
    async fn open(host: &SshDestination, installer: Arc<HostInstaller>, audit: Arc<Audit>) {
        let environment = SshEnvironment::new(
            host.clone(),
            Arc::new(OpenSshConnector::new()),
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
            Arc::new(OpenSshConnector::new()),
            Arc::new(OwnExecutable::opened_at(&build)),
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

// ---- revocation of a paired host (row I8) ----

/// Ends the connection it keeps, as a paired connection's TLS stream ends
/// once let go.
struct Cut(Arc<Connector>);
impl Drop for Cut {
    fn drop(&mut self) {
        self.0.cut();
    }
}

/// A link over a connection `connector` makes, as a paired host's: closing
/// it ends the stream.
async fn dialed_link(connector: &Arc<Connector>, audit: Arc<Audit>) -> Arc<HostLink> {
    let mut connection = connector.connect(&devbox(), String::new()).unwrap();
    connection.keep = Box::new(Cut(connector.clone()));
    HostLink::over(
        "peer:test",
        connection,
        audit,
        Duration::from_secs(5),
        Liveness {
            keepalive: KEEPALIVE_INTERVAL,
            silence: SILENCE_LIMIT,
        },
    )
    .await
    .unwrap_or_else(|_| panic!("the host answers"))
}

async fn let_go(gone: &mut tokio::sync::watch::Receiver<()>) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while gone.changed().await.is_ok() {}
    })
    .await
    .expect("the lease is let go");
}

/// Row I8 on the gateway's link: once revoked, no lease is admitted, the
/// lease running on it ends as revoked, the host is told and ends it with
/// that cause, its End is answered with the host's cleanup, the revocation
/// is recorded with the lease, and the connection closes once the lease is
/// let go.
#[tokio::test]
async fn a_revoked_link_ends_each_lease_as_revoked_and_closes_once_they_are_ended() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let link = dialed_link(&connector, audit.clone()).await;
    let id = lease();
    let mut granted = link
        .grant(
            id.as_str(),
            "claude",
            Sandbox::HarnessDefault,
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    let mut harness = link.start(id.as_str(), BTreeMap::new()).unwrap();
    harness.input.write_all(b"ping").await.unwrap();
    assert_eq!(read_some(&mut harness.output).await, b"ping");
    assert!(link.revoke(Duration::from_secs(30)), "this call revoked it");
    assert!(
        !link.revoke(Duration::from_secs(30)),
        "asked again, nothing more"
    );
    // From the moment it is revoked, nothing more is started or published
    // on it: the host is ending every lease.
    assert_eq!(
        link.start(id.as_str(), BTreeMap::new()).err(),
        Some(super::link::StartError::Closed)
    );
    assert_eq!(
        link.publish(id.as_str(), "call".into(), "out.txt".into(), None)
            .await
            .err(),
        Some(
            crate::conversation::application::PublishFailure::NotPublished(
                nessa_protocol::lease::PublishRefusal::LeaseEnded
            )
        )
    );
    assert_eq!(
        *granted.ending.wait_for(Option::is_some).await.unwrap(),
        Some(LeaseEndCause::Revoked)
    );
    assert!(link.is_open() && !link.is_serving() && link.revoked());
    // Nothing more is admitted on it.
    assert_eq!(
        link.grant(
            lease().as_str(),
            "claude",
            Sandbox::HarnessDefault,
            None,
            Duration::from_secs(5)
        )
        .await
        .err(),
        Some(LeaseRefusal::EnvironmentUnreachable)
    );
    // Its End is answered with what the host's own end took.
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(5), link.end(id.as_str()))
            .await
            .unwrap(),
        Some(Cleanup::Confirmed { forced: false })
    );
    let_go(&mut granted.gone).await;
    tokio::time::timeout(Duration::from_secs(5), link.ended())
        .await
        .expect("the revoked connection closes once its leases are let go");
    assert!(!link.unaudited(id.as_str()));
    assert_eq!(connector.stopped.load(Ordering::SeqCst), 1);
    let events = audit.events();
    let revoked = EnvironmentEvent::Revoked {
        host: "peer:test".into(),
        leases: vec![id.as_str().into()],
    };
    assert_eq!(
        events.iter().filter(|event| **event == revoked).count(),
        1,
        "{events:?}"
    );
    assert!(events.contains(&EnvironmentEvent::ConnectionLost {
        host: "peer:test".into(),
        leases: Vec::new(),
    }));
    let entries = connector.ledger.0.lock().unwrap().clone();
    let revoking = entries
        .iter()
        .position(|entry| matches!(entry, LedgerEntry::Revoking { lease, peer: Peer::Ssh } if lease == id.as_str()))
        .expect("the host records why it ends the lease");
    let ended = entries
        .iter()
        .position(|entry| {
            *entry
                == LedgerEntry::Ended {
                    lease: id.as_str().into(),
                    cleanup: Cleanup::Confirmed { forced: false },
                    cause: EndCause::Revoked,
                }
        })
        .expect("and that it ended as revoked");
    assert!(revoking < ended, "{entries:?}");
}

/// Row C22: a command whose host says `Ran` and never `Ended`, while the
/// connection stays up, gives its organization place back after the stop
/// wait. A later command can take that place.
#[tokio::test]
async fn a_finished_command_releases_its_budget_place_when_ended_never_arrives() {
    let connector = Connector::new(Reach::AnswersRan);
    let link = dialed_link(&connector, Arc::new(Audit::default())).await;
    let budgets = crate::conversation::application::OrganizationBudgets::new(None, Some(1));
    let id = lease();
    link.grant_command(
        id.as_str(),
        vec!["echo".into(), "hi".into()],
        None,
        5_000,
        budgets.command(),
        Duration::from_secs(5),
    )
    .await
    .expect("the host grants the command");
    assert_eq!(budgets.running(), (0, 1));
    let answer = tokio::time::timeout(Duration::from_secs(5), link.run(id.as_str()))
        .await
        .expect("the host says Ran")
        .expect("the command is still routed");
    assert!(matches!(answer.end, CommandEnd::Exited { code: 0 }));
    assert_eq!(budgets.running(), (0, 1));
    tokio::time::timeout(COMMAND_STOP_WAIT + Duration::from_secs(2), async {
        while budgets.running() != (0, 0) {
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("the place is given back once Ended does not arrive");
    assert!(
        tokio::time::timeout(Duration::from_millis(50), link.ended())
            .await
            .is_err(),
        "the connection stayed up"
    );
    assert!(budgets.command().is_some());
}

/// A grant the host does not answer in time keeps its organization budget
/// place: the host may still admit it, so the place is given back only once
/// the host answers the End it is sent.
#[tokio::test]
async fn a_grant_not_answered_in_time_keeps_its_budget_place_until_its_end() {
    let (open, gate) = tokio::sync::watch::channel(false);
    let connector = Connector::new(Reach::GrantingLate(gate));
    let link = dialed_link(&connector, Arc::new(Audit::default())).await;
    let budgets = crate::conversation::application::OrganizationBudgets::new(Some(1), None);
    assert_eq!(
        link.grant(
            lease().as_str(),
            "claude",
            Sandbox::HarnessDefault,
            budgets.lease(),
            Duration::from_millis(50),
        )
        .await
        .err(),
        Some(LeaseRefusal::EnvironmentUnreachable)
    );
    assert_eq!(budgets.running(), (1, 0));
    assert!(budgets.lease().is_none());
    // The host grants it late, then reads its End and answers it.
    open.send_replace(true);
    tokio::time::timeout(Duration::from_secs(5), async {
        while budgets.running() != (0, 0) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("its place is given back once the host ends it");
}

/// A lease still waiting for its grant is not yet a lease the connection
/// carries: the pool counts it as reserved until then, and it is counted
/// here once granted.
#[tokio::test]
async fn a_lease_counts_on_its_connection_once_granted() {
    let (open, gate) = tokio::sync::watch::channel(false);
    let connector = Connector::new(Reach::GrantingLate(gate));
    let link = dialed_link(&connector, Arc::new(Audit::default())).await;
    let asking = {
        let link = link.clone();
        tokio::spawn(async move {
            link.grant(
                lease().as_str(),
                "claude",
                Sandbox::HarnessDefault,
                None,
                Duration::from_secs(5),
            )
            .await
        })
    };
    tokio::time::timeout(Duration::from_secs(5), async {
        while !connector
            .frames
            .lock()
            .unwrap()
            .iter()
            .any(|frame| matches!(frame, ToEnvironment::Grant { .. }))
        {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host reads the Grant");
    assert_eq!(link.agent_leases(), 0);
    open.send_replace(true);
    let _granted = tokio::time::timeout(Duration::from_secs(5), asking)
        .await
        .expect("answered")
        .unwrap()
        .expect("granted");
    assert_eq!(link.agent_leases(), 1);
}

/// A lease the host grants after its connection was revoked, its Grant
/// already on its way ahead of the `Revoked`, is named in a revocation
/// record of its own, as every lease the host granted is, and ends as
/// revoked. That record unwritten, the lease settles nothing as confirmed.
#[tokio::test]
async fn a_lease_granted_after_its_revocation_is_recorded_revoked_on_its_own() {
    for failing in [false, true] {
        let (open, gate) = tokio::sync::watch::channel(false);
        let connector = Connector::new(Reach::GrantingLate(gate));
        let audit = Arc::new(Audit::default());
        let link = dialed_link(&connector, audit.clone()).await;
        let id = lease();
        let asking = {
            let link = link.clone();
            let id = id.clone();
            tokio::spawn(async move {
                link.grant(
                    id.as_str(),
                    "claude",
                    Sandbox::HarnessDefault,
                    None,
                    Duration::from_secs(5),
                )
                .await
            })
        };
        // The host has read its Grant, and holds its answer.
        tokio::time::timeout(Duration::from_secs(5), async {
            while !connector
                .frames
                .lock()
                .unwrap()
                .iter()
                .any(|frame| matches!(frame, ToEnvironment::Grant { .. }))
            {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("the host reads the Grant");
        if failing {
            // Only the revocation's own record is written.
            audit.2.store(audit.events().len() + 1, Ordering::SeqCst);
        }
        link.revoke(Duration::from_secs(30));
        open.send_replace(true);
        let mut granted = tokio::time::timeout(Duration::from_secs(5), asking)
            .await
            .expect("answered")
            .unwrap()
            .expect("the host granted it, its Grant ahead of the revocation");
        assert_eq!(
            *granted.ending.wait_for(Option::is_some).await.unwrap(),
            Some(LeaseEndCause::Revoked)
        );
        // The host ends it as told; once it is let go, the connection closes.
        let_go(&mut granted.gone).await;
        // Its Ended is read after its Granted was handled.
        let revocations: Vec<_> = audit
            .events()
            .into_iter()
            .filter(|event| matches!(event, EnvironmentEvent::Revoked { .. }))
            .collect();
        let alone = EnvironmentEvent::Revoked {
            host: "peer:test".into(),
            leases: vec![id.as_str().into()],
        };
        let first = EnvironmentEvent::Revoked {
            host: "peer:test".into(),
            leases: Vec::new(),
        };
        if failing {
            assert_eq!(revocations, vec![first], "failing");
            assert!(link.unaudited(id.as_str()));
        } else {
            assert_eq!(revocations, vec![first, alone]);
            assert!(!link.unaudited(id.as_str()));
        }
        drop(granted);
        tokio::time::timeout(Duration::from_secs(5), link.ended())
            .await
            .expect("closed once it is let go");
    }
}

/// A revoked connection whose lease nobody ends is closed at its drain
/// bound all the same: its permit is not held for good.
#[tokio::test]
async fn a_revoked_link_closes_at_its_bound_when_a_lease_is_never_ended() {
    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let link = dialed_link(&connector, audit.clone()).await;
    let id = lease();
    let _granted = link
        .grant(
            id.as_str(),
            "claude",
            Sandbox::HarnessDefault,
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    link.revoke(Duration::from_millis(100));
    tokio::time::timeout(Duration::from_secs(5), link.ended())
        .await
        .expect("closed at the bound");
    assert!(audit.events().contains(&EnvironmentEvent::ConnectionLost {
        host: "peer:test".into(),
        leases: vec![id.as_str().into()],
    }));
}

/// A revocation that cannot be recorded still ends and closes everything,
/// but settles none of its leases as confirmed: a command the host stops
/// for it answers no cleanup, where a recorded revocation answers the
/// host's.
#[tokio::test]
async fn a_revocation_whose_audit_fails_leaves_its_leases_unconfirmed() {
    for failing in [false, true] {
        let audit = Arc::new(Audit::default());
        let connector = serving_commands();
        let host = environment(connector.clone(), audit.clone());
        let commands = host.commands().unwrap();
        let hold = commands
            .grant(&lease(), &work(&["sleep", "60"]))
            .await
            .unwrap()
            .hold;
        let (_stop, stopped) = tokio::sync::watch::channel(None);
        let running = tokio::spawn(hold.run(10_000, stopped));
        // Revoked once the host is running it, so its stop is the revocation's.
        until_recorded(&connector, |entry| {
            matches!(entry, LedgerEntry::CommandStarted { .. })
        })
        .await;
        audit.1.store(failing, Ordering::SeqCst);
        host.revoke_connection().await;
        let result = tokio::time::timeout(Duration::from_secs(5), running)
            .await
            .expect("the host stops it and answers")
            .unwrap();
        assert_eq!(
            result.exit,
            CommandExit::Stopped {
                cause: LeaseEndCause::Revoked
            }
        );
        assert_eq!(
            result.cleanup,
            (!failing).then_some(LeaseCleanup::Confirmed { forced: true }),
            "audit failing: {failing}"
        );
    }

    let connector = Connector::new(Reach::Serving);
    let audit = Arc::new(Audit::default());
    let link = dialed_link(&connector, audit.clone()).await;
    let id = lease();
    let mut granted = link
        .grant(
            id.as_str(),
            "claude",
            Sandbox::HarnessDefault,
            None,
            Duration::from_secs(5),
        )
        .await
        .unwrap();
    audit.1.store(true, Ordering::SeqCst);
    link.revoke(Duration::from_secs(30));
    assert_eq!(
        *granted.ending.wait_for(Option::is_some).await.unwrap(),
        Some(LeaseEndCause::Revoked)
    );
    assert!(link.unaudited(id.as_str()));
    let _ = tokio::time::timeout(Duration::from_secs(5), link.end(id.as_str())).await;
    tokio::time::timeout(Duration::from_secs(5), link.ended())
        .await
        .expect("closed all the same");
}

/// Waits until the host's ledger holds an entry `recorded` matches.
async fn until_recorded(connector: &Connector, recorded: impl Fn(&LedgerEntry) -> bool) {
    tokio::time::timeout(Duration::from_secs(5), async {
        while !connector.ledger.0.lock().unwrap().iter().any(&recorded) {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("the host records it");
}

/// A command granted before its connection was revoked, and run after, is
/// still sent to the host, which answers it never started and holds
/// nothing: its lease ended revoked, on the host's own record.
#[tokio::test]
async fn a_command_run_after_its_revocation_is_answered_never_started() {
    let connector = serving_commands();
    let host = environment(connector.clone(), Arc::new(Audit::default()));
    let commands = host.commands().unwrap();
    let id = lease();
    let hold = commands
        .grant(&id, &work(&["sleep", "60"]))
        .await
        .unwrap()
        .hold;
    host.revoke_connection().await;
    until_recorded(
        &connector,
        |entry| matches!(entry, LedgerEntry::Ended { lease, .. } if lease == id.as_str()),
    )
    .await;
    let (_stop, stopped) = tokio::sync::watch::channel(None);
    let result = tokio::time::timeout(Duration::from_secs(5), hold.run(10_000, stopped))
        .await
        .expect("the host answers it");
    assert_eq!(result.exit, CommandExit::NotStarted);
    assert_eq!(result.cleanup, Some(LeaseCleanup::NotHeld));
    let entries = connector.ledger.0.lock().unwrap().clone();
    assert!(
        entries.contains(&LedgerEntry::Revoking {
            lease: id.as_str().into(),
            peer: Peer::Ssh,
        }),
        "{entries:?}"
    );
    assert!(
        entries.contains(&LedgerEntry::Ended {
            lease: id.as_str().into(),
            cleanup: Cleanup::NotHeld,
            cause: EndCause::Revoked,
        }),
        "{entries:?}"
    );
    assert!(
        !entries
            .iter()
            .any(|entry| matches!(entry, LedgerEntry::CommandStarted { .. })),
        "{entries:?}"
    );
}

/// The first cause known is the one kept: an expiry the host said before
/// the revocation stays the lease's cause, and an expiry said after it does
/// not replace the revocation. The host is told once however often asked.
#[tokio::test]
async fn a_lease_ending_for_one_cause_keeps_it_when_the_other_comes() {
    for (early, kept) in [
        (true, LeaseEndCause::Expired),
        (false, LeaseEndCause::Revoked),
    ] {
        let connector = Connector::new(Reach::Expiring { early });
        let link = dialed_link(&connector, Arc::new(Audit::default())).await;
        let id = lease();
        let mut granted = link
            .grant(
                id.as_str(),
                "claude",
                Sandbox::HarnessDefault,
                None,
                Duration::from_secs(5),
            )
            .await
            .unwrap();
        if early {
            assert_eq!(
                *granted.ending.wait_for(Option::is_some).await.unwrap(),
                Some(LeaseEndCause::Expired)
            );
        }
        link.revoke(Duration::from_secs(30));
        link.revoke(Duration::from_secs(30));
        // The host's Ending, then its Ended, follow the revocation.
        let_go(&mut granted.gone).await;
        assert_eq!(*granted.ending.borrow(), Some(kept));
        tokio::time::timeout(Duration::from_secs(5), link.ended())
            .await
            .expect("closed");
        let revoked = connector
            .frames
            .lock()
            .unwrap()
            .iter()
            .filter(|frame| **frame == ToEnvironment::Revoked)
            .count();
        assert_eq!(revoked, 1);
    }
}

// #922: the lease's host is authority, even when target adaptation happens later.
#[tokio::test]
async fn model_targets_and_normalization_retain_the_granted_host() {
    let connector = Connector::new(Reach::Serving);
    let environment = environment(connector.clone(), Arc::new(Audit::default()));
    let (initial, given) = binding(true);
    let id = lease();
    let opened = environment
        .open(&id, &terms("claude"), initial)
        .await
        .unwrap();
    let original_host = given.lock().unwrap().clone().unwrap();
    assert!(opened.provider.harness_host_identity() == HarnessHostIdentity::hosted(&original_host));
    let (mut target, target_host) = binding(true);
    Arc::get_mut(&mut target).unwrap().capabilities =
        crate::conversation_test_support::Provider::new(Arc::new(
            crate::conversation_test_support::ProviderFactory::default(),
        ))
        .with_model("second-model")
        .capabilities()
        .clone();
    let wrapped = opened.hold.model_provider(target.clone()).unwrap();
    assert!(ModelChange::between(opened.provider.as_ref(), wrapped.as_ref()).is_ok());
    let adapted_host = target_host.lock().unwrap().clone().unwrap();
    assert!(Arc::ptr_eq(&original_host, &adapted_host));
    assert!(wrapped.harness_host_identity() == HarnessHostIdentity::hosted(&original_host));
    assert!(ModelChange::between(opened.provider.as_ref(), target.as_ref()).is_err());
    let normalized = wrapped.with_model_configuration(None, None, None).unwrap();
    assert!(normalized.harness_host_identity() == HarnessHostIdentity::hosted(&original_host));
    // Cleanup and a later launch use the same captured host, without another grant.
    for host in [&original_host, &adapted_host] {
        let mut process = host.start(HarnessLaunch::default()).unwrap();
        process.input.write_all(b"same lease").await.unwrap();
        assert_eq!(read_some(process.output.as_mut()).await, b"same lease");
        process.input.shutdown().await.unwrap();
        assert_eq!(
            process
                .control
                .cleanup(Duration::from_millis(10), Duration::from_millis(100))
                .await
                .unwrap(),
            CloseOutcome { forced: false }
        );
    }
    assert_eq!(
        opened.hold.end(LeaseEndCause::Closed).await,
        LeaseRelease::Released(LeaseCleanup::Confirmed { forced: false })
    );
    let entries = connector.ledger.0.lock().unwrap().clone();
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, LedgerEntry::Granted { .. }))
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, LedgerEntry::Ended { .. }))
            .count(),
        1
    );
    assert_eq!(
        entries
            .iter()
            .filter(|e| matches!(e, LedgerEntry::Started { .. }))
            .count(),
        2
    );
    assert_eq!(connector.stopped.load(Ordering::SeqCst), 2);
    connector.cut();
}

/// Control frames still reach the actual environment-role frame loop. Only End
/// delivery is held; the host's Granted/Ended ledger remains the evidence.
pub(crate) struct PreparationEndPause {
    entered: tokio::sync::Notify,
    requests: AtomicUsize,
    release: tokio::sync::watch::Sender<bool>,
    connector: std::sync::Weak<Connector>,
}
impl PreparationEndPause {
    pub(crate) async fn wait_end(&self) {
        tokio::time::timeout(Duration::from_secs(60), self.entered.notified())
            .await
            .unwrap();
    }
    pub(crate) fn end_requests(&self) -> usize {
        self.requests.load(Ordering::SeqCst)
    }
    pub(crate) fn release(&self) {
        self.release.send_replace(true);
    }
    pub(crate) fn held(&self) -> usize {
        let connector = self
            .connector
            .upgrade()
            .expect("environment still owns connector");
        let rows = connector.ledger.0.lock().unwrap();
        rows.iter()
            .filter(|row| match row {
                LedgerEntry::Granted { lease, .. } => !rows.iter().any(
                    |row| matches!(row, LedgerEntry::Ended { lease: ended, .. } if ended == lease),
                ),
                _ => false,
            })
            .count()
    }
    pub(crate) fn close(&self) {
        self.release();
        if let Some(connector) = self.connector.upgrade() {
            connector.cut();
        }
    }
}
fn relay_preparation_end(
    from: tokio::io::ReadHalf<DuplexStream>,
    mut to: tokio::io::WriteHalf<DuplexStream>,
    pause: Arc<PreparationEndPause>,
) -> JoinHandle<()> {
    tokio::spawn(async move {
        let mut stream = FrameStream::new(from);
        while let Ok(Some(body)) = stream.next().await {
            let frame: ToEnvironment = decode(&body).unwrap();
            if matches!(frame, ToEnvironment::End { .. }) {
                pause.requests.fetch_add(1, Ordering::SeqCst);
                pause.entered.notify_one();
                let mut release = pause.release.subscribe();
                let _ = release.wait_for(|open| *open).await;
            }
            if to.write_all(&encode(&frame).unwrap()).await.is_err() {
                break;
            }
        }
        let _ = to.shutdown().await;
    })
}
pub(crate) fn preparation_environment() -> (Arc<dyn Environment>, Arc<PreparationEndPause>) {
    let connector = Connector::new(Reach::Serving);
    let pause = Arc::new(PreparationEndPause {
        entered: tokio::sync::Notify::new(),
        requests: AtomicUsize::new(0),
        release: tokio::sync::watch::channel(false).0,
        connector: Arc::downgrade(&connector),
    });
    *connector.end_pause.lock().unwrap() = Some(pause.clone());
    (
        Arc::new(environment(connector, Arc::new(Audit::default()))),
        pause,
    )
}
