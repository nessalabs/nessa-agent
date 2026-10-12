//! A paired environment, end to end in one process (issue #706): gateway A
//! is composed as `local_auth` composes it, with its paired environments
//! behind the native listener; machine E pairs as an environment, dials A,
//! and serves A's leases with `nessa env serve`'s own frame loop over that
//! connection. A runs a command there, refuses a key paired only to read,
//! takes a second connection in place of the first, and stops at the
//! credential's revocation. Row I2 in `docs/design/auth/peer-gateways.md`
//! ("Environment peers").
use super::local_auth::SystemClock;
use super::native_pairing::{bind, prepare, start, RunningNative};
use super::paired_environments::{ListenerEnvironments, RegistryServing};
use super::peer_reading_tests::{inputs, owned, uuid, NoAgents, WAIT};
use super::runtime_config::NativeConfig;
use crate::app::dependencies::RuntimeDependencies;
use crate::conversation::application::LeaseRelease;
use crate::conversation::application::{CommandResult, EnvironmentFuture, PairedEnvironments};
use crate::conversation::infrastructure::ssh_environment::{
    DurableEnvironmentAudit, PeerEnvironments, ServingCredentials, SshTimings,
};
use crate::conversation::infrastructure::LocalReceiverAuthority;
use crate::device_pairing::infrastructure::PairingOwnerCommands;
use crate::env::{LEASE_PROTOCOL, VERSION};
use crate::env_serve::application::{
    serve, CommandRan, CommandRunner, CommandStop, Runners, ServeTimings, Serving,
};
use crate::env_serve::domain::{Agents, CommandGrant, MaxLease, Peer, PeerGrant};
use crate::env_serve::infrastructure::{
    dial, ConfiguredLauncher, FileGrants, FileLedger, FileOutbox, LaunchSpec, TokioTimer,
};
use crate::product::{ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::adapters::local::LocalCredentialStore;
use nessa_auth::adapters::pairing::{FilePairingState, ManualCode, NativeIdentity, OsEntropy};
use nessa_auth::application::credential_admin::RevokeCredentialRequest;
use nessa_auth::application::pairing::{ClientPendingStore, DeviceConnectionProof};
use nessa_auth::application::ports::AccessError;
use nessa_auth::application::ports::Clock;
use nessa_auth::application::session::AuthenticatedSession;
use nessa_auth::domain::pairing::ConsentClass;
use nessa_auth::domain::{AudienceId, OrganizationId, Resource, ResourceId};
use nessa_client_core::pairing::NativeEnrollmentClient;
use nessa_protocol::lease::{Cleanup, CommandEnd};
use nessa_protocol::pairing::wire::NativePairingStatus;
use nessa_sdk::application::agent_execution::{
    agents::AgentError,
    providers::{
        AgentProvider, HarnessHost, HarnessLaunch, ProviderIdentity, ProviderOpenError,
        ProviderOpenFuture, ProviderOpenRequest,
    },
};
use nessa_sdk::domain::agent_execution::leases::{
    AgentWork, LeaseDeadline, LeaseEndCause, LeaseGrants, LeaseTerms, LeaseWork, SandboxProfile,
};
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandRefusal, CommandWork, LeaseCleanup, LeaseId, LeaseRefusal, PeerEnvironment,
};
use std::collections::HashMap;
use std::future::Future;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;
use tokio::io::AsyncReadExt;
use tokio::sync::{watch, Notify};
use tokio::task::JoinHandle;

/// Gateway A: native pairing with its paired environments composed.
struct Gateway {
    auth: Arc<LocalCredentialStore>,
    session: AuthenticatedSession,
    owner: String,
    commands: PairingOwnerCommands,
    peers: Arc<PeerEnvironments>,
    /// Holds the next ask of whether a key serves, once armed.
    gate: Arc<Gate>,
    native: SocketAddr,
    running: Option<RunningNative>,
}

/// The registry's answer, held once when armed: the next ask waits until
/// released, so a test can act while a lease's ask is pending.
struct Gate {
    inner: RegistryServing,
    armed: AtomicBool,
    entered: Notify,
    released: Notify,
}

impl ServingCredentials for Gate {
    fn serves<'a>(
        &'a self,
        proof: &'a DeviceConnectionProof,
    ) -> EnvironmentFuture<'a, Result<bool, AccessError>> {
        Box::pin(async move {
            if self.armed.swap(false, Ordering::SeqCst) {
                self.entered.notify_one();
                self.released.notified().await;
            }
            self.inner.serves(proof).await
        })
    }
}

