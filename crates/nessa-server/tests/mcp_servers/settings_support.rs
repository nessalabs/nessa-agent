//! Substitutes for what managing the stored servers reads from outside — the
//! configuration file and its lock, the audit, and the clock — each able to
//! fail, and the settings built over them.
use crate::mcp_servers::application::{
    AuditUnavailable, McpServerAudit, McpServerAuditPhase, McpServerAuditRecord,
    McpServerInitiator, McpServerSettings, StoreLock,
};
use crate::mcp_servers::domain::{ConfiguredMcpServer, StdioServer, MANAGED_SERVER_NAME};
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

/// What the check refuses: a configuration holding this is not one the
/// gateway starts with.
pub(crate) const UNPARSEABLE: &str = "refused-by-the-runtime-configuration";

/// `config.json` in memory, with its lock, and publishing that can be made
/// to fail.
#[derive(Default)]
pub(crate) struct MemoryFiles {
    pub(crate) bytes: Mutex<Option<Vec<u8>>>,
    pub(crate) held: Arc<AtomicBool>,
    pub(crate) fail_publish: AtomicBool,
    pub(crate) publishes: AtomicUsize,
    pub(crate) locks: AtomicUsize,
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
        Ok(self
            .bytes
            .lock()
            .unwrap()
            .as_ref()
            .map(|bytes| bytes[..bytes.len().min(limit + 1)].to_vec()))
    }
    fn publish(&self, bytes: &[u8]) -> io::Result<()> {
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

/// The settings over `files` and `audit`, with the managed server, and the
/// live set they replace (holding what `files` stores at the start).
pub(crate) fn settings_over(
    files: Arc<MemoryFiles>,
    audit: Arc<RecordingAudit>,
    clock: Arc<dyn Clock>,
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
    );
    let launches = LaunchSettings::new(&[managed()], std::env::temp_dir(), BTreeMap::new());
    let servers = McpServers::new(
        launches.launch_set(&[]).unwrap(),
        Arc::new(RuntimeClock::new()),
    )
    .unwrap();
    let settings = McpServerSettings::new(
        Arc::new(store),
        audit,
        Arc::new(LiveMcpServers::new(servers.clone(), launches)),
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
        .map(|server| server.name)
        .collect()
}
