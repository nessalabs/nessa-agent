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
use super::paired_environments::{ListenerEnvironments, RegistryPaired, RegistryServing};
use super::peer_reading_tests::{inputs, owned, uuid, NoAgents, WAIT};
use super::runtime_config::NativeConfig;
use crate::app::dependencies::RuntimeDependencies;
use crate::conversation::application::LeaseRelease;
use crate::conversation::application::{
    CommandResult, EnvironmentFuture, OrganizationBudgets, PairedEnvironments, WorkerLoad,
};
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
    dial as dial_with_secret, ConfiguredLauncher, FileGrants, FileLedger, FileOutbox, LaunchSpec,
};

async fn dial(
    address: String,
    relay: Option<nessa_protocol::relay::RelayAddress>,
    tunnel: Option<nessa_protocol::tunnel::TunnelAddress>,
    identity: std::sync::Arc<nessa_auth::adapters::pairing::NativeIdentity>,
    pin: [u8; 44],
    clock: std::sync::Arc<dyn nessa_protocol::clock::Clock>,
) -> std::io::Result<crate::device_pairing::infrastructure::TlsStream> {
    dial_with_secret(address, relay, tunnel, None, identity, pin, clock).await
}
use crate::product::{ProductDependencies, ProductRouteState};
use crate::timer::{manual::ManualTimer, TokioTimer};
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
use nessa_protocol::relay::{RelayAddress, RelayRefusal};
use nessa_sdk::application::agent_execution::providers::HarnessHostIdentity;
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
    /// Each connection that ended having reported its machine.
    departed: Arc<Departed>,
    /// Which keys the registry says are paired, as `devices.list` asks.
    paired: RegistryPaired,
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

/// Records each report and departure [`PeerEnvironments`] tells of.
#[derive(Default)]
struct Departed {
    reported: std::sync::Mutex<Vec<(PeerEnvironment, nessa_protocol::lease::MachineReport)>>,
    left: std::sync::Mutex<Vec<(PeerEnvironment, nessa_protocol::lease::MachineReport)>>,
}

impl crate::conversation::infrastructure::ssh_environment::Sightings for Departed {
    fn reported(&self, peer: PeerEnvironment, machine: &nessa_protocol::lease::MachineReport) {
        self.reported.lock().unwrap().push((peer, machine.clone()));
    }
    fn left(&self, peer: PeerEnvironment, machine: nessa_protocol::lease::MachineReport) {
        self.left.lock().unwrap().push((peer, machine));
    }
}

impl Gateway {
    async fn start(root: &Path) -> Self {
        Self::start_with(root, SshTimings::default()).await
    }

    /// As [`Self::start`], its paired environments kept with `timings`.
    async fn start_with(root: &Path, timings: SshTimings) -> Self {
        Self::start_at(root, timings, None, OrganizationBudgets::default()).await
    }

    /// As [`Self::start`], its paired environments sharing `budgets`.
    async fn start_budgeted(root: &Path, budgets: OrganizationBudgets) -> Self {
        Self::start_at(root, SshTimings::default(), None, budgets).await
    }

    /// As [`Self::start`], registered at `relay` as `native.relay` has it.
    async fn start_relayed(root: &Path, relay: RelayAddress) -> Self {
        Self::start_at(
            root,
            SshTimings::default(),
            Some(relay),
            OrganizationBudgets::default(),
        )
        .await
    }

    async fn start_at(
        root: &Path,
        timings: SshTimings,
        relay: Option<RelayAddress>,
        budgets: OrganizationBudgets,
    ) -> Self {
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
                relay,
                advertised_address: None,
                tunnel: None,
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
        let departed = Arc::new(Departed::default());
        let paired = RegistryPaired {
            store: auth.clone(),
            audience: AudienceId::new(gateway.clone()).unwrap(),
        };
        let peers = Arc::new(
            PeerEnvironments::new(
                gate.clone(),
                DurableEnvironmentAudit::new(root.join("environments"), Arc::new(SystemClock))
                    .unwrap(),
                timings,
                budgets,
            )
            .with_sightings(departed.clone()),
        );
        let prepared = prepared.with_environments(Arc::new(ListenerEnvironments(peers.clone())));
        let state = ProductRouteState::new(
            ResourceId::new(gateway.clone()).unwrap(),
            OrganizationId::new(organization).unwrap(),
            AudienceId::new(gateway).unwrap(),
            ProductDependencies {
                owner_phone_purpose: auth.clone(),
                owner_phone_policy: Arc::new(
                    crate::conversation::infrastructure::CedarOwnerPhonePolicyEvaluator::new()
                        .unwrap(),
                ),
                owner_phone_receivers: None,
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
            departed,
            paired,
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
    /// What its serving waits on.
    timer: Arc<dyn crate::timer::Timer>,
    /// Asks it to drain, as `env dial`'s signal does.
    drain: Arc<watch::Sender<bool>>,
    /// What it reports of its machine; nothing unless a test says.
    machine: Option<Arc<dyn crate::devices::application::MachineProbe>>,
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
            timer: Arc::new(TokioTimer::new()),
            drain: Arc::new(watch::channel(false).0),
            machine: None,
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
                policy: Arc::new(
                    crate::env_serve::infrastructure::CedarLeasePolicy::new().unwrap(),
                ),
                machine: self.machine.clone(),
            },
            Runners {
                harnesses: self.launcher.clone(),
                commands: Some(Arc::new(Echo)),
            },
            self.ledger.clone(),
            self.outbox.clone(),
            None,
            ServeTimings::default(),
            self.timer.clone(),
            self.drain.subscribe(),
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
        _publish_point: Option<String>,
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

    let first = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let first = serving(&directory.path().join("e"), e.pin, first);
    a.until_connected(e.peer, true).await;
    let ran = a.echo(e.peer, "hello").await.unwrap();
    assert_eq!(ran.exit, CommandExit::Exited { code: 0 });
    assert_eq!(ran.stdout, b"hello");
    assert_eq!(ran.cleanup, Some(LeaseCleanup::Confirmed { forced: false }));

    // A second connection replaces the first, whose serving ends.
    let second = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let second = serving(&directory.path().join("e2"), e.pin, second);
    tokio::time::timeout(WAIT, first)
        .await
        .expect("the replaced connection's serving ends")
        .unwrap();
    a.until_connected(e.peer, true).await;
    assert_eq!(a.echo(e.peer, "again").await.unwrap().stdout, b"again");

    use crate::devices::application::PairedKeys;
    assert!(a.paired.paired().unwrap().contains(&e.peer));
    a.auth
        .revoke_sync(RevokeCredentialRequest {
            request_id: uuid(),
            issuer_principal_id: a.owner.clone(),
            credential_id: e.credential.clone(),
            revoked_at: SystemClock.unix_seconds(),
        })
        .unwrap();
    // Revoked, its key is no longer paired: Devices forgets it.
    assert!(!a.paired.paired().unwrap().contains(&e.peer));
    assert_eq!(
        a.echo(e.peer, "revoked").await.unwrap_err(),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable)
    );
    tokio::time::timeout(WAIT, second)
        .await
        .expect("the revoked environment's serving ends")
        .unwrap();
    a.until_connected(e.peer, false).await;
    let refused = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .err()
    .expect("a revoked environment is refused");
    assert_eq!(refused.kind(), ErrorKind::PermissionDenied);
}

/// The organization's budget is shared by every paired environment: with
/// one command allowed, a second is refused busy before the environment is
/// asked, and runs once the first's lease has ended on the environment.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_organization_budget_refuses_a_command_past_it_until_one_ends() {
    let directory = tempfile::tempdir().unwrap();
    let budgets = OrganizationBudgets::new(None, Some(1));
    let a = Gateway::start_budgeted(&directory.path().join("a"), budgets.clone()).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let _serving = serving(&directory.path().join("e"), e.pin, stream);
    a.until_connected(e.peer, true).await;

