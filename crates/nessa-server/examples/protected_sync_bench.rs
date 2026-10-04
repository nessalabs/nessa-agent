//! End-to-end check and timing of protected native sync, against a real
//! `nessa server` and real `read_only_sync` client processes. Not run in CI.
//!
//! ```text
//! protected_sync_bench --nessa PATH --client PATH [--sizes 20,200,2000]
//! ```
//!
//! For each conversation size N (messages; half user, half assistant, with
//! realistic lengths) it builds a throwaway gateway, seeds one conversation,
//! pairs device A and device B through the owner's product socket and the
//! client's `pair` command, syncs both, checks both caches hold exactly the
//! gateway's messages, writes a new turn on the gateway, re-syncs both, revokes
//! B (whose next command must purge only after its authenticated Terminal
//! status) and checks A keeps converging. The first size also runs the
//! adversarial probes: another key, a replayed authentication, a truncated
//! frame and substituted scopes, each refused without touching A's cache.
//!
//! Every client command runs through a byte-counting TCP relay in front of the
//! native listener, so each pass reports bytes on the wire in each direction,
//! TLS included. Wall times are whole client processes: process start, the
//! pinned status connection, the protected connection and the cache.
//!
//! The report is one JSON document on stdout; diagnostics go to stderr. The
//! exit status is nonzero when any check fails.
use nessa_auth::adapters::pairing::{
    FilePairingState, GatewayTrust, NativeIdentity, NativeTransport, OsEntropy,
};
use nessa_auth::application::pairing::ClientPendingStore;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::pairing::{
    encode_frame,
    wire::{encode_request, NativePairingRequest},
    EnrollmentChannel, FrameReader, MAX_PROTECTED_REQUEST_BYTES, MAX_PROTECTED_RESPONSE_BYTES,
};
use nessa_sdk::application::agent_execution::{
    executions::{ExecutionEvent, ExecutionRequest, ExecutionUpdate},
    permissions::ActionContext,
    providers::ProviderIdentity,
    sessions::{
        InvocationRecord, SessionChange, SessionSaveUnit, SessionSnapshot, SessionStorage,
        SubmissionAcknowledgement,
    },
};
use nessa_sdk::domain::agent_execution::{
    executions::{ExecutionId, ExecutionOutcome, MessageChunk, SubmissionMode},
    prompts::{PromptText, UserMessage},
    sessions::{ExecutionSessionId, ProviderContext, SessionId},
};
use nessa_sdk::infrastructure::session_storage::RecordStorage;
use nessa_server::agents::domain::AgentId;
use nessa_server::conversation::application::ConversationRepository;
use nessa_server::conversation::domain::{
    Conversation, ConversationApprovalMode, ConversationId, ConversationModelId,
};
use nessa_server::conversation::infrastructure::LocalConversationStore;
use serde_json::{json, Value};
use std::io::{Read, Write};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitCode, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::{Duration, Instant};
use tungstenite::{stream::MaybeTlsStream, Message, WebSocket};

const WAIT: Duration = Duration::from_secs(60);
const PAGES: &str = "100000";

fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_writer(std::io::stderr)
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()),
        )
        .init();
    let Some(arguments) = Arguments::parse(std::env::args().skip(1).collect()) else {
        tracing::error!(
            "usage: protected_sync_bench --nessa PATH --client PATH [--sizes 20,200,2000]"
        );
        return ExitCode::from(2);
    };
    let mut runs = Vec::new();
    let mut checks = Checks::default();
    for (index, size) in arguments.sizes.iter().enumerate() {
        tracing::info!(messages = size, "run");
        runs.push(run(&arguments, *size, index == 0, &mut checks));
    }
    let report = json!({
        "machine": machine(),
        "runs": runs,
        "checks": checks.report(),
        "allChecksPassed": checks.passed(),
    });
    println!("{}", serde_json::to_string_pretty(&report).unwrap());
    if checks.passed() {
        ExitCode::SUCCESS
    } else {
        ExitCode::FAILURE
    }
}

struct Arguments {
    nessa: PathBuf,
    client: PathBuf,
    sizes: Vec<usize>,
}
impl Arguments {
    fn parse(arguments: Vec<String>) -> Option<Self> {
        let mut nessa = None;
        let mut client = None;
        let mut sizes = vec![20, 200, 2000];
        let mut arguments = arguments.into_iter();
        while let Some(name) = arguments.next() {
            let value = arguments.next()?;
            match name.as_str() {
                "--nessa" => nessa = Some(PathBuf::from(value)),
                "--client" => client = Some(PathBuf::from(value)),
                "--sizes" => {
                    sizes = value
                        .split(',')
                        .map(|size| size.parse().ok().filter(|size: &usize| *size >= 2))
                        .collect::<Option<_>>()?
                }
                _ => return None,
            }
        }
        Some(Self {
            nessa: nessa?,
            client: client?,
            sizes,
        })
    }
}

