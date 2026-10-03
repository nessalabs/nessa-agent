//! The composed `nessa server` process with native pairing configured: an
//! owner creates and approves through the product socket while a device enrolls
//! through the native listener. Design rows S1, S3, S6, S12 and O1 in
//! `docs/design/auth/device-pairing.md` ("Owner routes and mounting").
//!
//! Unix only: the process is stopped with SIGTERM, as launchd stops it.
use super::product_client::ProductClient;
use super::support::{pending, WAIT};
use nessa_auth::{
    adapters::{
        local::{BootstrapRequest, LocalCredentialStore},
        pairing::{ManualCode, OsEntropy},
    },
    application::{
        dto::{
            CredentialGrantDto, MembershipInputDto, MembershipRoleDto, MembershipStateDto,
            OrganizationInputDto, PrincipalInputDto, PrincipalKindDto, ResourceDto,
        },
        ports::{AccessReader, CredentialEvidence, CredentialVerifier},
    },
    domain::AudienceId,
};
use nessa_server::{
    app::dependencies::RuntimeDependencies,
    device_pairing::infrastructure::{wire::NativePairingStatus, NativeEnrollmentClient},
};
use serde_json::{json, Value};
use std::{
    io::Write,
    net::{SocketAddr, TcpListener, TcpStream},
    path::{Path, PathBuf},
    process::{Child, Command, ExitStatus, Stdio},
    time::{Duration, Instant},
};
use tempfile::TempDir;

/// A throwaway gateway namespace: its data directory, owner credential and ports.
struct Gateway {
    root: TempDir,
    /// The owner credential `auth init --local` writes, as provisioned by default.
    token: String,
    product: SocketAddr,
    native: Option<SocketAddr>,
}
impl Gateway {
    /// Initialize local access and write `config.json`, naming a native
    /// address only when `native` is set.
    fn new(native: bool) -> Self {
        let root = tempfile::tempdir().unwrap();
        // The data directory is private, as an installation's is.
        nessa_local_storage::create_directory(&root.path().join("data")).unwrap();
        let token_file = root.path().join("owner.token");
        let init = command(&root.path().join("data"))
            .args(["auth", "init", "--local", "--owner-token-file"])
            .arg(&token_file)
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let token = std::fs::read_to_string(&token_file)
            .unwrap()
            .trim()
            .to_owned();
        let native = native.then(free_address);
        let config = match native {
            Some(address) => json!({"native": {"listenAddress": address.to_string()}}),
            None => json!({}),
        };
        let mut file = nessa_local_storage::open(
            &root.path().join("data/ci/config.json"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        file.write_all(config.to_string().as_bytes()).unwrap();
        file.sync_all().unwrap();
        Self {
            root,
            token,
            product: free_address(),
            native,
        }
    }
    fn private_directory(&self) -> PathBuf {
        self.root.path().join("data/ci/native-pairing")
    }
    fn key(&self) -> Vec<u8> {
        std::fs::read(self.private_directory().join("gateway-key")).unwrap()
    }
    /// Start `nessa server` and wait until the product socket accepts.
    fn start(&self) -> Server {
        let log = std::fs::File::create(self.root.path().join("server.log")).unwrap();
        let child = command(&self.root.path().join("data"))
            .arg("server")
            .env("NESSA_PORT", self.product.port().to_string())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut server = Server(Some(child));
        let deadline = Instant::now() + WAIT;
        while TcpStream::connect(self.product).is_err() {
            assert!(Instant::now() < deadline, "the gateway did not start");
            if let Some(status) = server.0.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "the gateway exited during startup: {status}\n{}",
                    std::fs::read_to_string(self.root.path().join("server.log")).unwrap()
                );
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        server
    }
    fn owner(&self) -> ProductClient {
        ProductClient::connect(self.product, &self.token)
    }
}

/// The running process. Dropping it without `stop` kills it.
struct Server(Option<Child>);
impl Server {
    /// SIGTERM, then the exit status once shutdown has reported.
    fn stop(mut self) -> ExitStatus {
        let mut child = self.0.take().unwrap();
        let pid = libc::pid_t::try_from(child.id()).unwrap();
        assert_eq!(unsafe { libc::kill(pid, libc::SIGTERM) }, 0);
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status;
            }
            assert!(Instant::now() < deadline, "the gateway did not stop");
            std::thread::sleep(Duration::from_millis(50));
        }
    }
}
impl Drop for Server {
    fn drop(&mut self) {
        if let Some(mut child) = self.0.take() {
            let _ = child.kill();
            let _ = child.wait();
        }
    }
}