    let environment = a.peers.environment(&e.peer);
    let commands = environment.commands().unwrap();
    let work = CommandWork::new(vec!["echo".into(), "held".into()], None, 5_000).unwrap();
    let held = tokio::time::timeout(WAIT, commands.grant(&LeaseId::new(uuid()).unwrap(), &work))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(budgets.running(), (0, 1));
    assert_eq!(
        a.echo(e.peer, "past").await.unwrap_err(),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentBusy)
    );
    let (_stop, stopped) = watch::channel(None);
    let ran = tokio::time::timeout(WAIT, held.hold.run(work.timeout_ms(), stopped))
        .await
        .unwrap();
    assert_eq!(ran.stdout, b"held");
    // Its place is given back once the host has let its lease go, not as
    // its hold is: until then the worker may still hold the work.
    tokio::time::timeout(WAIT, async {
        while budgets.running() != (0, 0) {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(a.echo(e.peer, "after").await.unwrap().stdout, b"after");
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
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
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
    let refused = dial(
        a.native.to_string(),
        None,
        None,
        reader.identity.clone(),
        reader.pin,
        clock(),
    )
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
    let first = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
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
    let second = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
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
    tokio::time::timeout(WAIT, crate::env_serve::application::enroll(&pairing, code))
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
    let (identity, pin, _) =
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
                        dial(native.to_string(), None, None, identity, pin, clock()).await
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
                watch::channel(false).1,
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
    tokio::time::timeout(WAIT, crate::env_serve::application::enroll(&pairing, code))
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
    let not_a_key = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        [0; 44],
        clock(),
    )
    .await
    .err()
    .expect("a pin that is not a key cannot dial");
    assert_eq!(not_a_key.kind(), ErrorKind::InvalidInput);
    let other = e.identity.public_spki();
    let not_the_gateway = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        other,
        clock(),
    )
    .await
    .err()
    .expect("another key's pin does not match the gateway");
    assert_eq!(not_the_gateway.kind(), ErrorKind::ConnectionAborted);
}

/// A relay on this runtime, as `nessa relay serve` runs one; it ends with
/// the test's runtime.
async fn relay() -> RelayAddress {
    use crate::relay::{
        application::{Relay, RelayLimits},
        infrastructure::{serve_tcp, Ed25519Proofs, OsRelayEntropy},
    };
    let socket = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = RelayAddress::try_from(socket.local_addr().unwrap().to_string()).unwrap();
    let timer: Arc<dyn crate::timer::Timer> = Arc::new(TokioTimer::new());
    let relay = Relay::new(
        timer.clone(),
        Arc::new(OsRelayEntropy),
        Arc::new(Ed25519Proofs),
        RelayLimits::default(),
    );
    tokio::spawn(serve_tcp(socket, relay, timer, std::future::pending()));
    address
}

/// Port 0 is never a listener's: a direct dial to it fails at once, and no
/// other test can take it in the meantime.
fn nowhere() -> String {
    "127.0.0.1:0".to_owned()
}

/// R4, row RL14 in `docs/design/auth/relay.md`: an environment that
/// enrolled with the gateway, and then has only a relay path to it, waits
/// for the owner's approval through the relay, as `env dial --relay` does,
/// then dials through it and serves leases as over a direct connection.
/// A command run each way leaves the same command-ledger checkpoint.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_environment_with_only_a_relay_path_is_approved_and_serves_through_it() {
    use crate::env_serve::application::approval;
    use crate::env_serve::infrastructure::FilePairingEnvironment;

    let directory = tempfile::tempdir().unwrap();
    let relay = relay().await;
    let a = Gateway::start_relayed(&directory.path().join("a"), relay.clone()).await;
    let created = a
        .commands
        .create(&a.session, ConsentClass::PeerEnvironment)
        .await
        .unwrap();
    let environment = directory.path().join("e-environment");
    nessa_local_storage::create_directory(&environment).unwrap();
    // Enrolling learns the gateway's key, so it needs the direct path.
    let pairing =
        FilePairingEnvironment::open(&environment, &a.native.to_string(), clock()).unwrap();
    let code = ManualCode::parse(created.code().expose_bytes()).unwrap();
    tokio::time::timeout(WAIT, crate::env_serve::application::enroll(&pairing, code))
        .await
        .unwrap()
        .unwrap();
    pairing.shutdown().await;
    // Its directory's lock goes with it.
    drop(pairing);

    // From here the gateway's own address refuses: only the relay reaches it.
    let unreachable = nowhere();
    let id = created.record().id();
    let claimed = a.commands.status(&a.session, id).await.unwrap();
    let (_, key) = claimed.claim_binding().unwrap();
    a.commands.approve(&a.session, id, key).await.unwrap();
    let pairing = FilePairingEnvironment::open(&environment, &unreachable, clock())
        .unwrap()
        .through(Some(relay.clone()));
    let (_running, running) = watch::channel(false);
    let poll = Duration::from_millis(50);
    // The gateway may not have registered yet: the relay's `Offline` is
    // asked again, as any unanswered status is.
    let (identity, pin, _) =
        tokio::time::timeout(WAIT, approval(&pairing, poll, &TokioTimer::new(), running))
            .await
            .expect("approved through the relay")
            .unwrap()
            .expect("not stopped");
    pairing.shutdown().await;
    let peer = PeerEnvironment::new(*key.bytes());

    // Without the relay the gateway is out of reach.
    let direct = dial(
        unreachable.clone(),
        None,
        None,
        identity.clone(),
        pin,
        clock(),
    )
    .await
    .err()
    .expect("nothing listens at the direct address");
    // Refused on Linux, an unusable address on macOS: either way the
    // direct dial's own error, not the relay's.
    assert_ne!(direct.kind(), ErrorKind::InvalidInput);
    assert_eq!(RelayRefusal::carried(&direct), None);
    let stream = dial(unreachable, Some(relay), None, identity, pin, clock())
        .await
        .unwrap();
    let _serving = serving(&directory.path().join("e"), pin, stream);
    a.until_connected(peer, true).await;
    let ran = a.echo(peer, "through the relay").await.unwrap();
    assert_eq!(ran.stdout, b"through the relay");
    let relayed = ended_checkpoint(&directory.path().join("e")).await;

    // The same command over a direct connection, on the same gateway.
    let direct_machine = a
        .pair(
            &directory.path().join("direct-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        direct_machine.identity.clone(),
        direct_machine.pin,
        clock(),
    )
    .await
    .unwrap();
    let _direct = serving(&directory.path().join("direct"), direct_machine.pin, stream);
    a.until_connected(direct_machine.peer, true).await;
    let ran = a.echo(direct_machine.peer, "direct").await.unwrap();
    assert_eq!(ran.stdout, b"direct");
    let direct = ended_checkpoint(&directory.path().join("direct")).await;
    assert_eq!(relayed, direct);
    assert_eq!(
        serde_json::Value::Array(relayed),
        serde_json::json!([
            {
                "kind": "command_granted",
                "cwd": null,
                "timeoutMs": 5000,
                "askedTimeoutMs": 5000,
                "sandbox": "none"
            },
            {"kind": "command_started"},
            {
                "kind": "command_ran",
                "end": {"outcome": "exited", "code": 0},
                "droppedBytes": 0
            },
            {
                "kind": "ended",
                "cleanup": {"outcome": "confirmed", "forced": false},
                "cause": "asked"
            }
        ])
    );
}