impl Gateway {
    async fn start(root: &Path) -> Self {
        let (gateway, organization) = (uuid(), uuid());
        let (auth, session, owner) = owned(root, &gateway, &organization).await;
        let receivers = Arc::new(
            LocalReceiverAuthority::open(
                &root.join("receivers.sqlite3"),
                &CedarPolicyEvaluator::profile_digest(),
                Arc::new(SystemClock),
            )
            .unwrap(),
        );
        let (prepared, commands, _) = prepare(
            &NativeConfig {
                listen_address: "127.0.0.1:0".parse().unwrap(),
            },
            inputs(root, &auth, receivers, &gateway, &organization),
        )
        .await
        .unwrap();
        let resource = Resource::new(
            OrganizationId::new(organization.clone()).unwrap(),
            ResourceId::new(gateway.clone()).unwrap(),
        );
        let gate = Arc::new(Gate {
            inner: RegistryServing {
                store: auth.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                clock: Arc::new(SystemClock),
                gateway: resource,
                audience: AudienceId::new(gateway.clone()).unwrap(),
            },
            armed: AtomicBool::new(false),
            entered: Notify::new(),
            released: Notify::new(),
        });
        let peers = Arc::new(PeerEnvironments::new(
            gate.clone(),
            DurableEnvironmentAudit::new(root.join("environments"), Arc::new(SystemClock)).unwrap(),
            SshTimings::default(),
        ));
        let prepared = prepared.with_environments(Arc::new(ListenerEnvironments(peers.clone())));
        let state = ProductRouteState::new(
            ResourceId::new(gateway.clone()).unwrap(),
            OrganizationId::new(organization).unwrap(),
            AudienceId::new(gateway).unwrap(),
            ProductDependencies {
                verifier: auth.clone(),
                access: auth.clone(),
                clock: Arc::new(SystemClock),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                uptime_clock: RuntimeDependencies::default().clock,
                agent_probe: Arc::new(NoAgents),
            },
        );
        let bound = bind(prepared, RuntimeDependencies::default().clock, state)
            .await
            .unwrap();
        let native = bound.local_address();
        let (failure, _) = watch::channel(None);
        Self {
            auth,
            session,
            owner,
            commands,
            peers,
            gate,
            native,
            running: Some(start(bound, failure)),
        }
    }

    /// A machine pairs into a new invitation of `class`, which the owner
    /// approves: its key and the gateway's pin, and its credential.
    async fn pair(&self, root: &Path, class: ConsentClass) -> Machine {
        let created = self.commands.create(&self.session, class).await.unwrap();
        nessa_local_storage::create_directory(root).unwrap();
        let root = root.canonicalize().unwrap();
        nessa_local_storage::create_directory_beneath(&root, Path::new("state")).unwrap();
        let store = Arc::new(FilePairingState::open(&root, Path::new("state")).unwrap());
        let client = NativeEnrollmentClient::enrolling(
            class,
            store.clone(),
            RuntimeDependencies::default().clock,
        );
        let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
        let connect = || std::net::TcpStream::connect(self.native).unwrap();
        tokio::time::timeout(WAIT, client.enroll(connect(), code, OsEntropy))
            .await
            .unwrap()
            .unwrap();
        let id = created.record().id();
        let claimed = self.commands.status(&self.session, id).await.unwrap();
        let (_, key) = claimed.claim_binding().unwrap();
        self.commands.approve(&self.session, id, key).await.unwrap();
        let status = tokio::time::timeout(WAIT, client.status(connect(), None))
            .await
            .unwrap()
            .unwrap();
        assert!(
            matches!(status, NativePairingStatus::Active { .. }),
            "{status:?}"
        );
        client.shutdown().await;
        let saved = store.load_credential().unwrap().unwrap();
        let credential = saved.credential().as_str().to_owned();
        let (key_material, pin, _) = saved.into_enrollment().into_parts();
        Machine {
            identity: Arc::new(NativeIdentity::restore(key_material).unwrap()),
            pin,
            peer: PeerEnvironment::new(*key.bytes()),
            credential,
        }
    }