fn command(root: &Path) -> Command {
    let mut command = Command::new(env!("CARGO_BIN_EXE_nessa"));
    command
        .env_clear()
        .env("NESSA_DATA_DIR", root)
        .env("NESSA_STAGE", "ci");
    command
}

fn free_address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}

fn invitation(status: &Value) -> Value {
    status["invitationId"].clone()
}

/// Rows O1, S3, S12, S17: the composed gateway enrolls a device to Approved, and a
/// SIGTERM with a native peer still connected stops it cleanly.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn mounted_gateway_enrolls_a_device_to_approved() {
    let gateway = Gateway::new(true);
    let native = gateway.native.unwrap();
    let server = tokio::task::block_in_place(|| gateway.start());
    assert!(
        !gateway.key().is_empty(),
        "the first start publishes the key"
    );
    // The default owner credential pairs devices (design row S17).
    let created = tokio::task::block_in_place(|| gateway.owner().ok("pairing.create", json!({})));
    let id = invitation(&created["status"]);
    let client_root = tempfile::tempdir().unwrap();
    let (_, store) = pending(client_root.path(), "device");
    let client = NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock);
    let code = ManualCode::parse(created["code"].as_str().unwrap().as_bytes()).unwrap();
    let claimed = tokio::time::timeout(
        WAIT,
        client.enroll(TcpStream::connect(native).unwrap(), code, OsEntropy),
    )
    .await
    .unwrap()
    .unwrap();
    let NativePairingStatus::Claimed(consent) = claimed else {
        panic!("the device claims the invitation: {claimed:?}");
    };
    let approved = tokio::task::block_in_place(|| {
        let mut owner = gateway.owner();
        let status = owner.ok("pairing.status", json!({"invitationId": id}));
        assert_eq!(status["phase"], "claimed");
        owner.ok(
            "pairing.approve",
            json!({"invitationId": id, "deviceKey": status["claimedDeviceKey"]}),
        )
    });
    assert_eq!(approved["phase"], "approved");
    let status = tokio::time::timeout(
        WAIT,
        client.status(
            TcpStream::connect(native).unwrap(),
            Some((*consent).clone()),
        ),
    )
    .await
    .unwrap()
    .unwrap();
    assert_eq!(status, NativePairingStatus::Approved(consent));
    client.shutdown().await;
    // A peer that connects and never speaks is woken by shutdown, not waited on
    // for its TLS deadline.
    let _held = TcpStream::connect(native).unwrap();
    let stopped = tokio::task::block_in_place(|| server.stop());
    assert!(
        stopped.success(),
        "shutdown confirmed every stage: {stopped}"
    );
}

/// Row S1: without a native section the gateway opens no private state and
/// binds no native socket, and the pairing methods are not configured.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_disabled_has_no_native_effect() {
    let gateway = Gateway::new(false);
    let server = tokio::task::block_in_place(|| gateway.start());
    let refused =
        tokio::task::block_in_place(|| gateway.owner().refused("pairing.create", json!({})));
    assert_eq!(refused, "pairing_not_configured");
    assert!(!gateway.private_directory().exists());
    let stopped = tokio::task::block_in_place(|| server.stop());
    assert!(stopped.success(), "{stopped}");
}