/// Named pass/fail results, in the order they were checked.
#[derive(Default)]
struct Checks(Vec<(String, bool, Value)>);
impl Checks {
    fn check(&mut self, name: impl Into<String>, passed: bool, evidence: Value) {
        let name = name.into();
        if !passed {
            tracing::error!(%name, %evidence, "check failed");
        }
        self.0.push((name, passed, evidence));
    }
    fn passed(&self) -> bool {
        self.0.iter().all(|(_, passed, _)| *passed)
    }
    fn report(&self) -> Value {
        Value::Array(
            self.0
                .iter()
                .map(|(name, passed, evidence)| {
                    if *passed {
                        json!({"name":name,"passed":true})
                    } else {
                        json!({"name":name,"passed":false,"evidence":evidence})
                    }
                })
                .collect(),
        )
    }
}

fn machine() -> Value {
    let read = |program: &str, arguments: &[&str]| {
        Command::new(program)
            .args(arguments)
            .output()
            .ok()
            .map(|output| String::from_utf8_lossy(&output.stdout).trim().to_owned())
    };
    json!({
        "os": std::env::consts::OS,
        "arch": std::env::consts::ARCH,
        "cpu": read("sysctl", &["-n", "machdep.cpu.brand_string"]),
        "logicalCpus": read("sysctl", &["-n", "hw.ncpu"]),
        "memoryBytes": read("sysctl", &["-n", "hw.memsize"]),
        "loadAtStart": read("sysctl", &["-n", "vm.loadavg"]),
        "build": if cfg!(debug_assertions) { "debug" } else { "release" },
    })
}

/// One size: a fresh gateway, two devices, one conversation of `size` messages.
fn run(arguments: &Arguments, size: usize, probes: bool, checks: &mut Checks) -> Value {
    let directory = tempfile::tempdir().unwrap();
    let gateway = Gateway::new(directory.path(), &arguments.nessa);
    let server = gateway.start();
    let (organization, owner) = gateway.owner().identity();
    server.stop();
    let conversation = uuid();
    let started = Instant::now();
    let seeded = seed(&gateway, &organization, &owner, &conversation, size);
    let seed_ms = started.elapsed().as_millis();
    let mut server = gateway.start();
    let relay = Relay::start(gateway.native);
    let a = Device::new(
        directory.path(),
        "device-a",
        relay.address,
        &arguments.client,
    );
    let b = Device::new(
        directory.path(),
        "device-b",
        relay.address,
        &arguments.client,
    );
    let paired_a = a.pair(&gateway);
    checks.check(format!("n{size}.a.paired"), paired_a.is_some(), Value::Null);
    let baseline = a.timed(&relay, &["status", &a.profile_arg()]);
    let catalogue_a = a.timed(&relay, &["sync-catalogue", &a.profile_arg(), PAGES]);
    let (initial_a, preparing_a, ready_a) = a.synced(&relay, &conversation);
    checks.check(
        format!("n{size}.a.initial_sync_complete"),
        initial_a.ok && initial_a.value["work"]["complete"] == true,
        initial_a.value.clone(),
    );
    let paired_b = b.pair(&gateway);
    checks.check(format!("n{size}.b.paired"), paired_b.is_some(), Value::Null);
    let catalogue_b = b.timed(&relay, &["sync-catalogue", &b.profile_arg(), PAGES]);
    let (initial_b, preparing_b, ready_b) = b.synced(&relay, &conversation);
    checks.check(
        format!("n{size}.b.initial_sync_complete"),
        initial_b.ok && initial_b.value["work"]["complete"] == true,
        initial_b.value.clone(),
    );
    let origin = initial_a.value["durable"]["progress"]["scope"]["origin"]
        .as_str()
        .unwrap_or_default()
        .to_owned();
    let receiver_a = paired_a.clone().unwrap_or_default();
    let receiver_b = paired_b.clone().unwrap_or_default();
    converged(
        checks,
        &format!("n{size}.initial"),
        &seeded,
        &[(&a, &receiver_a), (&b, &receiver_b)],
        &origin,
        &conversation,
    );

    // A new turn on the gateway; both devices re-sync incrementally.
    server.stop();
    let seeded = append_turn(&gateway, &conversation, seeded);
    server = gateway.start();
    let (incremental_a, preparing_incremental, ready_incremental) = a.synced(&relay, &conversation);
    let (incremental_b, _, _) = b.synced(&relay, &conversation);
    checks.check(
        format!("n{size}.incremental_syncs_complete"),
        incremental_a.ok && incremental_b.ok,
        json!([incremental_a.value, incremental_b.value]),
    );
    converged(
        checks,
        &format!("n{size}.after_new_turn"),
        &seeded,
        &[(&a, &receiver_a), (&b, &receiver_b)],
        &origin,
        &conversation,
    );

    let probe_report = if probes {
        probe(checks, &gateway, &a, &b, &receiver_a, &receiver_b)
    } else {
        Value::Null
    };

    // Revoke B: its next command reads Terminal and purges, A keeps reading.
    let credential_b = b.credential.lock().unwrap().clone().unwrap_or_default();
    gateway.owner().ok(
        "credential.revoke",
        json!({"requestId": uuid(), "credentialId": credential_b}),
    );
    let revoked = b.timed(
        &relay,
        &["sync-records", &b.profile_arg(), &conversation, PAGES],
    );
    checks.check(
        format!("n{size}.b.revoked_reads_terminal_and_purges"),
        !revoked.ok
            && revoked.value["enrollment"]["phase"] == "terminal"
            && revoked.value["enrollment"]["cause"] == "credentialRevoked"
            && revoked.value["purge"]["receiver"] == receiver_b.as_str(),
        revoked.value.clone(),
    );
    let purged_rows = b.cached_rows(&receiver_b);
    checks.check(
        format!("n{size}.b.cache_purged"),
        purged_rows == 0,
        json!({"rows": purged_rows}),
    );
    let after_revoke = b.run(&["sync-records", &b.profile_arg(), &conversation, PAGES]);
    checks.check(
        format!("n{size}.b.no_longer_paired"),
        !after_revoke.ok && after_revoke.value["configurationFailure"] == "notPaired",
        after_revoke.value,
    );
    server.stop();
    let seeded = append_turn(&gateway, &conversation, seeded);
    server = gateway.start();
    let (converging, _, _) = a.synced(&relay, &conversation);
    checks.check(
        format!("n{size}.a.keeps_converging"),
        converging.ok,
        converging.value.clone(),
    );
    converged(
        checks,
        &format!("n{size}.a.after_b_revoked"),
        &seeded,
        &[(&a, &receiver_a)],
        &origin,
        &conversation,
    );
    let stopped = server.stop();
    checks.check(
        format!("n{size}.gateway_stops_cleanly"),
        stopped,
        Value::Null,
    );
    json!({
        "messages": size,
        "seedMs": seed_ms,
        "recordsDownloaded": initial_a.value["durable"]["progress"]["downloaded"],
        "statusOnly": baseline.measure(),
        "catalogue": {"a": catalogue_a.measure(), "b": catalogue_b.measure()},
        "initialSync": {"a": initial_a.measure(), "b": initial_b.measure()},
        "incrementalSync": {"a": incremental_a.measure(), "b": incremental_b.measure()},
        "sourcePreparing": {
            "initialA": {"attempts": preparing_a, "untilPassMs": ready_a.round()},
            "initialB": {"attempts": preparing_b, "untilPassMs": ready_b.round()},
            "afterRestart": {"attempts": preparing_incremental, "untilPassMs": ready_incremental.round()},
        },
        "revokedCommand": revoked.measure(),
        "afterRevokeSync": converging.measure(),
        "probes": probe_report,
    })
}