    /// Wait until `peer` is connected, or no longer, as `connected` says.
    async fn until_connected(&self, peer: PeerEnvironment, connected: bool) {
        tokio::time::timeout(WAIT, async {
            while self.peers.connected().contains(&peer) != connected {
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("the paired environment's connection never changed");
    }

    /// Run `echo <word>` on `peer` under a command lease of its own.
    async fn echo(
        &self,
        peer: PeerEnvironment,
        word: &str,
    ) -> Result<CommandResult, CommandRefusal> {
        let environment = self.peers.environment(&peer);
        let commands = environment
            .commands()
            .expect("a paired environment runs commands");
        let lease = LeaseId::new(uuid()).unwrap();
        let work = CommandWork::new(vec!["echo".into(), word.into()], None, 5_000).unwrap();
        let hold = tokio::time::timeout(WAIT, commands.grant(&lease, &work))
            .await
            .unwrap()?
            .hold;
        let (_stop, stopped) = watch::channel(None);
        Ok(
            tokio::time::timeout(WAIT, hold.run(work.timeout_ms(), stopped))
                .await
                .unwrap(),
        )
    }
}

impl Drop for Gateway {
    fn drop(&mut self) {
        // The listener's task is let go with the runtime.
        drop(self.running.take());
    }
}

/// One paired machine: what it dials with, and what it is called.
struct Machine {
    identity: Arc<NativeIdentity>,
    pin: [u8; 44],
    peer: PeerEnvironment,
    credential: String,
}

/// Machine E, serving: `nessa env serve`'s frame loop over one connection it
/// dialed to the gateway `pin` names, with a command runner that echoes,
/// and commands granted to that gateway.
fn serving(
    root: &Path,
    pin: [u8; 44],
    stream: crate::device_pairing::infrastructure::TlsStream,
) -> JoinHandle<()> {
    let host = Host::open(root, pin);
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    tokio::spawn(async move { host.serve(from_gateway, to_gateway).await })
}

/// E's serving host, as `env serve` composes it: one ledger, outbox and
/// launcher for every connection it serves, the gateway it dials, and what
/// its owner grants that gateway.
struct Host {
    ledger: Arc<FileLedger>,
    outbox: Arc<FileOutbox>,
    launcher: Arc<ConfiguredLauncher>,
    peer: Peer,
    grants: Arc<FileGrants>,
    /// Where `ledger` is written.
    ledger_path: std::path::PathBuf,
}

impl Host {
    /// A host whose owner granted the gateway `pin` names commands.
    fn open(root: &Path, pin: [u8; 44]) -> Self {
        let host = Self::ungranted(root, pin);
        host.grant_commands();
        host
    }

    /// A host whose owner granted the gateway `pin` names nothing yet.
    fn ungranted(root: &Path, pin: [u8; 44]) -> Self {
        Self::running(root, pin, HashMap::new())
    }

    /// A host whose configuration starts `agents`, its owner granting the
    /// gateway `pin` names nothing yet.
    fn running(
        root: &Path,
        pin: [u8; 44],
        agents: HashMap<nessa_protocol::agents::AgentId, LaunchSpec>,
    ) -> Self {
        nessa_local_storage::create_directory(root).unwrap();
        let workspace = root.canonicalize().unwrap();
        Self {
            ledger_path: workspace.join("leases.jsonl"),
            peer: Peer::Gateway { pin },
            grants: Arc::new(FileGrants::new(&workspace)),
            ledger: Arc::new(FileLedger::open(workspace.join("leases.jsonl")).unwrap()),
            outbox: Arc::new(
                FileOutbox::open(
                    workspace.join("outbox"),
                    workspace.join("sockets"),
                    &workspace,
                )
                .unwrap(),
            ),
            launcher: Arc::new(ConfiguredLauncher::new(
                workspace.to_string_lossy().into_owned(),
                agents,
            )),
        }
    }

    /// What `nessa env grant --commands` does for this host's gateway.
    fn grant_commands(&self) {
        let commands = CommandGrant::new(CommandWork::MAX_TIMEOUT_MS).unwrap();
        self.grants
            .change(&self.peer, |_| {
                Ok::<_, ()>(Some(PeerGrant::nothing().with_commands(commands)))
            })
            .unwrap();
    }

    async fn serve<R, W>(&self, input: R, output: W)
    where
        R: tokio::io::AsyncRead + Unpin + Send + 'static,
        W: tokio::io::AsyncWrite + Unpin + Send + 'static,
    {
        serve(
            input,
            output,
            VERSION,
            LEASE_PROTOCOL,
            Serving {
                peer: self.peer,
                grants: self.grants.clone(),
            },
            Runners {
                harnesses: self.launcher.clone(),
                commands: Some(Arc::new(Echo)),
            },
            self.ledger.clone(),
            self.outbox.clone(),
            ServeTimings::default(),
            Arc::new(TokioTimer::new()),
        )
        .await;
    }
}

/// Answers `echo` with its arguments.
struct Echo;

impl CommandRunner for Echo {
    fn run(
        &self,
        command: CommandWork,
        _stop: watch::Receiver<Option<CommandStop>>,
    ) -> Pin<Box<dyn Future<Output = CommandRan> + Send + 'static>> {
        Box::pin(async move {
            CommandRan {
                end: CommandEnd::Exited { code: 0 },
                stdout: command.argv()[1..].join(" ").into_bytes(),
                stderr: Vec::new(),
                dropped_bytes: 0,
                cleanup: Cleanup::Confirmed { forced: false },
            }
        })
    }
}

fn clock() -> Arc<dyn nessa_protocol::clock::Clock> {
    RuntimeDependencies::default().clock
}

/// Row I2: E pairs as an environment, dials A, and A runs a command there
/// over E's own connection. A second connection from E takes the place of
/// the first, which A closes. Once E's credential is revoked, A's next
/// command is refused unreachable, A closes the connection, and E's serving
/// ends; E is then refused at `openEnvironment`.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_environment_dials_in_serves_and_stops_at_revocation() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    // Not connected: nothing to run on yet.
    assert_eq!(
        a.echo(e.peer, "early").await.unwrap_err(),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable)
    );