/// Rows S3, S6: a restart ends an Available invitation as Restarted by the
/// gateway, keeps the same key, and frees the slot for a new code.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn native_restart_ends_available_and_keeps_the_key() {
    let gateway = Gateway::new(true);
    let server = tokio::task::block_in_place(|| gateway.start());
    let created = tokio::task::block_in_place(|| gateway.owner().ok("pairing.create", json!({})));
    let key = gateway.key();
    assert!(tokio::task::block_in_place(|| server.stop()).success());
    let server = tokio::task::block_in_place(|| gateway.start());
    tokio::task::block_in_place(|| {
        let mut owner = gateway.owner();
        let status = owner.ok(
            "pairing.status",
            json!({"invitationId": invitation(&created["status"])}),
        );
        assert_eq!(status["phase"], "terminal");
        assert_eq!(status["terminal"]["cause"], "restarted");
        assert_eq!(status["terminal"]["initiator"], json!({"kind": "system"}));
        owner.ok("pairing.create", json!({}));
    });
    assert_eq!(gateway.key(), key, "the key is restored, not regenerated");
    assert!(tokio::task::block_in_place(|| server.stop()).success());
}

/// The actions a credential `token` carries, read from the registry at `data`.
async fn grants(data: &Path, token: &str) -> Vec<String> {
    let store = LocalCredentialStore::open(data.join("ci/auth"), "credentials.v1.json").unwrap();
    let gateway = AudienceId::new(store.gateway_id().unwrap()).unwrap();
    let evidence = CredentialEvidence::new(token.trim().as_bytes().to_vec()).unwrap();
    let verified = store.verify(&evidence, &gateway).await.unwrap();
    let mut actions = store
        .read(&verified.credential_id)
        .await
        .unwrap()
        .credential
        .grants()
        .iter()
        .map(|grant| grant.action().as_str().to_owned())
        .collect::<Vec<_>>();
    actions.sort();
    actions
}

const OWNER_GRANTS: [&str; 4] = [
    "conversation.read",
    "conversation.write",
    "credential.manage",
    "server.read",
];

/// Row S17 (panel): the default `nessa-panel` surface carries the owner set,
/// whether `auth init` provisioned it or `provision-surface` without
/// `--grants` provisioned it again.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn default_panel_surface_carries_the_owner_grants() {
    let gateway = Gateway::new(false);
    let data = gateway.root.path().join("data");
    let panel = data.join("ci/auth/surfaces/nessa-panel.token");
    assert_eq!(
        grants(&data, &std::fs::read_to_string(&panel).unwrap()).await,
        OWNER_GRANTS
    );
    assert_eq!(grants(&data, &gateway.token).await, OWNER_GRANTS);
    std::fs::remove_file(&panel).unwrap();
    let again = command(&data)
        .args([
            "auth",
            "provision-surface",
            "--local",
            "--surface-id",
            "nessa-panel",
        ])
        .output()
        .unwrap();
    assert!(
        again.status.success(),
        "{}",
        String::from_utf8_lossy(&again.stderr)
    );
    assert_eq!(
        grants(&data, &std::fs::read_to_string(&panel).unwrap()).await,
        OWNER_GRANTS
    );
}

