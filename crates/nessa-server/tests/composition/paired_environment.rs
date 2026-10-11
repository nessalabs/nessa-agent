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
use crate::conversation::application::{CommandResult, PairedEnvironments};
use crate::conversation::infrastructure::ssh_environment::{
    DurableEnvironmentAudit, PeerEnvironments, SshTimings,
};
use crate::conversation::infrastructure::LocalReceiverAuthority;
use crate::device_pairing::infrastructure::PairingOwnerCommands;
use crate::env::{LEASE_PROTOCOL, VERSION};
use crate::env_serve::application::{
    serve, CommandRan, CommandRunner, CommandStop, Runners, ServeTimings,
};
use crate::env_serve::infrastructure::{dial, ConfiguredLauncher, FileLedger, FileOutbox};
use crate::product::{ProductDependencies, ProductRouteState};
use nessa_auth::adapters::cedar::CedarPolicyEvaluator;
use nessa_auth::adapters::local::LocalCredentialStore;
use nessa_auth::adapters::pairing::{FilePairingState, ManualCode, NativeIdentity, OsEntropy};
use nessa_auth::application::credential_admin::RevokeCredentialRequest;
use nessa_auth::application::pairing::ClientPendingStore;
use nessa_auth::application::ports::Clock;
use nessa_auth::application::session::AuthenticatedSession;
use nessa_auth::domain::pairing::ConsentClass;
use nessa_auth::domain::{AudienceId, OrganizationId, Resource, ResourceId};
use nessa_client_core::pairing::NativeEnrollmentClient;
use nessa_protocol::lease::{Cleanup, CommandEnd};
use nessa_protocol::pairing::wire::NativePairingStatus;
use nessa_sdk::domain::agent_execution::leases::{
    CommandExit, CommandRefusal, CommandWork, LeaseCleanup, LeaseId, LeaseRefusal, PeerEnvironment,
};
use std::collections::HashMap;
use std::future::Future;
use std::io::ErrorKind;
use std::net::SocketAddr;
use std::path::Path;
use std::pin::Pin;
use std::sync::Arc;
use std::time::Duration;
use tokio::sync::watch;
use tokio::task::JoinHandle;

/// Gateway A: native pairing with its paired environments composed.
struct Gateway {
    auth: Arc<LocalCredentialStore>,
    session: AuthenticatedSession,
    owner: String,
    commands: PairingOwnerCommands,
    peers: Arc<PeerEnvironments>,
    native: SocketAddr,
    running: Option<RunningNative>,
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
        let peers = Arc::new(PeerEnvironments::new(
            Arc::new(RegistryServing {
                store: auth.clone(),
                policy: Arc::new(CedarPolicyEvaluator::new().unwrap()),
                clock: Arc::new(SystemClock),
                gateway: resource,
                audience: AudienceId::new(gateway.clone()).unwrap(),
            }),
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
            .unwrap()?;
        let (_stop, stopped) = watch::channel(None);
        Ok(tokio::time::timeout(WAIT, hold.run(stopped)).await.unwrap())
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
/// dialed, with a command runner that echoes.
fn serving(
    root: &Path,
    stream: crate::device_pairing::infrastructure::TlsStream,
) -> JoinHandle<()> {
    nessa_local_storage::create_directory(root).unwrap();
    let workspace = root.canonicalize().unwrap();
    let ledger = Arc::new(FileLedger::open(workspace.join("leases.jsonl")).unwrap());
    let outbox = Arc::new(
        FileOutbox::open(
            workspace.join("outbox"),
            workspace.join("sockets"),
            &workspace,
        )
        .unwrap(),
    );
    let launcher = Arc::new(ConfiguredLauncher::new(
        workspace.to_string_lossy().into_owned(),
        HashMap::new(),
    ));
    let (from_gateway, to_gateway) = tokio::io::split(stream);
    tokio::spawn(serve(
        from_gateway,
        to_gateway,
        VERSION,
        LEASE_PROTOCOL,
        Runners {
            harnesses: launcher,
            commands: Some(Arc::new(Echo)),
        },
        ledger,
        outbox,
        ServeTimings::default(),
    ))
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
    let first = serving(&directory.path().join("e"), first);
    a.until_connected(e.peer, true).await;
    let ran = a.echo(e.peer, "hello").await.unwrap();
    assert_eq!(ran.exit, CommandExit::Exited { code: 0 });
    assert_eq!(ran.stdout, b"hello");
    assert_eq!(ran.cleanup, Some(LeaseCleanup::Confirmed { forced: false }));

    // A second connection replaces the first, whose serving ends.
    let second = dial(a.native, e.identity.clone(), e.pin, clock())
        .await
        .unwrap();
    let second = serving(&directory.path().join("e2"), second);
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