    let first = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .unwrap();
    let first = serving(&directory.path().join("e"), e.pin, first);
    a.until_connected(e.peer, true).await;
    let ran = a.echo(e.peer, "hello").await.unwrap();
    assert_eq!(ran.exit, CommandExit::Exited { code: 0 });
    assert_eq!(ran.stdout, b"hello");
    assert_eq!(ran.cleanup, Some(LeaseCleanup::Confirmed { forced: false }));

    // A second connection replaces the first, whose serving ends.
    let second = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .unwrap();
    let second = serving(&directory.path().join("e2"), e.pin, second);
    tokio::time::timeout(WAIT, first)
        .await
        .expect("the replaced connection's serving ends")
        .unwrap();
    a.until_connected(e.peer, true).await;
    assert_eq!(a.echo(e.peer, "again").await.unwrap().stdout, b"again");

    a.auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: uuid(),
            issuer_principal_id: a.owner.clone(),
            credential_id: e.credential.clone(),
            revoked_at: SystemClock.unix_seconds(),
        })
        .unwrap();
    assert_eq!(
        a.echo(e.peer, "revoked").await.unwrap_err(),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable)
    );
    tokio::time::timeout(WAIT, second)
        .await
        .expect("the revoked environment's serving ends")
        .unwrap();
    a.until_connected(e.peer, false).await;
    let refused = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .err()
        .expect("a revoked environment is refused");
    assert_eq!(refused.kind(), ErrorKind::PermissionDenied);
}

/// Row I4 end to end: E serves the gateway it dialed only what E's owner
/// granted it. With no grant, A's command is refused not granted, by E,
/// over the live connection; once E's owner grants commands, A's next one
/// runs, on the same connection.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_dialed_gateway_runs_only_what_the_environment_granted_it() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .unwrap();
    let host = Arc::new(Host::ungranted(&directory.path().join("e"), e.pin));
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    let _serving = {
        let host = host.clone();
        tokio::spawn(async move { host.serve(from_gateway, to_gateway).await })
    };
    a.until_connected(e.peer, true).await;
    assert_eq!(
        a.echo(e.peer, "ungranted").await.unwrap_err(),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentNotGranted)
    );
    host.grant_commands();
    assert_eq!(a.echo(e.peer, "granted").await.unwrap().stdout, b"granted");
}

/// A key paired only to read, as a peer gateway, holds no credential to
/// serve: `openEnvironment` refuses it before it takes a permit, and the
/// gateway lists no environment for it.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_key_paired_to_read_is_refused_at_open_environment() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let reader = a
        .pair(
            &directory.path().join("reader-state"),
            ConsentClass::PeerRead,
        )
        .await;
    let refused = dial(a.native, reader.identity.clone(), reader.pin, clock())
        .await
        .err()
        .expect("a reading peer is refused");
    assert_eq!(refused.kind(), ErrorKind::PermissionDenied);
    assert!(a.peers.connected().is_empty());
}