/// Both caches hold exactly the gateway's messages, in order, and agree with
/// each other on every projected field but the per-read revision.
fn converged(
    checks: &mut Checks,
    name: &str,
    seeded: &[(String, String)],
    devices: &[(&Device, &String)],
    origin: &str,
    conversation: &str,
) {
    let mut views = Vec::new();
    for (device, receiver) in devices {
        let shown = device.run(&["show", &device.cache_arg(), receiver, origin, conversation]);
        // The retained view is display-bounded: it shows the most recent
        // turns, and says when it left older ones out.
        let messages = messages(&shown.value["view"]);
        let truncated = shown.value["view"]["truncated"] == true;
        let expected = &seeded[seeded.len() - messages.len().min(seeded.len())..];
        checks.check(
            format!("{name}.{}.equals_gateway_turns", device.name),
            shown.ok
                && !messages.is_empty()
                && messages == expected
                && (truncated || messages.len() == seeded.len())
                && shown.value["downloaded"] == shown.value["applied"],
            json!({"shown": messages.len(), "gateway": seeded.len(), "truncated": truncated,
                "downloaded": shown.value["downloaded"], "applied": shown.value["applied"]}),
        );
        let mut view = shown.value["view"].clone();
        if let Some(view) = view.as_object_mut() {
            view.remove("revision");
        }
        views.push(view);
    }
    if views.len() == 2 {
        checks.check(
            format!("{name}.devices_agree"),
            views[0] == views[1],
            Value::Null,
        );
    }
}

/// (user text, assistant text) of each projected turn.
fn messages(view: &Value) -> Vec<(String, String)> {
    view["messages"]
        .as_array()
        .map(|messages| {
            messages
                .iter()
                .map(|message| {
                    let reply = message["parts"]
                        .as_array()
                        .map(|parts| {
                            parts
                                .iter()
                                .filter_map(|part| part["text"].as_str())
                                .collect::<String>()
                        })
                        .unwrap_or_default();
                    (
                        message["userText"].as_str().unwrap_or_default().to_owned(),
                        reply,
                    )
                })
                .collect()
        })
        .unwrap_or_default()
}