/// Row S18: an owner credential issued under the earlier three-action set
/// cannot create a code; `auth recover-owner` re-issues it for the same owner
/// with the current set, revoking the old one, and the new one can.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn recovered_old_owner_can_pair_devices() {
    let root = tempfile::tempdir().unwrap();
    let data = root.path().join("data");
    nessa_local_storage::create_directory(&data.join("ci/auth")).unwrap();
    // A registry as `auth init` wrote it before row S17, built through the
    // registry API rather than by editing files.
    let old = {
        let store =
            LocalCredentialStore::open(data.join("ci/auth"), "credentials.v1.json").unwrap();
        let grant = |action: &str| CredentialGrantDto {
            action: action.into(),
            resource: ResourceDto {
                organization_id: "org".into(),
                id: "gateway".into(),
            },
        };
        let issued = store
            .bootstrap(BootstrapRequest {
                gateway_id: "gateway".into(),
                organization: OrganizationInputDto { id: "org".into() },
                principal: PrincipalInputDto {
                    id: "owner".into(),
                    kind: PrincipalKindDto::Human,
                },
                membership: MembershipInputDto {
                    id: "owner-membership".into(),
                    principal_id: "owner".into(),
                    organization_id: "org".into(),
                    role: MembershipRoleDto::Admin,
                    state: MembershipStateDto::Active,
                },
                credential_id: "old-owner".into(),
                issued_at: 1,
                expires_at: None,
                grants: ["server.read", "conversation.write", "credential.manage"]
                    .into_iter()
                    .map(grant)
                    .collect(),
            })
            .unwrap();
        String::from_utf8(issued.evidence.expose_bytes().to_vec()).unwrap()
    };
    let native = free_address();
    let mut file = nessa_local_storage::open(
        &data.join("ci/config.json"),
        nessa_local_storage::OpenMode::CreateNew,
    )
    .unwrap();
    file.write_all(
        json!({"native": {"listenAddress": native.to_string()}})
            .to_string()
            .as_bytes(),
    )
    .unwrap();
    file.sync_all().unwrap();
    let gateway = Gateway {
        root,
        token: old.clone(),
        product: free_address(),
        native: Some(native),
    };
    let server = tokio::task::block_in_place(|| gateway.start());
    let refused =
        tokio::task::block_in_place(|| gateway.owner().refused("pairing.create", json!({})));
    assert_eq!(
        refused, "forbidden",
        "the earlier owner set cannot propose a consent"
    );
    assert!(tokio::task::block_in_place(|| server.stop()).success());
    let next_file = gateway.root.path().join("owner-next.token");
    let recovered = command(&data)
        .args(["auth", "recover-owner", "--local", "--owner-token-file"])
        .arg(&next_file)
        .output()
        .unwrap();
    assert!(
        recovered.status.success(),
        "{}",
        String::from_utf8_lossy(&recovered.stderr)
    );
    let next = std::fs::read_to_string(&next_file)
        .unwrap()
        .trim()
        .to_owned();
    assert_eq!(grants(&data, &next).await, OWNER_GRANTS);
    let gateway = Gateway {
        token: next,
        ..gateway
    };
    let server = tokio::task::block_in_place(|| gateway.start());
    tokio::task::block_in_place(|| {
        let mut owner = gateway.owner();
        let session = owner.ok("auth.session", json!({}));
        assert_eq!(
            session["principalId"], "owner",
            "the owner identity is preserved"
        );
        assert_eq!(session["organizationId"], "org");
        assert_eq!(session["gatewayId"], "gateway");
        owner.ok("pairing.create", json!({}));
    });
    // The earlier owner credential is revoked.
    let (answer, close) =
        tokio::task::block_in_place(|| refused_authentication(gateway.product, &old));
    assert_eq!(answer["id"], "1");
    assert_eq!(answer["ok"], false);
    assert_eq!(
        answer["error"]["code"], "unauthorized",
        "a revoked credential is refused as unauthorized: {answer}"
    );
    // Then the session closes with the published `authentication_failed` code.
    let schema: Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    )))
    .unwrap();
    let authentication_failed =
        &schema["$defs"]["SessionCloseReason"]["x-close-policy"]["authentication_failed"];
    assert_eq!(
        Some(u64::from(close)),
        authentication_failed["webSocketCode"].as_u64()
    );
    assert!(tokio::task::block_in_place(|| server.stop()).success());
}

/// Authenticate `credential` on a fresh socket; return the gateway's answer
/// and the close code it ends the session with.
fn refused_authentication(address: SocketAddr, credential: &str) -> (Value, u16) {
    let (mut socket, _) = tungstenite::connect(format!("ws://{address}/session")).unwrap();
    let challenge: Value = loop {
        if let tungstenite::Message::Text(text) = socket.read().unwrap() {
            break serde_json::from_str(&text).unwrap();
        }
    };
    socket
        .send(tungstenite::Message::Text(
            json!({"type": "req", "id": "1", "method": "session.authenticate", "params": {
                "minVersion": 1, "maxVersion": 1, "nonce": challenge["payload"]["nonce"],
                "credential": credential, "client": {"id": "nessa-cli"}}})
            .to_string()
            .into(),
        ))
        .unwrap();
    let answer: Value = loop {
        if let tungstenite::Message::Text(text) = socket.read().unwrap() {
            break serde_json::from_str(&text).unwrap();
        }
    };
    loop {
        match socket.read().unwrap() {
            tungstenite::Message::Close(Some(frame)) => return (answer, frame.code.into()),
            tungstenite::Message::Close(None) => panic!("the session closed without a reason"),
            _ => continue,
        }
    }
}