/// Review finding on #748: a lease asks whether the key still serves, and
/// the environment dials again while that ask is pending. The answer is
/// about the key, so the lease runs on the connection that replaced the one
/// it saw, rather than being refused for the connection now closed.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_lease_asked_across_a_reconnect_runs_on_the_new_connection() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let first = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .unwrap();
    let first = serving(&directory.path().join("e"), e.pin, first);
    a.until_connected(e.peer, true).await;

    a.gate.armed.store(true, Ordering::SeqCst);
    let a = Arc::new(a);
    let racing = {
        let a = a.clone();
        tokio::spawn(async move { a.echo(e.peer, "racing").await })
    };
    tokio::time::timeout(WAIT, a.gate.entered.notified())
        .await
        .expect("the lease asks whether the key serves");
    let second = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .unwrap();
    let _second = serving(&directory.path().join("e2"), e.pin, second);
    tokio::time::timeout(WAIT, first)
        .await
        .expect("the replaced connection's serving ends")
        .unwrap();
    a.until_connected(e.peer, true).await;
    a.gate.released.notify_one();
    let ran = racing.await.unwrap().unwrap();
    assert_eq!(ran.stdout, b"racing");
}

/// Part 1c: E pairs with `env enroll`'s pairing, waits for the owner's
/// approval as `env dial` does, then stays dialed in: one host serves each
/// connection `serve_until_stopped` is given, a connection that is cut is
/// dialed again, and stop closes the one being served.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_enrolled_environment_waits_for_approval_then_stays_dialed_in() {
    use super::env_dial_command::serve_until_stopped;
    use crate::env_serve::application::{approval, redial, Backoff};
    use crate::env_serve::infrastructure::FilePairingEnvironment;
    use std::sync::Mutex;

    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let created = a
        .commands
        .create(&a.session, ConsentClass::PeerEnvironment)
        .await
        .unwrap();
    let environment = directory.path().join("e-environment");
    nessa_local_storage::create_directory(&environment).unwrap();
    let native = a.native;
    let pairing = FilePairingEnvironment::open(&environment, &native.to_string(), clock()).unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    tokio::time::timeout(WAIT, pairing.enroll(code))
        .await
        .unwrap()
        .unwrap();

    // Not approved yet: waiting is still waiting.
    let (_running, running) = watch::channel(false);
    let poll = Duration::from_millis(20);
    assert!(tokio::time::timeout(
        Duration::from_millis(300),
        approval(&pairing, poll, &TokioTimer::new(), running.clone())
    )
    .await
    .is_err());
    let id = created.record().id();
    let claimed = a.commands.status(&a.session, id).await.unwrap();
    let (_, key) = claimed.claim_binding().unwrap();
    a.commands.approve(&a.session, id, key).await.unwrap();
    let (identity, pin) =
        tokio::time::timeout(WAIT, approval(&pairing, poll, &TokioTimer::new(), running))
            .await
            .unwrap()
            .unwrap()
            .expect("not stopped");
    pairing.shutdown().await;
    let peer = PeerEnvironment::new(*key.bytes());

    let host = Arc::new(Host::open(&directory.path().join("e"), pin));
    let served = Arc::new(Mutex::new(Vec::new()));
    // Dials after the first wait for this, so the test sees the gateway
    // drop the cut connection before another one is made.
    let (release, released) = watch::channel(false);
    let dials = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let (stop, stopping) = watch::channel(false);
    let dialing = {
        let served = served.clone();
        tokio::spawn(async move {
            redial(
                move || {
                    let identity = identity.clone();
                    let mut released = released.clone();
                    let n = dials.fetch_add(1, Ordering::SeqCst);
                    async move {
                        if n > 0 {
                            let _ = released.wait_for(|go| *go).await;
                        }
                        dial(native, identity, pin, clock()).await
                    }
                },
                move |stream, stopping| {
                    served.lock().unwrap().push(stream.closer());
                    let host = host.clone();
                    async move {
                        serve_until_stopped(stream, stopping, |input, output| async move {
                            host.serve(input, output).await;
                        })
                        .await;
                    }
                },
                Backoff {
                    first: Duration::from_millis(10),
                    most: Duration::from_millis(100),
                },
                &TokioTimer::new(),
                stopping,
            )
            .await
        })
    };
    a.until_connected(peer, true).await;
    let ran = a.echo(peer, "first").await.unwrap();
    assert_eq!(ran.stdout, b"first");

    // The first connection is cut: E dials again, and the same host serves.
    served.lock().unwrap()[0].close();
    a.until_connected(peer, false).await;
    release.send(true).unwrap();
    tokio::time::timeout(WAIT, async {
        while served.lock().unwrap().len() < 2 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("a cut connection is dialed again");
    a.until_connected(peer, true).await;
    let ran = a.echo(peer, "dialed in again").await.unwrap();
    assert_eq!(ran.stdout, b"dialed in again");

    // Stop closes the connection being served, through serve_until_stopped.
    stop.send(true).unwrap();
    tokio::time::timeout(WAIT, dialing)
        .await
        .expect("stop ends dialing")
        .unwrap()
        .unwrap();
    a.until_connected(peer, false).await;
    assert_eq!(served.lock().unwrap().len(), 2);
}

/// Part 1c: waiting for approval ends, with a reason, when the owner denies
/// the machine, and when nothing was ever paired here, even with the
/// gateway out of reach.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn waiting_for_approval_ends_when_denied_or_never_paired() {
    use crate::env_serve::application::{approval, NotApproved};
    use crate::env_serve::infrastructure::FilePairingEnvironment;
    use nessa_auth::application::pairing::OwnerDecision;

    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let native = a.native.to_string();
    let (_running, running) = watch::channel(false);
    let poll = Duration::from_millis(20);

    // Told at once, with no gateway to ask: nothing listens at this address.
    let nowhere = std::net::TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
        .to_string();
    let unpaired = directory.path().join("unpaired");
    nessa_local_storage::create_directory(&unpaired).unwrap();
    let pairing = FilePairingEnvironment::open(&unpaired, &nowhere, clock()).unwrap();
    let never = tokio::time::timeout(
        WAIT,
        approval(&pairing, poll, &TokioTimer::new(), running.clone()),
    )
    .await
    .unwrap();
    assert_eq!(never.err(), Some(NotApproved::NotPaired));
    pairing.shutdown().await;

    let created = a
        .commands
        .create(&a.session, ConsentClass::PeerEnvironment)
        .await
        .unwrap();
    let denied = directory.path().join("denied");
    nessa_local_storage::create_directory(&denied).unwrap();
    let pairing = FilePairingEnvironment::open(&denied, &native, clock()).unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    tokio::time::timeout(WAIT, pairing.enroll(code))
        .await
        .unwrap()
        .unwrap();
    a.commands
        .decide(&a.session, created.record().id(), OwnerDecision::Deny)
        .await
        .unwrap();
    let ended = tokio::time::timeout(WAIT, approval(&pairing, poll, &TokioTimer::new(), running))
        .await
        .unwrap();
    assert_eq!(ended.err(), Some(NotApproved::Ended));
    pairing.shutdown().await;
}