/// The adversarial rows against A's real enrollment: each refused, and A's
/// cache untouched.
fn probe(
    checks: &mut Checks,
    gateway: &Gateway,
    a: &Device,
    b: &Device,
    receiver_a: &str,
    receiver_b: &str,
) -> Value {
    let before = std::fs::read(&a.cache).unwrap_or_default();
    let (pin, credential, epoch) = a.saved();
    let address = gateway.native;
    let identity = || a.identity();
    // Another key, holding no device credential, is refused at `openProduct`
    // before it can hold a product permit or present A's credential id.
    let stranger = Probe::open(
        address,
        NativeIdentity::generate(&mut OsEntropy).unwrap(),
        pin,
    );
    let wrong_key = stranger.first.clone();
    checks.check(
        "probe.wrong_key_refused",
        wrong_key["kind"] == "refused",
        wrong_key.clone(),
    );
    // A recorded authentication replayed on another connection.
    let first = Probe::open(address, identity(), pin);
    let mut second = Probe::open(address, identity(), pin);
    let replayed = second.authenticate(&credential, &first.nonce);
    checks.check(
        "probe.replayed_authentication_refused",
        code(&replayed) == "unauthorized",
        replayed.clone().unwrap_or_default(),
    );
    // A frame cut short, then a scope naming B's receiver or a stale epoch.
    let mut truncated = Probe::open(address, identity(), pin);
    let nonce = truncated.nonce.clone();
    let ready = truncated.authenticate(&credential, &nonce);
    truncated.send_raw(&64u32.to_be_bytes());
    truncated.send_raw(b"{\"type\":\"req\"");
    drop(truncated);
    checks.check(
        "probe.truncated_frame_session_was_ready",
        ready.as_ref().is_some_and(|ready| ready["ok"] == true),
        Value::Null,
    );
    let mut scoped = Probe::open(address, identity(), pin);
    let nonce = scoped.nonce.clone();
    scoped.authenticate(&credential, &nonce);
    let other = scoped.call(
        "conversation.catalogueHead",
        json!({"receiverId": receiver_b, "accessEpoch": epoch.to_string()}),
    );
    let stale = scoped.call(
        "conversation.catalogueHead",
        json!({"receiverId": receiver_a, "accessEpoch": (epoch + 1).to_string()}),
    );
    checks.check(
        "probe.other_receiver_refused",
        code(&other) == "wrong_receiver",
        other.clone().unwrap_or_default(),
    );
    checks.check(
        "probe.stale_epoch_refused",
        code(&stale) == "stale_epoch",
        stale.clone().unwrap_or_default(),
    );
    let after = std::fs::read(&a.cache).unwrap_or_default();
    checks.check("probe.cache_untouched", before == after, Value::Null);
    let _ = b;
    json!({"wrongKey": wrong_key["kind"], "replayed": code(&replayed),
        "otherReceiver": code(&other), "staleEpoch": code(&stale)})
}
fn code(response: &Option<Value>) -> String {
    response
        .as_ref()
        .and_then(|response| response["error"]["code"].as_str())
        .unwrap_or("closed")
        .to_owned()
}