/// `command_checkpoint` once the host has recorded the command's end.
/// `echo` returns at `Ran`; the end line is what the checkpoint compares.
async fn ended_checkpoint(root: &Path) -> Vec<serde_json::Value> {
    tokio::time::timeout(WAIT, async {
        loop {
            let entries = command_checkpoint(root);
            if entries
                .iter()
                .any(|entry| entry.get("kind").and_then(|kind| kind.as_str()) == Some("ended"))
            {
                return entries;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the host recorded the command's end")
}

/// A command lease's ledger with the lease id, the arguments and the peer
/// taken out, so a direct run and a relayed run can be compared.
fn command_checkpoint(root: &Path) -> Vec<serde_json::Value> {
    let path = root.canonicalize().unwrap().join("leases.jsonl");
    std::fs::read_to_string(&path)
        .unwrap_or_else(|error| panic!("reading {}: {error}", path.display()))
        .lines()
        .filter(|line| !line.is_empty())
        .map(|line| {
            let mut entry: serde_json::Value = serde_json::from_str(line).unwrap();
            let object = entry.as_object_mut().unwrap();
            object.remove("lease");
            object.remove("argv");
            object.remove("peer");
            entry
        })
        .collect()
}

/// R4: the relay's `Offline` reaches the dialer as the relay's own answer,
/// told apart from the direct dial's refusal, so it is dialed again.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_relay_without_the_gateway_answers_offline_to_an_environments_dial() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let offline = dial(
        nowhere(),
        Some(relay().await),
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .err()
    .expect("the gateway never registered at the relay");
    assert_eq!(RelayRefusal::carried(&offline), Some(RelayRefusal::Offline));
    assert_ne!(offline.kind(), ErrorKind::InvalidInput);
}

/// R4: a pending pairing whose saved pin is not a key cannot be asked for
/// at the relay, and asking again does not change that: waiting for
/// approval ends as unusable rather than asking forever.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_pending_pin_that_is_not_a_key_ends_waiting_through_a_relay() {
    use crate::env_serve::application::{approval, NotApproved};
    use crate::env_serve::infrastructure::FilePairingEnvironment;
    use nessa_auth::domain::pairing::{AttemptId, ConsentIntentId, InvitationId, PublicIntent};

    let directory = tempfile::tempdir().unwrap();
    let environment = directory.path().join("e-environment");
    nessa_local_storage::create_directory(&environment).unwrap();
    let root = environment.canonicalize().unwrap();
    nessa_local_storage::create_directory_beneath(&root, Path::new("pairing")).unwrap();
    let intent = PublicIntent::new(
        InvitationId::new([1; 16]),
        AttemptId::new([2; 16]),
        ConsentIntentId::new([3; 16]),
        1,
        1,
        ConsentClass::PeerEnvironment,
    )
    .unwrap();
    let identity = NativeIdentity::generate(&mut OsEntropy).unwrap();
    // Saved and let go, so the environment's pairing can take the lock.
    FilePairingState::open(&root, Path::new("pairing"))
        .unwrap()
        .save_pending(identity.key_material(), &[0; 44], intent, None)
        .unwrap();

    let pairing = FilePairingEnvironment::open(&environment, &nowhere(), clock())
        .unwrap()
        .through(Some(relay().await));
    let (_running, running) = watch::channel(false);
    let ended = tokio::time::timeout(
        WAIT,
        approval(
            &pairing,
            Duration::from_millis(20),
            &TokioTimer::new(),
            running,
        ),
    )
    .await
    .expect("told at once, not asked again");
    assert!(matches!(ended, Err(NotApproved::Unusable(_))), "{ended:?}");
    pairing.shutdown().await;
}

/// A binding that runs its harness on whichever host it is given, and
/// remembers that host.
struct OnHost {
    captured_host: Option<Arc<dyn HarnessHost>>,
    capabilities: nessa_sdk::domain::effective_capabilities::value_objects::EffectiveCapabilities,
    host: Arc<std::sync::Mutex<Option<Arc<dyn HarnessHost>>>>,
}

impl AgentProvider for OnHost {
    fn harness_host_identity(&self) -> HarnessHostIdentity<'_> {
        self.captured_host
            .as_ref()
            .map_or_else(HarnessHostIdentity::local, HarnessHostIdentity::hosted)
    }
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
        *self.host.lock().unwrap() = Some(host.clone());
        Ok(Arc::new(Self {
            captured_host: Some(host),
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
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    // E runs `claude` as `cat`, which runs until it is stopped. Its serving
    // waits on a timer only this test moves, so the lease's deadline comes
    // when the test says, never before its harness has started.
    let cat = LaunchSpec {
        executable: "/bin/cat".into(),
        args: Vec::new(),
        environment: Default::default(),
    };
    let timer = Arc::new(ManualTimer::default());
    let mut host = Host::running(
        &directory.path().join("e"),
        e.pin,
        HashMap::from([(nessa_protocol::agents::AgentId::Claude, cat)]),
    );
    host.timer = timer.clone();
    let host = Arc::new(host);
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
        captured_host: None,
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
    // Its start is recorded before the deadline is reached.
    let recorded = |kind: &'static str| {
        let ledger = std::fs::read_to_string(&host.ledger_path).unwrap();
        ledger.lines().any(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            entry["kind"] == kind && entry["lease"] == lease.as_str()
        })
    };
    tokio::time::timeout(WAIT, async {
        while !recorded("started") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the harness starts");
    let ends = opened
        .hold
        .ends()
        .expect("a paired environment's lease can end there");
    timer.advance(Duration::from_millis(1_500));
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
    tokio::time::timeout(WAIT, harness.output.read_to_end(&mut rest))
        .await
        .expect("the harness's output ends")
        .expect("and reads to its end");
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

/// Row K1: a paired environment asked to drain, as `env dial
/// --drain-deadline` is at a signal, ends its lease on both sides with E's
/// cleanup, never as lost: A sees the lease ending as the system stopping it
/// and asks its End, E answers with what stopping the harness took, and then
/// E stops serving and the connection closes.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn k1_a_paired_environment_drains_and_both_sides_record_its_cleanup() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
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
    host.grants
        .change(&host.peer, |_| {
            Ok::<_, ()>(Some(PeerGrant::nothing().with_agents(Agents::All)))
        })
        .unwrap();
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    let serving = {
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
        captured_host: None,
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
    let harness_host = given.lock().unwrap().clone().unwrap();
    let mut harness = harness_host.start(HarnessLaunch::default()).unwrap();
    let recorded = |kind: &'static str| {
        let ledger = std::fs::read_to_string(&host.ledger_path).unwrap();
        ledger.lines().any(|line| {
            let entry: serde_json::Value = serde_json::from_str(line).unwrap();
            entry["kind"] == kind && entry["lease"] == lease.as_str()
        })
    };
    tokio::time::timeout(WAIT, async {
        while !recorded("started") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the harness starts");
    // The pool reads what each worker carries off its connection.
    assert_eq!(
        a.peers.serving(),
        [(
            e.peer,
            WorkerLoad {
                leases: 1,
                draining: false
            }
        )]
    );
    let ends = opened
        .hold
        .ends()
        .expect("a paired environment's lease can end there");
    let mut changes = a.peers.connections();
    changes.borrow_and_update();
    host.drain.send_replace(true);
    // E says it is draining before anything else, so the pool sends it no
    // new lease while it is still connected, and whoever lists where work
    // can go is told to read the list again.
    tokio::time::timeout(WAIT, changes.changed())
        .await
        .expect("draining is said to whoever lists environments")
        .unwrap();
    assert!(a.peers.serving().iter().any(|(_, load)| load.draining));
    // A system stop, as the gateway retiring is: never a lost lease.
    assert_eq!(
        tokio::time::timeout(WAIT, ends).await.expect("E ends it"),
        LeaseEndCause::Stopped
    );
    // Draining, E has not stopped serving: it waits for A's End.
    assert!(!serving.is_finished());
    let released = tokio::time::timeout(WAIT, opened.hold.end(LeaseEndCause::Stopped))
        .await
        .unwrap();
    assert!(
        matches!(
            released,
            LeaseRelease::Released(LeaseCleanup::Confirmed { .. })
        ),
        "{released:?}"
    );
    // Every End answered: E stops serving, and A sees it leave.
    tokio::time::timeout(WAIT, serving)
        .await
        .expect("a drained environment stops serving")
        .unwrap();
    a.until_connected(e.peer, false).await;
    let mut rest = Vec::new();
    tokio::time::timeout(WAIT, harness.output.read_to_end(&mut rest))
        .await
        .expect("the harness's output ends")
        .expect("and reads to its end");
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
        ["granted", "started", "draining", "stopped", "ended"],
        "{entries:?}"
    );
    assert_eq!(entries[4]["cause"], "drained");
    assert_eq!(entries[4]["cleanup"]["outcome"], "confirmed");
}

/// Whether a key still serves is asked again this often in the revocation
/// tests, in place of the keepalive interval.
const RECHECK: Duration = Duration::from_millis(100);

impl Gateway {
    /// The owner revokes `machine`'s credential.
    fn revoke(&self, machine: &Machine) {
        self.auth
            .revoke_sync(RevokeCredentialRequest {
                request_id: uuid(),
                issuer_principal_id: self.owner.clone(),
                credential_id: machine.credential.clone(),
                revoked_at: SystemClock.unix_seconds(),
            })
            .unwrap();
    }

    /// Every record of `kind` in the gateway's environment audit under
    /// `root`.
    fn environment_records(root: &Path, kind: &str) -> Vec<serde_json::Value> {
        Self::environment_audit(root)
            .into_iter()
            .filter(|record| record["kind"] == kind)
            .collect()
    }

    /// Every record in the gateway's environment audit under `root`.
    fn environment_audit(root: &Path) -> Vec<serde_json::Value> {
        let mut records = Vec::new();
        let mut directories = vec![root.join("environments")];
        while let Some(directory) = directories.pop() {
            let Ok(entries) = std::fs::read_dir(&directory) else {
                continue;
            };
            for entry in entries {
                let path = entry.unwrap().path();
                if path.is_dir() {
                    directories.push(path);
                    continue;
                }
                let Ok(text) = std::fs::read_to_string(&path) else {
                    continue;
                };
                if let Ok(record) = serde_json::from_str::<serde_json::Value>(&text) {
                    records.push(record);
                }
            }
        }
        records
    }
}

/// Row I8: an idle paired environment whose credential is revoked has its
/// connection closed once the gateway asks again, with no lease asked of
/// it: it leaves the connected list at once, its serving ends, and its next
/// dial is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn an_idle_paired_environment_is_closed_once_revoked() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("a");
    let a = Gateway::start_with(
        &root,
        SshTimings {
            keepalive: RECHECK,
            ..SshTimings::default()
        },
    )
    .await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let served = serving(&directory.path().join("e"), e.pin, stream);
    a.until_connected(e.peer, true).await;
    a.revoke(&e);
    tokio::time::timeout(WAIT, served)
        .await
        .expect("the revoked environment's serving ends with no lease asked")
        .unwrap();
    a.until_connected(e.peer, false).await;
    let records = Gateway::environment_records(&root, "revoked");
    assert_eq!(records.len(), 1, "{records:?}");
    let refused = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .err()
    .expect("a revoked environment is refused");
    assert_eq!(refused.kind(), ErrorKind::PermissionDenied);
}

/// Row I8 end to end: E runs a harness for A under a lease; A's owner
/// revokes E's credential. Without any new lease asked, A sees it at its
/// next check, ends the lease as revoked, tells E, and E stops the harness
/// and records the lease revoking and ended as revoked; A's End is answered
/// with E's cleanup, A closes the connection, and E's next dial is refused.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_environment_revoked_mid_lease_ends_it_on_both_sides_and_closes() {
    let directory = tempfile::tempdir().unwrap();
    let root = directory.path().join("a");
    let a = Gateway::start_with(
        &root,
        SshTimings {
            keepalive: RECHECK,
            ..SshTimings::default()
        },
    )
    .await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
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
    host.grants
        .change(&host.peer, |_| {
            Ok::<_, ()>(Some(PeerGrant::nothing().with_agents(Agents::All)))
        })
        .unwrap();
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    let served = {
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
        captured_host: None,
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
    let harness_host = given.lock().unwrap().clone().unwrap();
    let mut harness = harness_host.start(HarnessLaunch::default()).unwrap();
    let entries = || -> Vec<serde_json::Value> {
        std::fs::read_to_string(&host.ledger_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .filter(|entry: &serde_json::Value| entry["lease"] == lease.as_str())
            .collect()
    };
    tokio::time::timeout(WAIT, async {
        while !entries().iter().any(|entry| entry["kind"] == "started") {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the harness starts");
    let ends = opened
        .hold
        .ends()
        .expect("a paired environment's lease can end there");

    a.revoke(&e);
    assert_eq!(
        tokio::time::timeout(WAIT, ends)
            .await
            .expect("A sees the revocation with no lease asked"),
        LeaseEndCause::Revoked
    );
    // It leaves the connected list at once, though its connection still
    // carries the lease's end.
    assert!(!a.peers.connected().contains(&e.peer));
    let released = tokio::time::timeout(WAIT, opened.hold.end(LeaseEndCause::Revoked))
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
    tokio::time::timeout(WAIT, harness.output.read_to_end(&mut rest))
        .await
        .expect("the harness's output ends")
        .expect("and reads to its end");
    drop(opened);
    // A closes the connection once the lease is let go; E's serving ends.
    tokio::time::timeout(WAIT, served)
        .await
        .expect("the revoked environment's serving ends")
        .unwrap();
    let entries = entries();
    let kinds: Vec<_> = entries
        .iter()
        .map(|entry| entry["kind"].as_str().unwrap().to_owned())
        .collect();
    assert_eq!(
        kinds,
        ["granted", "started", "revoking", "stopped", "ended"],
        "{entries:?}"
    );
    // The revocation names the gateway the lease was granted to.
    assert_eq!(entries[2]["peer"], entries[0]["peer"]);
    assert!(entries[2]["peer"].is_string(), "{entries:?}");
    assert_eq!(entries[4]["cause"], "revoked");
    assert_eq!(entries[4]["cleanup"]["outcome"], "confirmed");
    let revoked = Gateway::environment_records(&root, "revoked");
    assert_eq!(revoked.len(), 1, "{revoked:?}");
    assert!(
        revoked[0].to_string().contains(lease.as_str()),
        "{revoked:?}"
    );
    let refused = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .err()
    .expect("a revoked environment is refused");
    assert_eq!(refused.kind(), ErrorKind::PermissionDenied);
}

/// Every bytes of `bytes`, pulled in order until the end.
async fn read_all(bytes: &mut Box<dyn crate::conversation::application::ArtifactBytes>) -> Vec<u8> {
    let mut read = Vec::new();
    while let Some(chunk) = tokio::time::timeout(WAIT, bytes.next())
        .await
        .expect("each chunk in time")
        .expect("the file reads to its end")
    {
        read.extend_from_slice(&chunk);
    }
    read
}

fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::{Digest, Sha256};
    Sha256::digest(bytes)
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect()
}

/// Row I9 end to end: E runs a harness for A under a lease over the
/// connection E dialed. The harness publishes a file on its lease's
/// publish point, and A asks E to publish another for the agent; A reads
/// each over that same connection's lease frames, finds the bytes are
/// what E published, and answers held. The harness is told the
/// conversation holds its file, and E records each publish and A's answer.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_paired_environment_publishes_a_file_and_the_gateway_reads_it_back() {
    use crate::conversation::application::{ArtifactOffer, PublishAsk};
    use crate::env_serve::application::{PublishAnswer, PublishRequest};
    use nessa_protocol::lease::Collection;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt};

    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    // E runs `claude` as a shell that notes its publish point where the
    // test can find it, then runs as `cat` until it is stopped.
    let harness = LaunchSpec {
        executable: "/bin/sh".into(),
        args: vec![
            "-c".into(),
            r#"printf %s "$NESSA_ARTIFACTS" > point.part && mv point.part point && exec cat"#
                .into(),
        ],
        environment: Default::default(),
    };
    let host = Arc::new(Host::running(
        &directory.path().join("e"),
        e.pin,
        HashMap::from([(nessa_protocol::agents::AgentId::Claude, harness)]),
    ));
    host.grants
        .change(&host.peer, |_| {
            Ok::<_, ()>(Some(PeerGrant::nothing().with_agents(Agents::All)))
        })
        .unwrap();
    let workspace = host.ledger_path.parent().unwrap().to_path_buf();
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
        captured_host: None,
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
    let mut offers = opened
        .hold
        .artifacts()
        .expect("a paired environment's lease publishes");
    let harness_host = given.lock().unwrap().clone().unwrap();
    let _harness = harness_host.start(HarnessLaunch::default()).unwrap();
    let point_file = workspace.join("point");
    let point = tokio::time::timeout(WAIT, async {
        loop {
            match std::fs::read_to_string(&point_file) {
                Ok(point) if !point.is_empty() => break point,
                _ => tokio::time::sleep(Duration::from_millis(10)).await,
            }
        }
    })
    .await
    .expect("the harness is told its publish point");

    // The harness publishes a file larger than many ranges, as
    // `nessa artifact publish` would.
    let image: Vec<u8> = (0..300_017u32).map(|index| (index % 251) as u8).collect();
    let image_path = workspace.join("Nessa.dmg");
    std::fs::write(&image_path, &image).unwrap();
    let publisher = tokio::spawn(async move {
        let mut socket = tokio::net::UnixStream::connect(point).await.unwrap();
        let mut line = serde_json::to_vec(&PublishRequest {
            path: image_path.to_str().unwrap().to_owned(),
            media_type: None,
        })
        .unwrap();
        line.push(b'\n');
        socket.write_all(&line).await.unwrap();
        let mut answer = String::new();
        tokio::io::BufReader::new(socket)
            .read_line(&mut answer)
            .await
            .unwrap();
        serde_json::from_str::<PublishAnswer>(&answer).unwrap()
    });
    let ArtifactOffer {
        file,
        mut bytes,
        answer,
        request,
    } = tokio::time::timeout(WAIT, offers.recv())
        .await
        .expect("offered in time")
        .expect("an offer");
    assert_eq!(request, None);
    assert_eq!(file.name, "Nessa.dmg");
    assert_eq!(file.size, image.len() as u64);
    let read = read_all(&mut bytes).await;
    assert_eq!(read, image);
    assert_eq!(sha256_hex(&read), file.digest);
    answer.answer(Collection::Held).await;
    assert_eq!(
        tokio::time::timeout(WAIT, publisher)
            .await
            .expect("the harness is answered")
            .unwrap(),
        PublishAnswer::Held {
            digest: sha256_hex(&image),
            size: image.len() as u64,
        }
    );

    // The gateway asks for one on the agent's behalf.
    let report = b"a report the agent asked to keep".to_vec();
    std::fs::write(workspace.join("report.txt"), &report).unwrap();
    let publish = opened
        .hold
        .publish(PublishAsk {
            request: "call-1".into(),
            path: "report.txt".into(),
            media_type: None,
        })
        .expect("a paired environment's lease publishes");
    assert_eq!(tokio::time::timeout(WAIT, publish).await.unwrap(), Ok(()));
    let ArtifactOffer {
        file,
        mut bytes,
        answer,
        request,
    } = tokio::time::timeout(WAIT, offers.recv())
        .await
        .expect("offered in time")
        .expect("an offer");
    assert_eq!(request.as_deref(), Some("call-1"));
    assert_eq!(read_all(&mut bytes).await, report);
    assert_eq!(sha256_hex(&report), file.digest);
    answer.answer(Collection::Held).await;

    let released = tokio::time::timeout(WAIT, opened.hold.end(LeaseEndCause::Stopped))
        .await
        .unwrap();
    assert!(
        matches!(
            released,
            LeaseRelease::Released(LeaseCleanup::Confirmed { .. })
        ),
        "{released:?}"
    );
    // E recorded each publish and A's answer to it, before the lease ended.
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
        [
            "granted",
            "started",
            "published",
            "collected",
            "publish_asked",
            "published",
            "collected",
            "stopped",
            "ended",
        ],
        "{entries:?}"
    );
    assert_eq!(entries[2]["digest"], sha256_hex(&image));
    assert_eq!(entries[3]["outcome"]["outcome"], "held");
    assert_eq!(entries[5]["request"], "call-1");
    assert_eq!(entries[6]["outcome"]["outcome"], "held");
    // Nothing is left staged on E.
    let staged = std::fs::read_dir(workspace.join("outbox"))
        .map(|entries| entries.count())
        .unwrap_or(0);
    assert_eq!(staged, 0);
    // A recorded every frame E sent it to a lease it held: none dropped.
    let a_root = directory.path().join("a");
    assert!(!Gateway::environment_records(&a_root, "connected").is_empty());
    let dropped = Gateway::environment_records(&a_root, "frame_dropped");
    assert!(dropped.is_empty(), "{dropped:?}");
}

/// Row I10: the paired environments' count of connection changes moves
/// where each change is made: once when E's connection is admitted, once
/// when it ends, and, revoked, once when it is revoked and once more when
/// the revoked connection ends. Each wake finds the connected list already
/// changed, so whoever reads it on the wake (the agent catalog's
/// subscriptions) reads the list as it is after the change.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn connection_changes_are_said_as_a_paired_environment_connects_ends_and_is_revoked() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start_with(
        &directory.path().join("a"),
        SshTimings {
            keepalive: RECHECK,
            ..SshTimings::default()
        },
    )
    .await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let mut changes = a.peers.connections();
    let start = *changes.borrow();
    // Woken until the count reaches `count`, the list then saying
    // `connected`.
    async fn until(
        peers: &PeerEnvironments,
        changes: &mut watch::Receiver<u64>,
        peer: PeerEnvironment,
        count: u64,
        connected: bool,
    ) {
        tokio::time::timeout(WAIT, async {
            while *changes.borrow_and_update() != count {
                changes.changed().await.expect("the source stays open");
            }
        })
        .await
        .unwrap_or_else(|_| panic!("the count never reached {count}"));
        assert_eq!(peers.connected().contains(&peer), connected, "{count}");
    }

    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let served = serving(&directory.path().join("e"), e.pin, stream);
    until(&a.peers, &mut changes, e.peer, start + 1, true).await;

    // E goes: its connection ends from its side.
    served.abort();
    until(&a.peers, &mut changes, e.peer, start + 2, false).await;

    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let served = serving(&directory.path().join("e2"), e.pin, stream);
    until(&a.peers, &mut changes, e.peer, start + 3, true).await;

    a.revoke(&e);
    until(&a.peers, &mut changes, e.peer, start + 5, false).await;
    tokio::time::timeout(WAIT, served)
        .await
        .expect("the revoked environment's serving ends")
        .unwrap();
}

struct BuildBox;
impl crate::devices::application::MachineProbe for BuildBox {
    fn read(&self) -> Option<nessa_protocol::lease::MachineReport> {
        Some(nessa_protocol::lease::MachineReport {
            hostname: "build-box".into(),
            os: "linux".into(),
            os_version: "24.04".into(),
            arch: "x86_64".into(),
            cpus: 8,
            cpu_permille: 610,
            memory_total_bytes: 32,
            memory_used_bytes: 8,
            disk_total_bytes: 100,
            disk_used_bytes: 40,
            disks: vec![nessa_protocol::lease::MachineDisk {
                mount: "/".into(),
                total_bytes: 100,
                used_bytes: 40,
            }],
        })
    }
}

/// A paired environment reports its machine over its lease connection: the
/// gateway lists it serving with that report, and once the connection ends
/// it is told the last report, to keep.
#[tokio::test]
async fn a_paired_environment_reports_its_machine_and_its_departure_keeps_it() {
    use crate::devices::application::MachineProbe;
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let mut host = Host::open(&directory.path().join("e"), e.pin);
    host.machine = Some(Arc::new(BuildBox));
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    let served = tokio::spawn(async move { host.serve(from_gateway, to_gateway).await });
    let reported = tokio::time::timeout(WAIT, async {
        loop {
            let workers = a.peers.workers();
            if let Some(worker) = workers.iter().find(|worker| worker.peer == e.peer) {
                if worker.machine.is_some() {
                    return (worker.load, worker.machine.clone());
                }
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the machine is reported");
    assert_eq!(reported.0.map(|load| load.leases), Some(0));
    assert_eq!(reported.1, BuildBox.read());
    assert!(a.departed.left.lock().unwrap().is_empty());
    // The report is told as it arrives, not only once the connection ends.
    tokio::time::timeout(WAIT, async {
        while a.departed.reported.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the report is told");
    assert_eq!(
        a.departed.reported.lock().unwrap()[0],
        (e.peer, BuildBox.read().unwrap())
    );

    served.abort();
    tokio::time::timeout(WAIT, async {
        while a.departed.left.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the departure is told");
    assert_eq!(
        *a.departed.left.lock().unwrap(),
        vec![(e.peer, BuildBox.read().unwrap())]
    );
}

/// A revoked worker is forgotten, not seen leaving: its connection's end
/// is not told, so no sighting saved as of then outlives the forgetting.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn a_revoked_paired_environment_is_not_told_as_departing() {
    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start_with(
        &directory.path().join("a"),
        SshTimings {
            keepalive: RECHECK,
            ..SshTimings::default()
        },
    )
    .await;
    let e = a
        .pair(
            &directory.path().join("e-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let stream = dial(
        a.native.to_string(),
        None,
        None,
        e.identity.clone(),
        e.pin,
        clock(),
    )
    .await
    .unwrap();
    let mut host = Host::open(&directory.path().join("e"), e.pin);
    host.machine = Some(Arc::new(BuildBox));
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    let served = tokio::spawn(async move { host.serve(from_gateway, to_gateway).await });
    tokio::time::timeout(WAIT, async {
        while a.departed.reported.lock().unwrap().is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("the report is told");
    a.revoke(&e);
    let _ = tokio::time::timeout(WAIT, served)
        .await
        .expect("the revoked environment's serving ends");
    a.until_connected(e.peer, false).await;
    // Room for a departure, were one told.
    tokio::time::sleep(RECHECK).await;
    assert!(a.departed.left.lock().unwrap().is_empty());
}

/// A code file as a container's secret holds it: the display form and a
/// line end.
fn code_file(directory: &Path, name: &str, code: &ManualCode) -> std::path::PathBuf {
    let path = directory.join(name);
    let mut bytes = code.display_bytes().to_vec();
    bytes.push(b'\n');
    std::fs::write(&path, bytes).unwrap();
    path
}

/// Row K2: `env dial --enroll-code-file` enrolls a machine paired with
/// nothing, and on every later start, approved or not yet, finds it paired
/// with that gateway and sends nothing, so a container's entrypoint is the
/// same command each time.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn k2_a_code_file_enrolls_once_then_dials() {
    use crate::env_serve::application::{approval, enroll_once, EnrolledOnce};
    use crate::env_serve::infrastructure::{FileEnrollmentCode, FilePairingEnvironment};

    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let created = a
        .commands
        .create(&a.session, ConsentClass::PeerEnvironment)
        .await
        .unwrap();
    let code = FileEnrollmentCode(code_file(directory.path(), "code", created.code()));
    let environment = directory.path().join("e-environment");
    nessa_local_storage::create_directory(&environment).unwrap();
    let address = a.native.to_string();
    let enroll_once = || async {
        let pairing = FilePairingEnvironment::open(&environment, &address, clock()).unwrap();
        let enrolled = tokio::time::timeout(WAIT, enroll_once(&pairing, &code))
            .await
            .unwrap();
        pairing.shutdown().await;
        enrolled
    };

    assert_eq!(enroll_once().await, Ok(EnrolledOnce::Now));
    // Restarted before the owner approved: still paired with A, nothing sent.
    assert_eq!(enroll_once().await, Ok(EnrolledOnce::Before));
    let id = created.record().id();
    let claimed = a.commands.status(&a.session, id).await.unwrap();
    let (_, key) = claimed.claim_binding().unwrap();
    a.commands.approve(&a.session, id, key).await.unwrap();
    let pairing = FilePairingEnvironment::open(&environment, &address, clock()).unwrap();
    let (_, running) = watch::channel(false);
    let (_, pin, _) = tokio::time::timeout(
        WAIT,
        approval(
            &pairing,
            Duration::from_millis(20),
            &TokioTimer::new(),
            running,
        ),
    )
    .await
    .unwrap()
    .unwrap()
    .expect("not stopped");
    pairing.shutdown().await;
    drop(pairing);
    // Restarted once approved: the same, and its pin is A's.
    assert_eq!(enroll_once().await, Ok(EnrolledOnce::Before));
    let saved = FilePairingEnvironment::approved_gateway(&environment, clock())
        .await
        .unwrap();
    assert_eq!(saved, Some(pin));
}

/// Row K2: a volume paired with gateway A, given gateway B's address and
/// code, refuses to dial B instead of taking B's code as its pairing's, and
/// sends B nothing: B's invitation is still unclaimed. A pairing that
/// recorded no gateway refuses too, and a file holding no code says so.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn k2_a_volume_paired_to_one_gateway_refuses_enrolling_with_another() {
    use crate::env_serve::application::{enroll_once, NotEnrolled};
    use crate::env_serve::infrastructure::{FileEnrollmentCode, FilePairingEnvironment};

    let directory = tempfile::tempdir().unwrap();
    let a = Gateway::start(&directory.path().join("a")).await;
    let b = Gateway::start(&directory.path().join("b")).await;
    let environment = directory.path().join("e-environment");
    nessa_local_storage::create_directory(&environment).unwrap();

    let not_a_code = FileEnrollmentCode(directory.path().join("not-a-code"));
    std::fs::write(&not_a_code.0, b"hello\n").unwrap();
    let pairing =
        FilePairingEnvironment::open(&environment, &a.native.to_string(), clock()).unwrap();
    let refused = enroll_once(&pairing, &not_a_code).await.unwrap_err();
    assert_eq!(
        refused,
        NotEnrolled::Failed(format!("{} holds no pairing code", not_a_code.0.display()))
    );
    let for_a = a
        .commands
        .create(&a.session, ConsentClass::PeerEnvironment)
        .await
        .unwrap();
    // A code the gateway never gave: refused, and the address it recorded
    // first is left over a pairing that does not exist, read by nothing.
    let wrong = FileEnrollmentCode(directory.path().join("wrong"));
    std::fs::write(&wrong.0, b"ABCD-EFGH\n").unwrap();
    let refused = tokio::time::timeout(WAIT, enroll_once(&pairing, &wrong))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(
        refused,
        NotEnrolled::Failed("the code is not the one the gateway gave".into())
    );
    let code_a = FileEnrollmentCode(code_file(directory.path(), "code-a", for_a.code()));
    tokio::time::timeout(WAIT, enroll_once(&pairing, &code_a))
        .await
        .unwrap()
        .unwrap();
    pairing.shutdown().await;
    drop(pairing);

    let for_b = b
        .commands
        .create(&b.session, ConsentClass::PeerEnvironment)
        .await
        .unwrap();
    let code_b = FileEnrollmentCode(code_file(directory.path(), "code-b", for_b.code()));
    let pairing =
        FilePairingEnvironment::open(&environment, &b.native.to_string(), clock()).unwrap();
    let refused = enroll_once(&pairing, &code_b).await.unwrap_err();
    assert_eq!(
        refused,
        NotEnrolled::PairedElsewhere(Some(a.native.to_string()))
    );
    pairing.shutdown().await;
    drop(pairing);
    let unclaimed = b
        .commands
        .status(&b.session, for_b.record().id())
        .await
        .unwrap();
    assert!(unclaimed.claim_binding().is_none());

    // Paired, with no record of which gateway: refused, not guessed.
    std::fs::remove_file(environment.join("paired-with")).unwrap();
    let pairing =
        FilePairingEnvironment::open(&environment, &a.native.to_string(), clock()).unwrap();
    let refused = enroll_once(&pairing, &code_a).await.unwrap_err();
    assert_eq!(refused, NotEnrolled::PairedElsewhere(None));
    pairing.shutdown().await;
}

/// Row K3, the hosted-worker setup with two organizations, end to end:
/// gateways A and B are one organization each, and workers W and X are
/// paired one to each, serving from workspaces of their own. Each gateway
/// leases, runs commands, and reads published files only on its own
/// worker; the other's key is refused at A's door, its name is
/// unreachable from each gateway, and a path out of a worker's workspace
/// is refused. Each worker's ledger holds its own gateway's leases alone,
/// and each gateway's environment audit names its own worker alone.
#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn k3_two_organizations_share_no_lease_workspace_artifact_tool_or_audit() {
    use crate::conversation::application::{
        ArtifactOffer, EnvironmentLease, PublishAsk, PublishFailure,
    };
    use nessa_protocol::lease::{Collection, PublishRefusal};

    let directory = tempfile::tempdir().unwrap();
    let (a_root, b_root) = (directory.path().join("a"), directory.path().join("b"));
    // Two gateways, each started as its own organization.
    let a = Gateway::start(&a_root).await;
    let b = Gateway::start(&b_root).await;
    let w = a
        .pair(
            &directory.path().join("w-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;
    let x = b
        .pair(
            &directory.path().join("x-state"),
            ConsentClass::PeerEnvironment,
        )
        .await;

    // Leases: X's key is not paired with A. With A's pin it is refused at
    // A's door; with its own pin, the key it finds is not B's, and the dial
    // is cut there.
    let refused = dial(
        a.native.to_string(),
        None,
        None,
        x.identity.clone(),
        w.pin,
        clock(),
    )
    .await
    .err()
    .expect("a key paired with B is refused by A");
    assert_eq!(refused.kind(), ErrorKind::PermissionDenied);
    let not_b = dial(
        a.native.to_string(),
        None,
        None,
        x.identity.clone(),
        x.pin,
        clock(),
    )
    .await
    .err()
    .expect("A is not the gateway X's pin names");
    assert_eq!(not_b.kind(), ErrorKind::ConnectionAborted);

    // Each worker runs `claude` as `cat`, which runs until it is stopped;
    // each owner grants its own gateway agents and commands.
    let cat = LaunchSpec {
        executable: "/bin/cat".into(),
        args: Vec::new(),
        environment: Default::default(),
    };
    let worker = |name: &str, pin: [u8; 44]| {
        let host = Arc::new(Host::running(
            &directory.path().join(name),
            pin,
            HashMap::from([(nessa_protocol::agents::AgentId::Claude, cat.clone())]),
        ));
        let commands = CommandGrant::new(CommandWork::MAX_TIMEOUT_MS).unwrap();
        host.grants
            .change(&host.peer, |_| {
                Ok::<_, ()>(Some(
                    PeerGrant::nothing()
                        .with_agents(Agents::All)
                        .with_commands(commands),
                ))
            })
            .unwrap();
        host
    };
    let (w_host, x_host) = (worker("w", w.pin), worker("x", x.pin));
    let mut serving = Vec::new();
    for (gateway, machine, host) in [(&a, &w, &w_host), (&b, &x, &x_host)] {
        let stream = dial(
            gateway.native.to_string(),
            None,
            None,
            machine.identity.clone(),
            machine.pin,
            clock(),
        )
        .await
        .unwrap();
        let (from_gateway, to_gateway) = tokio::io::split(stream);
        let host = host.clone();
        serving.push(tokio::spawn(async move {
            host.serve(from_gateway, to_gateway).await
        }));
        gateway.until_connected(machine.peer, true).await;
    }
    // Each gateway's paired environments hold its own worker alone; a
    // command on the other's name finds no connection.
    assert_eq!(a.peers.connected(), [w.peer]);
    assert_eq!(b.peers.connected(), [x.peer]);
    assert_eq!(a.echo(w.peer, "a").await.unwrap().stdout, b"a");
    assert_eq!(b.echo(x.peer, "b").await.unwrap().stdout, b"b");
    assert_eq!(
        a.echo(x.peer, "across").await.unwrap_err(),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable)
    );
    assert_eq!(
        b.echo(w.peer, "across").await.unwrap_err(),
        CommandRefusal::Environment(LeaseRefusal::EnvironmentUnreachable)
    );

    // An agent lease `gateway` asks of `machine`, with the harness host it
    // is given.
    type Given = Arc<std::sync::Mutex<Option<Arc<dyn HarnessHost>>>>;
    async fn agent_lease(
        gateway: &Gateway,
        machine: &Machine,
    ) -> (LeaseId, Result<EnvironmentLease, LeaseRefusal>, Given) {
        let environment = gateway.peers.environment(&machine.peer);
        let lease = LeaseId::new(uuid()).unwrap();
        let terms = LeaseTerms {
            environment: environment.declaration().environment,
            work: LeaseWork::Agent(AgentWork::new("claude", "model").unwrap()),
            sandbox: SandboxProfile::HarnessDefault,
            grants: LeaseGrants::Opening,
            deadline: LeaseDeadline::UntilEnded,
        };
        let given: Given = Arc::new(std::sync::Mutex::new(None));
        let binding = Arc::new(OnHost {
            captured_host: None,
            capabilities: crate::conversation_test_support::Provider::new(Arc::new(
                crate::conversation_test_support::ProviderFactory::default(),
            ))
            .capabilities()
            .clone(),
            host: given.clone(),
        });
        let opened = tokio::time::timeout(WAIT, environment.open(&lease, &terms, binding))
            .await
            .unwrap();
        (lease, opened, given)
    }
    // Nor does an agent lease on the other's name reach it.
    for (gateway, theirs) in [(&a, &x), (&b, &w)] {
        let (_, refused, _) = agent_lease(gateway, theirs).await;
        assert_eq!(
            refused
                .err()
                .expect("no lease on the other organization's worker"),
            LeaseRefusal::EnvironmentUnreachable
        );
    }

    // Workspaces and artifacts: the same name in each workspace holds that
    // organization's file, and each gateway's lease reads its own.
    for (name, text) in [("w", "for A"), ("x", "for B")] {
        std::fs::write(directory.path().join(name).join("report.txt"), text).unwrap();
    }
    let mut leases = Vec::new();
    for (gateway, machine, text) in [(&a, &w, "for A"), (&b, &x, "for B")] {
        let (lease, opened, given) = agent_lease(gateway, machine).await;
        let opened = opened.unwrap_or_else(|refusal| panic!("granted: {refusal:?}"));
        let mut offers = opened
            .hold
            .artifacts()
            .expect("a paired environment's lease publishes");
        let harness_host = given.lock().unwrap().clone().unwrap();
        let harness = harness_host.start(HarnessLaunch::default()).unwrap();
        let publish = opened
            .hold
            .publish(PublishAsk {
                request: "report".into(),
                path: "report.txt".into(),
                media_type: None,
            })
            .expect("a paired environment's lease publishes");
        assert_eq!(tokio::time::timeout(WAIT, publish).await.unwrap(), Ok(()));
        let ArtifactOffer {
            mut bytes, answer, ..
        } = tokio::time::timeout(WAIT, offers.recv())
            .await
            .expect("offered in time")
            .expect("an offer");
        assert_eq!(read_all(&mut bytes).await, text.as_bytes());
        answer.answer(Collection::Held).await;
        leases.push((lease, opened, harness));
    }
    // A path out of W's workspace, into X's, where a file is, is refused
    // before anything is read.
    let across = leases[0]
        .1
        .hold
        .publish(PublishAsk {
            request: "across".into(),
            path: "../x/report.txt".into(),
            media_type: None,
        })
        .expect("a paired environment's lease publishes");
    assert_eq!(
        tokio::time::timeout(WAIT, across).await.unwrap(),
        Err(PublishFailure::NotPublished(
            PublishRefusal::OutsideWorkspace
        ))
    );
    for (_, opened, _) in &leases {
        let released = tokio::time::timeout(WAIT, opened.hold.end(LeaseEndCause::Stopped))
            .await
            .unwrap();
        assert!(
            matches!(
                released,
                LeaseRelease::Released(LeaseCleanup::Confirmed { .. })
            ),
            "{released:?}"
        );
    }

    // Audit: each worker's ledger records the command its own gateway ran,
    // and not the other's.
    for (host, mine, theirs) in [(&w_host, "a", "b"), (&x_host, "b", "a")] {
        let commands: Vec<serde_json::Value> = std::fs::read_to_string(&host.ledger_path)
            .unwrap()
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|entry| entry["kind"] == "command_granted")
            .map(|entry| entry["argv"].clone())
            .collect();
        assert!(
            commands.contains(&serde_json::json!(["echo", mine])),
            "{commands:?}"
        );
        assert!(
            !commands.contains(&serde_json::json!(["echo", theirs])),
            "{commands:?}"
        );
    }
    // And the agent leases its own gateway asked, granted and ended, and
    // none of the other's.
    for (host, mine, theirs) in [
        (&w_host, &leases[0].0, &leases[1].0),
        (&x_host, &leases[1].0, &leases[0].0),
    ] {
        let ledger = std::fs::read_to_string(&host.ledger_path).unwrap();
        let kinds: Vec<String> = ledger
            .lines()
            .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
            .filter(|entry| entry["lease"] == mine.as_str())
            .map(|entry| entry["kind"].as_str().unwrap().to_owned())
            .collect();
        assert_eq!(kinds.first().map(String::as_str), Some("granted"));
        assert_eq!(kinds.last().map(String::as_str), Some("ended"));
        assert!(!ledger.contains(theirs.as_str()));
    }
    // Each gateway's environment audit, which records its workers'
    // connections, names its own worker at that worker's workspace, and
    // nothing of the other's.
    for (root, mine, theirs, workspace) in [(&a_root, &w, &x, "w"), (&b_root, &x, &w, "x")] {
        let records = Gateway::environment_audit(root);
        assert!(!records.is_empty());
        for record in &records {
            assert_eq!(record["host"], mine.peer.name(), "{record}");
        }
        let connected = Gateway::environment_records(root, "connected");
        assert_eq!(connected.len(), 1, "{connected:?}");
        assert_eq!(
            connected[0]["workspace"],
            directory
                .path()
                .join(workspace)
                .canonicalize()
                .unwrap()
                .to_str()
                .unwrap()
        );
        assert!(!format!("{records:?}").contains(&theirs.peer.name()));
    }
    // Nothing is left serving past the test.
    for task in serving {
        task.abort();
    }
}