/// Part 1c: a pin that is not a key is this side's own setup, which dialing
/// again cannot change, so it stops redialing; a key that is not this
/// gateway's is the gateway's answer, and is dialed again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pin_that_is_not_a_key_is_invalid_input_and_another_key_is_not() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let not_a_key = dial(a.native, e.identity.clone(), [0; 44], clock())
        .await
        .err()
        .expect("a pin that is not a key cannot dial");
    assert_eq!(not_a_key.kind(), ErrorKind::InvalidInput);
    let other = e.identity.public_spki();
    let not_the_gateway = dial(a.native, e.identity.clone(), other, clock())
        .await
        .err()
        .expect("another key's pin does not match the gateway");
    assert_eq!(not_the_gateway.kind(), ErrorKind::ConnectionAborted);
}

/// A binding that runs its harness on whichever host it is given, and
/// remembers that host.
struct OnHost {
    capabilities: nessa_sdk::domain::effective_capabilities::value_objects::EffectiveCapabilities,
    host: Arc<std::sync::Mutex<Option<Arc<dyn HarnessHost>>>>,
}

impl AgentProvider for OnHost {
    fn identity(&self) -> ProviderIdentity {
        ProviderIdentity::new("fixture", "model", "context").expect("a fixed identity is valid")
    }
    fn capabilities(
        &self,
    ) -> &nessa_sdk::domain::effective_capabilities::value_objects::EffectiveCapabilities {
        &self.capabilities
    }
    fn open(&self, _request: ProviderOpenRequest) -> ProviderOpenFuture<'_> {
        Box::pin(async { Err(ProviderOpenError::no_resources(AgentError::Closed)) })
    }
    fn on_host(&self, host: Arc<dyn HarnessHost>) -> Result<Arc<dyn AgentProvider>, AgentError> {
        *self.host.lock().unwrap() = Some(host);
        Ok(Arc::new(Self {
            capabilities: self.capabilities.clone(),
            host: self.host.clone(),
        }))
    }
}