/// A throwaway gateway namespace and its two ports.
struct Gateway {
    root: PathBuf,
    nessa: PathBuf,
    token: String,
    product: SocketAddr,
    native: SocketAddr,
}
impl Gateway {
    fn new(parent: &Path, nessa: &Path) -> Self {
        let root = parent.join("gateway");
        nessa_local_storage::create_directory(&root.join("data")).unwrap();
        let token_file = root.join("owner.token");
        let init = command(nessa, &root.join("data"))
            .args(["auth", "init", "--local", "--owner-token-file"])
            .arg(&token_file)
            .output()
            .unwrap();
        assert!(
            init.status.success(),
            "{}",
            String::from_utf8_lossy(&init.stderr)
        );
        let native = free_address();
        let workspace = root.join("workspace");
        nessa_local_storage::create_directory(&workspace).unwrap();
        let catalog = Path::new(env!("CARGO_MANIFEST_DIR")).join("../nessa-sdk/data/models.json");
        // An agent must be configured for the conversation stack, and with it
        // the record and catalogue sources, to be composed. Nothing starts it.
        let config = json!({
            "native": {"listenAddress": native.to_string()},
            "agents": {"catalog": catalog, "workspace": workspace,
                "runtimes": {"claude": {"command": "/usr/bin/true", "model": "claude-sonnet-5",
                    "toolsEnabled": true}}},
        });
        let mut file = nessa_local_storage::open(
            &root.join("data/ci/config.json"),
            nessa_local_storage::OpenMode::CreateNew,
        )
        .unwrap();
        file.write_all(config.to_string().as_bytes()).unwrap();
        file.sync_all().unwrap();
        Self {
            token: std::fs::read_to_string(&token_file)
                .unwrap()
                .trim()
                .to_owned(),
            nessa: nessa.to_owned(),
            product: free_address(),
            native,
            root,
        }
    }
    fn namespace(&self) -> PathBuf {
        self.root.join("data/ci")
    }
    fn start(&self) -> Server {
        let log = std::fs::OpenOptions::new()
            .create(true)
            .append(true)
            .open(self.root.join("server.log"))
            .unwrap();
        let child = command(&self.nessa, &self.root.join("data"))
            .arg("server")
            .env("NESSA_PORT", self.product.port().to_string())
            .stdout(Stdio::null())
            .stderr(log)
            .spawn()
            .unwrap();
        let mut server = Server(Some(child));
        let deadline = Instant::now() + WAIT;
        while TcpStream::connect(self.product).is_err() || TcpStream::connect(self.native).is_err()
        {
            assert!(Instant::now() < deadline, "the gateway did not start");
            if let Some(status) = server.0.as_mut().unwrap().try_wait().unwrap() {
                panic!(
                    "the gateway exited during startup: {status}\n{}",
                    std::fs::read_to_string(self.root.join("server.log")).unwrap_or_default()
                );
            }
            thread::sleep(Duration::from_millis(50));
        }
        server
    }
    fn owner(&self) -> Owner {
        Owner::connect(self.product, &self.token)
    }
}
struct Server(Option<Child>);
impl Server {
    /// SIGTERM, and whether the process exited successfully.
    fn stop(mut self) -> bool {
        let mut child = self.0.take().unwrap();
        let _ = Command::new("kill")
            .arg("-TERM")
            .arg(child.id().to_string())
            .status();
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = child.try_wait().unwrap() {
                return status.success();
            }
            if Instant::now() > deadline {
                let _ = child.kill();
                return false;
            }
            thread::sleep(Duration::from_millis(20));
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
fn command(program: &Path, data: &Path) -> Command {
    let mut command = Command::new(program);
    command
        .env_clear()
        .env("NESSA_DATA_DIR", data)
        .env("NESSA_STAGE", "ci")
        .env("PATH", "/usr/bin:/bin");
    if let Some(home) = std::env::var_os("HOME") {
        command.env("HOME", home);
    }
    command
}
fn free_address() -> SocketAddr {
    TcpListener::bind("127.0.0.1:0")
        .unwrap()
        .local_addr()
        .unwrap()
}
fn uuid() -> String {
    uuid::Uuid::new_v4().to_string()
}

/// The owner's product socket: the real WebSocket handshake with the owner
/// credential `auth init` wrote.
struct Owner {
    socket: WebSocket<MaybeTlsStream<TcpStream>>,
    ready: Value,
    next: u64,
}
impl Owner {
    fn connect(address: SocketAddr, credential: &str) -> Self {
        let (mut socket, _) = tungstenite::connect(format!("ws://{address}/session")).unwrap();
        let challenge = read_frame(&mut socket);
        let mut owner = Self {
            socket,
            ready: Value::Null,
            next: 0,
        };
        owner.ready = owner.ok(
            "session.authenticate",
            json!({"minVersion":1,"maxVersion":1,"nonce":challenge["payload"]["nonce"],
                "credential":credential,"client":{"id":"bench-owner"}}),
        );
        owner
    }
    fn identity(&self) -> (String, String) {
        (
            self.ready["organizationId"].as_str().unwrap().to_owned(),
            self.ready["principalId"].as_str().unwrap().to_owned(),
        )
    }
    fn ok(&mut self, method: &str, params: Value) -> Value {
        self.next += 1;
        let id = self.next.to_string();
        self.socket
            .send(Message::Text(
                json!({"type":"req","id":id,"method":method,"params":params})
                    .to_string()
                    .into(),
            ))
            .unwrap();
        loop {
            let frame = read_frame(&mut self.socket);
            if frame["type"] == "res" && frame["id"] == id {
                assert_eq!(frame["ok"], true, "{method}: {frame}");
                return frame["payload"].clone();
            }
        }
    }
}
fn read_frame(socket: &mut WebSocket<MaybeTlsStream<TcpStream>>) -> Value {
    loop {
        if let Message::Text(text) = socket.read().unwrap() {
            return serde_json::from_str(&text).unwrap();
        }
    }
}

/// One device: its private state, cache and profile, run as client processes.
struct Device {
    name: &'static str,
    root: PathBuf,
    cache: PathBuf,
    profile: PathBuf,
    client: PathBuf,
    credential: std::sync::Mutex<Option<String>>,
}
impl Device {
    fn new(parent: &Path, name: &'static str, gateway: SocketAddr, client: &Path) -> Self {
        let root = parent.join(name);
        nessa_local_storage::create_directory(&root).unwrap();
        #[cfg(unix)]
        let root = root.canonicalize().unwrap();
        let profile = root.join("profile.json");
        let mut file =
            nessa_local_storage::open(&profile, nessa_local_storage::OpenMode::CreateNew).unwrap();
        file.write_all(
            json!({"stateRoot": root, "stateDirectory": "state",
                "cache": root.join("cache.sqlite3"), "gatewayAddress": gateway.to_string()})
            .to_string()
            .as_bytes(),
        )
        .unwrap();
        Self {
            name,
            cache: root.join("cache.sqlite3"),
            profile,
            root,
            client: client.to_owned(),
            credential: std::sync::Mutex::new(None),
        }
    }
    fn cache_arg(&self) -> String {
        self.cache.to_string_lossy().into_owned()
    }
    fn profile_arg(&self) -> String {
        self.profile.to_string_lossy().into_owned()
    }
    /// Pair through the owner's product socket and the client's own `pair`
    /// and `status` commands; returns the issued receiver.
    fn pair(&self, gateway: &Gateway) -> Option<String> {
        let mut owner = gateway.owner();
        let created = owner.ok("pairing.create", json!({}));
        let id = created["status"]["invitationId"].clone();
        let mut child = Command::new(&self.client)
            .args(["pair", &self.profile_arg()])
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .unwrap();
        child
            .stdin
            .take()
            .unwrap()
            .write_all(format!("{}\n", created["code"].as_str().unwrap()).as_bytes())
            .unwrap();
        let paired = child.wait_with_output().unwrap();
        let paired: Value = serde_json::from_slice(&paired.stdout).ok()?;
        if paired["enrollment"]["phase"] != "claimed" {
            tracing::error!(%paired, "pair did not claim");
            return None;
        }
        let status = owner.ok("pairing.status", json!({"invitationId": id}));
        let approved = owner.ok(
            "pairing.approve",
            json!({"invitationId": id, "deviceKey": status["claimedDeviceKey"]}),
        );
        if approved["status"]["phase"] != "active" {
            tracing::error!(%approved, "approval did not activate");
            return None;
        }
        let active = self.run(&["status", &self.profile_arg()]);
        let enrollment = &active.value["enrollment"];
        if enrollment["phase"] != "active" {
            return None;
        }
        *self.credential.lock().unwrap() = enrollment["credentialId"].as_str().map(str::to_owned);
        enrollment["receiver"].as_str().map(str::to_owned)
    }
    fn run(&self, arguments: &[&str]) -> Ran {
        let started = Instant::now();
        let output = Command::new(&self.client)
            .args(arguments)
            .stdin(Stdio::null())
            .output()
            .unwrap();
        Ran {
            ok: output.status.success(),
            value: String::from_utf8_lossy(&output.stdout)
                .lines()
                .find(|line| line.starts_with('{'))
                .and_then(|line| serde_json::from_str(line).ok())
                .unwrap_or(Value::Null),
            millis: started.elapsed().as_secs_f64() * 1000.0,
            up: 0,
            down: 0,
            connections: 0,
        }
    }
    /// A client command through the relay, with its bytes on the wire.
    fn timed(&self, relay: &Relay, arguments: &[&str]) -> Ran {
        let before = relay.counters();
        let mut ran = self.run(arguments);
        let after = relay.counters();
        ran.up = after.0 - before.0;
        ran.down = after.1 - before.1;
        ran.connections = after.2 - before.2;
        ran
    }
    /// A record sync, repeated while the gateway answers `source_preparing`:
    /// after a restart it first restores the conversation's committed view,
    /// which is temporary unavailability, not an empty source. The pass that
    /// ran is returned, with how many attempts and how long it took to reach.
    fn synced(&self, relay: &Relay, conversation: &str) -> (Ran, u32, f64) {
        let started = Instant::now();
        let mut attempts = 0;
        loop {
            attempts += 1;
            let ran = self.timed(
                relay,
                &["sync-records", &self.profile_arg(), conversation, PAGES],
            );
            let preparing = ran.value["transportFailure"]["productCode"] == "source_preparing";
            if !preparing || started.elapsed() > WAIT {
                return (ran, attempts, started.elapsed().as_secs_f64() * 1000.0);
            }
            thread::sleep(Duration::from_millis(100));
        }
    }
    /// Cached rows of any kind for `receiver`.
    fn cached_rows(&self, receiver: &str) -> i64 {
        let database = nessa_local_database::rusqlite::Connection::open(&self.cache).unwrap();
        [
            "transcript_records",
            "transcript_progress",
            "transcript_checkpoints",
            "catalogue_entries",
            "catalogue_progress",
        ]
        .iter()
        .map(|table| {
            database
                .query_row(
                    &format!("SELECT COUNT(*) FROM {table} WHERE receiver = ?1"),
                    [receiver],
                    |row| row.get::<_, i64>(0),
                )
                .unwrap()
        })
        .sum()
    }
    /// The gateway pin, credential id and current epoch, read the way the
    /// client reads them; the private state is released afterwards.
    fn saved(&self) -> ([u8; 44], String, u64) {
        let state = FilePairingState::open(&self.root, Path::new("state")).unwrap();
        let saved = state.load_credential().unwrap().unwrap();
        let credential = saved.credential().as_str().to_owned();
        let pin = *saved.gateway_pin();
        drop(state);
        let status = self.run(&["status", &self.profile_arg()]);
        let epoch = status.value["enrollment"]["accessEpoch"]
            .as_str()
            .and_then(|epoch| epoch.parse().ok())
            .unwrap_or(1);
        (pin, credential, epoch)
    }
    /// The device's own key, restored from its private state.
    fn identity(&self) -> NativeIdentity {
        let state = FilePairingState::open(&self.root, Path::new("state")).unwrap();
        let (key, _, _) = state
            .load_credential()
            .unwrap()
            .unwrap()
            .into_enrollment()
            .into_parts();
        NativeIdentity::restore(key).unwrap()
    }
}
struct Ran {
    ok: bool,
    value: Value,
    millis: f64,
    up: u64,
    down: u64,
    connections: u64,
}
impl Ran {
    fn measure(&self) -> Value {
        json!({"ok": self.ok, "wallMs": (self.millis * 10.0).round() / 10.0,
            "bytesUp": self.up, "bytesDown": self.down, "connections": self.connections,
            "pages": self.value["work"]["pages"], "downloaded": self.value["work"]["downloaded"]})
    }
}

/// A byte-counting TCP relay in front of the native listener.
struct Relay {
    address: SocketAddr,
    up: Arc<AtomicU64>,
    down: Arc<AtomicU64>,
    connections: Arc<AtomicU64>,
}
impl Relay {
    fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let up = Arc::new(AtomicU64::new(0));
        let down = Arc::new(AtomicU64::new(0));
        let connections = Arc::new(AtomicU64::new(0));
        let (up_count, down_count, opened) = (up.clone(), down.clone(), connections.clone());
        thread::spawn(move || {
            for client in listener.incoming().flatten() {
                let Ok(server) = TcpStream::connect(upstream) else {
                    continue;
                };
                opened.fetch_add(1, Ordering::SeqCst);
                let _ = client.set_nodelay(true);
                let _ = server.set_nodelay(true);
                copy(
                    client.try_clone().unwrap(),
                    server.try_clone().unwrap(),
                    up_count.clone(),
                );
                copy(server, client, down_count.clone());
            }
        });
        Self {
            address,
            up,
            down,
            connections,
        }
    }
    fn counters(&self) -> (u64, u64, u64) {
        // Let the relay threads finish forwarding what the closed client sent.
        thread::sleep(Duration::from_millis(20));
        (
            self.up.load(Ordering::SeqCst),
            self.down.load(Ordering::SeqCst),
            self.connections.load(Ordering::SeqCst),
        )
    }
}
fn copy(mut from: TcpStream, mut to: TcpStream, count: Arc<AtomicU64>) {
    thread::spawn(move || {
        let mut buffer = [0; 64 * 1024];
        loop {
            match from.read(&mut buffer) {
                Ok(0) | Err(_) => break,
                Ok(read) => {
                    count.fetch_add(read as u64, Ordering::SeqCst);
                    if to.write_all(&buffer[..read]).is_err() {
                        break;
                    }
                }
            }
        }
        let _ = to.shutdown(std::net::Shutdown::Write);
    });
}

/// A raw protected connection, framing product messages itself.
struct Probe {
    transport: NativeTransport<TcpStream>,
    frames: FrameReader,
    next: u64,
    nonce: String,
    /// The gateway's first reply: the challenge, or a refused envelope.
    first: Value,
}
impl Probe {
    fn open(address: SocketAddr, identity: NativeIdentity, pin: [u8; 44]) -> Self {
        let socket = TcpStream::connect(address).unwrap();
        socket.set_read_timeout(Some(WAIT)).unwrap();
        let transport =
            NativeTransport::connect(socket, &identity, GatewayTrust::Pinned(pin)).unwrap();
        let mut selector = EnrollmentChannel::new(transport);
        selector
            .send_envelope(&encode_request(&NativePairingRequest::OpenProduct).unwrap())
            .unwrap();
        let mut probe = Self {
            transport: selector.into_transport(),
            frames: FrameReader::new(MAX_PROTECTED_RESPONSE_BYTES),
            next: 0,
            nonce: String::new(),
            first: Value::Null,
        };
        let challenge = probe.value().unwrap_or_default();
        probe.first = challenge.clone();
        probe.nonce = challenge["payload"]["nonce"]
            .as_str()
            .unwrap_or_default()
            .to_owned();
        probe
    }
    fn value(&mut self) -> Option<Value> {
        loop {
            let buffer = self.frames.unfilled().ok()?;
            let count = self
                .transport
                .read(buffer)
                .ok()
                .filter(|count| *count > 0)?;
            if let Some(body) = self.frames.filled(count).ok()? {
                return serde_json::from_slice(&body).ok();
            }
        }
    }
    fn send_raw(&mut self, bytes: &[u8]) {
        let _ = self.transport.write_all(bytes);
        let _ = self.transport.flush();
    }
    fn call(&mut self, method: &str, params: Value) -> Option<Value> {
        self.next += 1;
        let id = self.next.to_string();
        let text = json!({"type":"req","id":id,"method":method,"params":params}).to_string();
        self.send_raw(&encode_frame(MAX_PROTECTED_REQUEST_BYTES, text.as_bytes()).unwrap());
        loop {
            let value = self.value()?;
            if value["id"] == id {
                return Some(value);
            }
        }
    }
    fn authenticate(&mut self, credential: &str, nonce: &str) -> Option<Value> {
        self.call(
            "session.authenticate",
            json!({"minVersion":1,"maxVersion":1,"nonce":nonce,"credential":credential,
                "client":{"id":"bench-probe"}}),
        )
    }
}

/// Seed one conversation of `size` messages into the stopped gateway's own
/// stores: its ownership record and complete turns in the SDK record store.
fn seed(
    gateway: &Gateway,
    organization: &str,
    owner: &str,
    conversation: &str,
    size: usize,
) -> Vec<(String, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let root = gateway.namespace().join("conversations");
        let metadata = LocalConversationStore::open(&root.join("metadata.sqlite3")).unwrap();
        metadata
            .create(
                Conversation::new(
                    ConversationId::new(conversation).unwrap(),
                    OrganizationId::new(organization).unwrap(),
                    PrincipalId::new(owner).unwrap(),
                    "bench".into(),
                    uuid(),
                    1,
                    AgentId::Claude,
                    ConversationModelId::new("claude-sonnet-5").unwrap(),
                    ConversationApprovalMode::Ask,
                )
                .unwrap(),
            )
            .await
            .unwrap();
        let storage = RecordStorage::new(root.join("sessions")).unwrap();
        storage.initialize().await.unwrap();
        let id = SessionId::new(conversation).unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let provider = ProviderIdentity::new("claude", "claude-sonnet-5", "workspace").unwrap();
        // Provider observations are owned by a recorded provider session.
        let context =
            ProviderContext::Recorded(ExecutionSessionId::new("bench-provider-session").unwrap());
        let mut snapshot = SessionSnapshot {
            id: id.clone(),
            provider: provider.clone(),
            provider_context: context.clone(),
            invocations: Vec::new(),
            queue_history: Vec::new(),
        };
        let mut binding = lease.load().await.unwrap().binding().clone();
        let mut units = vec![SessionSaveUnit::new(vec![SessionChange::Opened {
            id: id.clone(),
            provider,
            context,
        }])
        .unwrap()];
        let mut seeded = Vec::new();
        for turn in 0..size / 2 {
            let (record, user, assistant) = turn_record(turn);
            units.push(turn_unit(&record));
            snapshot.invocations.push(record);
            seeded.push((user, assistant));
            if units.len() >= 25 {
                binding = lease
                    .save_changes(binding, snapshot.clone(), std::mem::take(&mut units))
                    .await
                    .unwrap()
                    .next()
                    .clone();
            }
        }
        if !units.is_empty() {
            lease.save_changes(binding, snapshot, units).await.unwrap();
        }
        drop(lease);
        storage.shutdown().await.unwrap();
        seeded
    })
}

