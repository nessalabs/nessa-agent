//! Substitutes for what managing the stored servers reads from outside — the
//! configuration file and its lock, the audit, the clock, and the server an
//! inspection starts — each able to fail, and the settings built over them.
use crate::mcp_servers::application::{
    AuditUnavailable, InspectBounds, InspectCut, InspectFailure, InspectFuture, InspectStop,
    Inspection, LiveServerSet, McpServerAudit, McpServerAuditPhase, McpServerAuditRecord,
    McpServerInitiator, McpServerSettings, ServerInspector, StoreLock,
};
use crate::mcp_servers::domain::{
    ConfigurationKey, ConfiguredMcpServer, StdioServer, MANAGED_SERVER_NAME,
};
use crate::mcp_servers::infrastructure::{
    ConfigCheck, ConfigFiles, ConfigJsonStore, LaunchSettings, LiveMcpServers,
};
use nessa_sdk::infrastructure::{
    clock::{Clock, ClockInstant, ClockSleep, RuntimeClock},
    mcp::McpServers,
};
use serde_json::{json, Map, Value};
use std::{
    collections::BTreeMap,
    io,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use tokio::sync::{watch, Semaphore};

/// What the check refuses: a configuration holding this is not one the
/// gateway starts with.
pub(crate) const UNPARSEABLE: &str = "refused-by-the-runtime-configuration";

/// `config.json` in memory, with its lock, publishing that can be made to
/// fail or held, reads counted, and an edit made outside the lock once the
/// next read has been answered.
#[derive(Default)]
pub(crate) struct MemoryFiles {
    pub(crate) bytes: Mutex<Option<Vec<u8>>>,
    pub(crate) held: Arc<AtomicBool>,
    pub(crate) fail_publish: AtomicBool,
    pub(crate) publishes: AtomicUsize,
    pub(crate) locks: AtomicUsize,
    /// How many reads have been made.
    pub(crate) reads: AtomicUsize,
    /// The file another writer, ignoring the lock, leaves just after the
    /// next read: taken by that read.
    pub(crate) after_next_read: Mutex<Option<Vec<u8>>>,
    /// Set as a publish starts; a publish then waits for a message on
    /// `publish_gate`, when one is given.
    pub(crate) publishing: AtomicBool,
    pub(crate) publish_gate: Mutex<Option<std::sync::mpsc::Receiver<()>>>,
}
impl MemoryFiles {
    pub(crate) fn holding(document: Value) -> Arc<Self> {
        let files = Self::default();
        *files.bytes.lock().unwrap() = Some(serde_json::to_vec(&document).unwrap());
        Arc::new(files)
    }
    pub(crate) fn raw(bytes: &[u8]) -> Arc<Self> {
        let files = Self::default();
        *files.bytes.lock().unwrap() = Some(bytes.to_vec());
        Arc::new(files)
    }
    pub(crate) fn document(&self) -> Value {
        serde_json::from_slice(self.bytes.lock().unwrap().as_ref().unwrap()).unwrap()
    }
    pub(crate) fn current(&self) -> Option<Vec<u8>> {
        self.bytes.lock().unwrap().clone()
    }
}

struct Held(Arc<AtomicBool>);
impl Drop for Held {
    fn drop(&mut self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

impl ConfigFiles for MemoryFiles {
    fn read(&self, limit: usize) -> io::Result<Option<Vec<u8>>> {
        self.reads.fetch_add(1, Ordering::SeqCst);
        let mut bytes = self.bytes.lock().unwrap();
        let read = bytes
            .as_ref()
            .map(|bytes| bytes[..bytes.len().min(limit + 1)].to_vec());
        if let Some(edited) = self.after_next_read.lock().unwrap().take() {
            *bytes = Some(edited);
        }
        Ok(read)
    }
    fn publish(&self, bytes: &[u8]) -> io::Result<()> {
        self.publishing.store(true, Ordering::SeqCst);
        let gate = self.publish_gate.lock().unwrap().take();
        if let Some(gate) = gate {
            let _ = gate.recv();
        }
        if self.fail_publish.load(Ordering::SeqCst) {
            return Err(io::Error::other("publish refused"));
        }
        self.publishes.fetch_add(1, Ordering::SeqCst);
        *self.bytes.lock().unwrap() = Some(bytes.to_vec());
        Ok(())
    }
    fn try_lock(&self) -> io::Result<Option<StoreLock>> {
        if self.held.swap(true, Ordering::SeqCst) {
            return Ok(None);
        }
        self.locks.fetch_add(1, Ordering::SeqCst);
        Ok(Some(Box::new(Held(self.held.clone()))))
    }
}

/// Every record, and either phase made to fail.
#[derive(Default)]
pub(crate) struct RecordingAudit {
    pub(crate) records: Mutex<Vec<McpServerAuditRecord>>,
    pub(crate) fail_requested: AtomicBool,
    pub(crate) fail_outcome: AtomicBool,
}
impl RecordingAudit {
    pub(crate) fn records(&self) -> Vec<McpServerAuditRecord> {
        self.records.lock().unwrap().clone()
    }
}
impl McpServerAudit for RecordingAudit {
    fn record(&self, record: &McpServerAuditRecord) -> Result<(), AuditUnavailable> {
        let fail = match record.phase {
            McpServerAuditPhase::Requested => &self.fail_requested,
            McpServerAuditPhase::Outcome(_) => &self.fail_outcome,
        };
        if fail.load(Ordering::SeqCst) {
            return Err(AuditUnavailable);
        }
        self.records.lock().unwrap().push(record.clone());
        Ok(())
    }
}

/// A clock that is wherever the last wait asked it to be: every wait ends at
/// once, so a bound is reached without waiting for it.
#[derive(Default)]
pub(crate) struct LeapingClock(Mutex<Duration>);
impl Clock for LeapingClock {
    fn now(&self) -> ClockInstant {
        ClockInstant::from_origin(*self.0.lock().unwrap())
    }
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep {
        let mut now = self.0.lock().unwrap();
        *now = (*now).max(deadline.since_origin());
        Box::pin(async {})
    }
}

/// A clock that moves only when told to, and whose sleeps end once it has
/// moved past them.
pub(crate) struct ManualClock(watch::Sender<Duration>);
impl Default for ManualClock {
    fn default() -> Self {
        Self(watch::channel(Duration::ZERO).0)
    }
}
impl ManualClock {
    pub(crate) fn advance(&self, by: Duration) {
        self.0.send_modify(|now| *now += by);
    }
}
impl Clock for ManualClock {
    fn now(&self) -> ClockInstant {
        ClockInstant::from_origin(*self.0.borrow())
    }
    fn sleep_until(&self, deadline: ClockInstant) -> ClockSleep {
        let mut now = self.0.subscribe();
        Box::pin(async move {
            let _ = now.wait_for(|now| *now >= deadline.since_origin()).await;
        })
    }
}

/// An inspector that answers what it is given, records the servers and
/// bounds it was asked with, and — while `gate` holds no permit — waits for
/// one before answering. It observes the stop as the real one does: given
/// before it starts, `stopping`; while it waits, cut `stopping`.
pub(crate) struct ScriptedInspector {
    pub(crate) answer: Mutex<Result<Inspection, InspectFailure>>,
    pub(crate) asked: Mutex<Vec<(ConfiguredMcpServer, InspectBounds)>>,
    pub(crate) gate: Arc<Semaphore>,
}
impl Default for ScriptedInspector {
    fn default() -> Self {
        Self {
            answer: Mutex::new(Ok(Inspection {
                tools: Vec::new(),
                cut: None,
            })),
            asked: Mutex::default(),
            gate: Arc::new(Semaphore::new(Semaphore::MAX_PERMITS)),
        }
    }
}
impl ServerInspector for ScriptedInspector {
    fn inspect(
        &self,
        server: &ConfiguredMcpServer,
        bounds: InspectBounds,
        mut stop: InspectStop,
    ) -> InspectFuture<'_> {
        self.asked.lock().unwrap().push((server.clone(), bounds));
        Box::pin(async move {
            if stop.given() {
                return Err(InspectFailure::Stopping);
            }
            tokio::select! {
                passed = self.gate.acquire() => {
                    let _passed = passed.unwrap();
                    self.answer.lock().unwrap().clone()
                }
                () = stop.wait() => Ok(Inspection {
                    tools: Vec::new(),
                    cut: Some(InspectCut::Stopping),
                }),
            }
        })
    }
}

pub(crate) fn server(name: &str) -> StdioServer {
    StdioServer {
        name: name.into(),
        command: PathBuf::from("/usr/bin/python3"),
        args: vec![format!("/{name}.py")],
    }
}

pub(crate) fn managed() -> ConfiguredMcpServer {
    ConfiguredMcpServer {
        server: StdioServer {
            name: MANAGED_SERVER_NAME.into(),
            command: PathBuf::from("/bundle/nessa-mcp"),
            args: vec!["--workspace".into(), "/w".into()],
        },
        enabled: true,
        env: BTreeMap::new(),
    }
}

/// A stored entry as `config.json` has it.
pub(crate) fn entry(name: &str) -> Value {
    json!({"name": name, "command": "/usr/bin/python3", "args": [format!("/{name}.py")]})
}

/// A configuration whose `agents.mcpServers` is `servers`.
pub(crate) fn config(servers: Vec<Value>) -> Value {
    json!({
        "session": {"writeTimeoutMs": 75},
        "agents": {"catalog": "/models.json", "workspace": "/w", "mcpServers": servers},
    })
}

pub(crate) fn initiator() -> McpServerInitiator {
    McpServerInitiator {
        organization_id: "organization".into(),
        principal_id: "principal".into(),
        credential_id: "credential".into(),
    }
}

/// The key the test settings' revisions are keyed with.
pub(crate) fn key() -> ConfigurationKey {
    ConfigurationKey::new([7; 32])
}

/// The settings over `files` and `audit`, with the managed server, and the
/// live set they replace (holding what `files` stores at the start).
pub(crate) fn settings_over(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
    clock: Arc<dyn Clock>,
) -> (McpServerSettings, McpServers) {
    inspected_over(files, audit, clock, Arc::new(ScriptedInspector::default()))
}

/// [`settings_over`], inspecting with `inspector`.
pub(crate) fn inspected_over(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
    clock: Arc<dyn Clock>,
    inspector: Arc<dyn ServerInspector>,
) -> (McpServerSettings, McpServers) {
    started_with(files, audit, clock, inspector, &[managed()])
}

/// The settings of a gateway that started with `startup` configured — the
/// managed server among them, or not — over `files`, on a leaping clock.
pub(crate) fn settings_started_with(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
    startup: &[ConfiguredMcpServer],
) -> (McpServerSettings, McpServers) {
    started_with(
        files,
        audit,
        Arc::new(LeapingClock::default()),
        Arc::new(ScriptedInspector::default()),
        startup,
    )
}

fn started_with(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
    clock: Arc<dyn Clock>,
    inspector: Arc<dyn ServerInspector>,
    startup: &[ConfiguredMcpServer],
) -> (McpServerSettings, McpServers) {
    live_through(files, audit, clock, inspector, startup, |live| {
        Arc::new(live)
    })
}

/// [`settings_for`], inspecting with `inspector`, and with the live set
/// seen through `live` — wrapped, so it can be made to fail.
pub(crate) fn settings_through(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
    inspector: Arc<dyn ServerInspector>,
    live: impl FnOnce(LiveMcpServers) -> Arc<dyn LiveServerSet>,
) -> (McpServerSettings, McpServers) {
    live_through(
        files,
        audit,
        Arc::new(LeapingClock::default()),
        inspector,
        &[managed()],
        live,
    )
}

fn live_through(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
    clock: Arc<dyn Clock>,
    inspector: Arc<dyn ServerInspector>,
    startup: &[ConfiguredMcpServer],
    live: impl FnOnce(LiveMcpServers) -> Arc<dyn LiveServerSet>,
) -> (McpServerSettings, McpServers) {
    let store = ConfigJsonStore::new(
        files.clone(),
        ConfigCheck {
            limit: 4096,
            parses: Box::new(|bytes| {
                serde_json::from_slice::<Value>(bytes).is_ok()
                    && !String::from_utf8_lossy(bytes).contains(UNPARSEABLE)
            }),
        },
        Map::from_iter([
            ("catalog".to_owned(), json!("/models.json")),
            ("workspace".to_owned(), json!("/w")),
        ]),
        clock,
        key(),
    );
    let launches = LaunchSettings::new(startup, std::env::temp_dir(), BTreeMap::new());
    let servers = McpServers::new(
        launches.launch_set(&[]).unwrap(),
        Arc::new(RuntimeClock::new()),
    )
    .unwrap();
    let settings = McpServerSettings::new(
        Arc::new(store),
        audit,
        live(LiveMcpServers::new(servers.clone(), launches)),
        inspector,
    );
    (settings, servers)
}

/// [`settings_over`] on a clock that leaps to each wait's end.
pub(crate) fn settings_for(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
) -> (McpServerSettings, McpServers) {
    settings_over(files, audit, Arc::new(LeapingClock::default()))
}

/// The names in the live set, in order.
pub(crate) fn live(servers: &McpServers) -> Vec<String> {
    servers
        .configured()
        .into_iter()
        .map(|launch| launch.server.name)
        .collect()
}