/// Row I7 end to end: E's owner lets A's agent leases last a while. A asks
/// for one with no deadline and is granted it with that duration; past it,
/// E stops the harness, records the lease expiring and ended as expired,
/// and says so; A's hold sees the lease ended as expired, and A's End is
/// answered with what E's own end took.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_environment_expires_a_lease_and_both_sides_record_it() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .unwrap();
    // E runs `claude` as `cat`, which runs until it is stopped.
    let cat = LaunchSpec {
        executable: "/bin/cat".into(),
        args: Vec::new(),
        environment: Default::default(),
    };
    let host = Arc::new(Host::running(
        &directory.path().join("e"),
        e.pin,
        HashMap::from([(nessa_protocol::agents::AgentId::Claude, cat)]),
    ));
    let max_lease = MaxLease::new(1_500).unwrap();
    host.grants
        .change(&host.peer, |_| {
            Ok::<_, ()>(Some(
                PeerGrant::nothing()
                    .with_agents(Agents::All)
                    .with_max_lease(max_lease),
            ))
        })
        .unwrap();
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    let _serving = {
        let host = host.clone();
        tokio::spawn(async move { host.serve(from_gateway, to_gateway).await })
    };
    a.until_connected(e.peer, true).await;

    let environment = a.peers.environment(&e.peer);
    let lease = LeaseId::new(uuid()).unwrap();
    let terms = LeaseTerms {
        environment: environment.declaration().environment,
        work: LeaseWork::Agent(AgentWork::new("claude", "model").unwrap()),
        sandbox: SandboxProfile::HarnessDefault,
        grants: LeaseGrants::Opening,
        deadline: LeaseDeadline::UntilEnded,
    };
    let given = Arc::new(std::sync::Mutex::new(None));
    let binding = Arc::new(OnHost {
        capabilities: crate::conversation_test_support::Provider::new(Arc::new(
            crate::conversation_test_support::ProviderFactory::default(),
        ))
        .capabilities()
        .clone(),
        host: given.clone(),
    });
    let opened = tokio::time::timeout(WAIT, environment.open(&lease, &terms, binding))
        .await
        .unwrap()
        .unwrap_or_else(|refusal| panic!("granted: {refusal:?}"));
    // Granted with the duration E's owner set, though none was asked.
    assert_eq!(opened.granted.lease_ms, Some(1_500));
    let harness_host = given.lock().unwrap().clone().unwrap();
    let mut harness = harness_host.start(HarnessLaunch::default()).unwrap();
    let ends = opened
        .hold
        .ends()
        .expect("a paired environment's lease can end there");
    assert_eq!(
        tokio::time::timeout(WAIT, ends).await.expect("E ends it"),
        LeaseEndCause::Expired
    );
    // A's End is answered with E's own end's cleanup.
    let released = tokio::time::timeout(WAIT, opened.hold.end(LeaseEndCause::Expired))
        .await
        .unwrap();
    assert!(
        matches!(
            released,
            LeaseRelease::Released(LeaseCleanup::Confirmed { .. })
        ),
        "{released:?}"
    );
    // The harness was stopped there: its output ends.
    let mut rest = Vec::new();
    let _ = tokio::time::timeout(WAIT, harness.output.read_to_end(&mut rest)).await;
    let entries: Vec<serde_json::Value> = std::fs::read_to_string(&host.ledger_path)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .filter(|entry: &serde_json::Value| entry["lease"] == lease.as_str())
        .collect();
    let kinds: Vec<_> = entries
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        kinds,
        ["granted", "started", "expiring", "stopped", "ended"],
        "{entries:?}"
    );
    assert_eq!(entries[0]["leaseMs"], 1_500);
    assert_eq!(entries[2]["leaseMs"], 1_500);
    assert_eq!(entries[4]["cause"], "expired");
    assert_eq!(entries[4]["cleanup"]["outcome"], "confirmed");
}