/// One more complete turn at the end of the conversation.
fn append_turn(
    gateway: &Gateway,
    conversation: &str,
    mut seeded: Vec<(String, String)>,
) -> Vec<(String, String)> {
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    runtime.block_on(async {
        let storage =
            RecordStorage::new(gateway.namespace().join("conversations/sessions")).unwrap();
        storage.initialize().await.unwrap();
        let id = SessionId::new(conversation).unwrap();
        let lease = storage.open(id.clone()).await.unwrap();
        let loaded = lease.load().await.unwrap();
        let binding = loaded.binding().clone();
        let mut snapshot = loaded.into_published(&id).unwrap().0.unwrap();
        let (record, user, assistant) = turn_record(seeded.len());
        let unit = turn_unit(&record);
        snapshot.invocations.push(record);
        lease
            .save_changes(binding, snapshot, vec![unit])
            .await
            .unwrap();
        drop(lease);
        storage.shutdown().await.unwrap();
        seeded.push((user, assistant));
        seeded
    })
}

/// A complete turn: a user message of 80–400 bytes and an assistant reply of
/// 400–2400 bytes, deterministic in the turn number.
fn turn_record(turn: usize) -> (InvocationRecord, String, String) {
    let id = ExecutionId::new(format!("turn-{turn:06}")).unwrap();
    let user = filler(turn, 80 + (turn * 37) % 320, "Question");
    let assistant = filler(turn, 400 + (turn * 211) % 2000, "Answer");
    let record = InvocationRecord {
        target_event_offset: None,
        submission: SubmissionMode::Immediate,
        request: ExecutionRequest {
            execution_id: id.clone(),
            user_message: UserMessage::text_only(PromptText::new(user.clone()).unwrap()),
            estimated_input_tokens: 1,
            reserved_output_tokens: 1,
        },
        actor: ActionContext::new("bench", "bench", format!("turn-{turn}")).unwrap(),
        acknowledgement: SubmissionAcknowledgement::Pending,
        events: vec![
            ExecutionEvent::new(
                id.clone(),
                ExecutionUpdate::Message(MessageChunk::text(assistant.clone())),
            ),
            ExecutionEvent::new(id, ExecutionUpdate::Finished(ExecutionOutcome::Completed)),
        ],
        scheduling: Vec::new(),
        provider_report: None,
        local_cancellation: None,
        local_outcome: Some(ExecutionOutcome::Completed),
        cancellation: None,
        result: Some(Ok(ExecutionOutcome::Completed)),
    };
    (record, user, assistant)
}
/// The decisions that saved `record`, in the order the SDK writes them.
fn turn_unit(record: &InvocationRecord) -> SessionSaveUnit {
    let mut input = record.clone();
    input.events.clear();
    input.result = None;
    input.local_outcome = None;
    let mut changes = vec![SessionChange::InputAccepted(Box::new(input))];
    changes.extend(
        record
            .events
            .iter()
            .cloned()
            .map(SessionChange::ProviderObservation),
    );
    changes.push(SessionChange::LocalSettlement {
        execution_id: record.request.execution_id.clone(),
        before: None,
        after: Ok(ExecutionOutcome::Completed),
        local_outcome: Some(ExecutionOutcome::Completed),
    });
    SessionSaveUnit::new(changes).unwrap()
}
fn filler(turn: usize, length: usize, label: &str) -> String {
    const WORDS: [&str; 12] = [
        "gateway",
        "transcript",
        "receiver",
        "epoch",
        "catalogue",
        "device",
        "sync",
        "record",
        "page",
        "fold",
        "cache",
        "turn",
    ];
    let mut text = format!("{label} {turn}:");
    let mut index = turn;
    while text.len() < length {
        text.push(' ');
        text.push_str(WORDS[index % WORDS.len()]);
        index = index.wrapping_mul(31).wrapping_add(7);
    }
    text.truncate(length);
    text
}
