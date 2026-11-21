//! The person's terminals on this gateway's machine (issue #1398,
//! `docs/design/terminals.md`).
//!
//! A terminal is a PTY session pty-runtime owns, read through Ghostty's
//! terminal model, so the gateway renders the screen and every view paints
//! the same rows. The hub keeps the list, the owner of each terminal, and
//! the rules for several views of one terminal
//! (`docs/state/services/desktop/workspace/share-a-terminal-between-views.md`):
//!
//! - one size owner, the view last focused; only it resizes the terminal,
//!   and when it goes the size is kept;
//! - each view gets a checkpoint of the screen, then frames in order, on its
//!   own connection's queue;
//! - history reads are per view.
//!
//! ```text
//! window ── terminal.focus {view, cols, rows} ──▶ TerminalHub ── resize? ──▶ engine thread
//! window ◀── terminal.output {view, frame} ◀── TerminalLink queue ◀────────── engine thread
//! ```
//!
//! The engine is a seam: the Ghostty one is built with the `terminals`
//! feature on Unix; without it the hub refuses every open as unavailable.
#[cfg(all(unix, feature = "terminals"))]
mod engine;
#[cfg(all(unix, feature = "terminals"))]
mod wire;

#[cfg(all(unix, feature = "terminals"))]
pub use engine::GhosttyEngine;

#[cfg(all(unix, feature = "terminals"))]
static PREPARED: std::sync::OnceLock<Arc<pty_runtime::Runtime>> = std::sync::OnceLock::new();

/// Why [`prepare`] made no runtime. It runs before logging starts, so the
/// reason is kept here and logged by whoever composes the gateway.
static UNPREPARED: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Build pty-runtime's runtime for this process. pty-runtime stages its
/// helper image by forking, so this runs on the main thread before any
/// async runtime starts, and only in a process that serves the gateway.
/// A runtime that cannot be built leaves terminals unavailable; the gateway
/// still starts.
pub fn prepare() {
    #[cfg(all(unix, feature = "terminals"))]
    {
        let options = pty_runtime::RuntimeOptions {
            max_sessions: 64,
            replay_bytes: 1024 * 1024,
            ..Default::default()
        };
        // Folders are checked by the gateway before a spawn (a channel's,
        // or an agent's); the runtime's own check only needs a root that
        // holds them all.
        match pty_runtime::Runtime::new(vec![PathBuf::from("/")], options) {
            Ok(runtime) => {
                let _ = PREPARED.set(Arc::new(runtime));
            }
            Err(error) => {
                let _ = UNPREPARED.set(format!("{error:?}"));
            }
        }
    }
}

/// Why [`prepare`] made no runtime, when it tried and failed.
pub fn unprepared_reason() -> Option<&'static str> {
    UNPREPARED.get().map(String::as_str)
}

/// The engine [`prepare`] made, when it made one.
pub fn prepared_engine() -> Option<Arc<dyn TerminalEngine>> {
    #[cfg(all(unix, feature = "terminals"))]
    {
        PREPARED.get().map(|runtime| {
            Arc::new(GhosttyEngine::new(Arc::clone(runtime))) as Arc<dyn TerminalEngine>
        })
    }
    #[cfg(not(all(unix, feature = "terminals")))]
    {
        None
    }
}

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

/// Terminals one person may hold at once, running or ended.
pub const MAX_TERMINALS_PER_OWNER: usize = 32;
/// Views a terminal may have at once, across every connection.
pub const MAX_VIEWS_PER_TERMINAL: usize = 16;
/// Messages waiting for one connection. A full queue drops nothing that
/// matters: the view it was for is redrawn whole on the next frame.
pub const CONNECTION_QUEUE: usize = 512;

/// Who a terminal belongs to: the organization and person that opened it.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct TerminalOwner {
    pub organization: String,
    pub principal: String,
}

/// A grid size, in cells.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct GridSize {
    pub cols: u16,
    pub rows: u16,
}

impl GridSize {
    /// The size, when the wire's numbers name one pty-runtime accepts.
    pub fn new(cols: u64, rows: u64) -> Option<Self> {
        let cols = u16::try_from(cols)
            .ok()
            .filter(|c| (2..=1000).contains(c))?;
        let rows = u16::try_from(rows)
            .ok()
            .filter(|r| (1..=1000).contains(r))?;
        (u32::from(cols) * u32::from(rows) <= 1_048_576).then_some(Self { cols, rows })
    }
}

/// What a terminal was started for.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalKind {
    /// A shell, from ⌘J.
    Shell,
    /// `command`, from Run in a reply, given to the login shell with `-c`.
    Run { command: String },
}

/// What to start, once the place is resolved.
#[derive(Clone, Debug)]
pub struct StartSpec {
    pub folder: PathBuf,
    pub shell: PathBuf,
    pub kind: TerminalKind,
    pub size: GridSize,
}

/// One terminal as the list shows it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TerminalEntry {
    pub id: String,
    pub request_id: String,
    pub title: String,
    pub kind: TerminalKind,
    pub channel_id: Option<String>,
    pub conversation_id: Option<String>,
    pub folder: String,
    pub shell: String,
    pub started_at_ms: u64,
    pub ended: Option<TerminalEnd>,
}

/// How a terminal's program ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct TerminalEnd {
    /// The exit code; `None` when a signal ended it.
    pub status: Option<i32>,
    pub at_ms: u64,
}

/// Why the hub refused.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalError {
    /// No terminal engine in this build, or it could not start.
    Unavailable,
    /// No such terminal of the caller's.
    NotFound,
    /// The caller holds as many as they may, or the terminal as many views.
    Limit,
    /// The request named nothing that can be run.
    Invalid,
    /// The program could not be started.
    Spawn(String),
}

/// A message for one view, as pty-runtime's client protocol JSON text.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalSignal {
    Output {
        terminal_id: String,
        view_id: String,
        message: String,
    },
    /// The owner's list changed.
    Listed,
}

/// One view of a terminal: the connection it is on and the window's id.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct ViewKey {
    pub connection: u64,
    pub view: String,
}

/// What the engine thread of one terminal is asked to do.
pub enum EngineCommand {
    /// Start showing this view: a checkpoint, then live frames, on `out`.
    Attach {
        key: ViewKey,
        out: mpsc::Sender<TerminalSignal>,
    },
    Detach {
        key: ViewKey,
    },
    Input(Vec<u8>),
    Resize(GridSize),
    History {
        key: ViewKey,
        start: u64,
        count: u16,
    },
    /// Hang up the program, kill it after 5 s, and end the thread.
    Close,
}

/// What an engine tells the hub about a terminal it runs.
pub trait EngineEvents: Send + Sync {
    /// The program ended.
    fn exited(&self, terminal: &str, status: Option<i32>);
}

/// A running terminal's engine side.
pub trait RunningTerminal: Send + Sync {
    /// Ask its thread; a thread that has gone ignores it.
    fn send(&self, command: EngineCommand);
}

/// What starts terminals. The Ghostty engine in a build with terminals.
pub trait TerminalEngine: Send + Sync {
    fn start(
        &self,
        id: &str,
        spec: &StartSpec,
        events: Arc<dyn EngineEvents>,
    ) -> Result<Box<dyn RunningTerminal>, TerminalError>;
}

/// Milliseconds since the epoch, injected so tests can fix it.
pub trait TerminalClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The system clock.
pub struct SystemTerminalClock;

impl TerminalClock for SystemTerminalClock {
    fn now_ms(&self) -> u64 {
        crate::timer::clock::wall_now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }
}

struct Live {
    owner: TerminalOwner,
    entry: TerminalEntry,
    running: Box<dyn RunningTerminal>,
    /// Each attached view's fit.
    views: HashMap<ViewKey, GridSize>,
    /// The view whose fit the terminal has; `None` once it went.
    size_owner: Option<ViewKey>,
    size: GridSize,
}

#[derive(Default)]
struct Hub {
    terminals: Vec<Live>,
    /// Connections that asked for the list, by connection id.
    links: HashMap<u64, (TerminalOwner, mpsc::Sender<TerminalSignal>)>,
}

/// The person's terminals on this gateway.
pub struct TerminalHub {
    engine: Option<Arc<dyn TerminalEngine>>,
    clock: Arc<dyn TerminalClock>,
    hub: Mutex<Hub>,
    next_connection: AtomicU64,
    /// Ticks whenever any list changes, for tests and diagnostics.
    changes: watch::Sender<u64>,
    me: std::sync::Weak<TerminalHub>,
}

struct HubEvents(std::sync::Weak<TerminalHub>);

impl EngineEvents for HubEvents {
    fn exited(&self, terminal: &str, status: Option<i32>) {
        if let Some(hub) = self.0.upgrade() {
            hub.ended(terminal, status);
        }
    }
}

/// What `open` is asked for, once the caller's place is resolved.
#[derive(Clone, Debug)]
pub struct OpenRequest {
    pub owner: TerminalOwner,
    pub request_id: String,
    pub channel_id: Option<String>,
    pub conversation_id: Option<String>,
    pub folder: PathBuf,
    pub kind: TerminalKind,
    pub size: GridSize,
}

impl TerminalHub {
    /// A hub that starts terminals with `engine`; `None` refuses every open.
    pub fn new(
        engine: Option<Arc<dyn TerminalEngine>>,
        clock: Arc<dyn TerminalClock>,
    ) -> Arc<Self> {
        Arc::new_cyclic(|me| Self {
            engine,
            clock,
            hub: Mutex::new(Hub::default()),
            next_connection: AtomicU64::new(1),
            changes: watch::channel(0).0,
            me: me.clone(),
        })
    }

    /// Whether this build can start a terminal at all.
    pub fn available(&self) -> bool {
        self.engine.is_some()
    }

    /// A change counter, bumped on every list change.
    pub fn changes(&self) -> watch::Receiver<u64> {
        self.changes.subscribe()
    }

    /// A place for one connection: what its views are sent through.
    pub fn connect(&self, owner: TerminalOwner) -> (TerminalLink, mpsc::Receiver<TerminalSignal>) {
        let (out, receiver) = mpsc::channel(CONNECTION_QUEUE);
        let link = TerminalLink {
            hub: self.me.clone(),
            connection: self.next_connection.fetch_add(1, Ordering::Relaxed),
            owner,
            out,
            listening: AtomicBool::new(false),
        };
        (link, receiver)
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Hub> {
        self.hub
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The owner's terminals, oldest first.
    pub fn list(&self, owner: &TerminalOwner) -> Vec<TerminalEntry> {
        self.lock()
            .terminals
            .iter()
            .filter(|live| &live.owner == owner)
            .map(|live| live.entry.clone())
            .collect()
    }

    /// Start a terminal, or answer the one `request.request_id` started.
    pub fn open(
        &self,
        request: OpenRequest,
        shell: PathBuf,
    ) -> Result<TerminalEntry, TerminalError> {
        let engine = self.engine.clone().ok_or(TerminalError::Unavailable)?;
        {
            let hub = self.lock();
            if let Some(live) = hub.terminals.iter().find(|live| {
                live.owner == request.owner && live.entry.request_id == request.request_id
            }) {
                return Ok(live.entry.clone());
            }
            if hub
                .terminals
                .iter()
                .filter(|live| live.owner == request.owner)
                .count()
                >= MAX_TERMINALS_PER_OWNER
            {
                return Err(TerminalError::Limit);
            }
        }
        let id = format!("t_{}", uuid::Uuid::new_v4().simple());
        let spec = StartSpec {
            folder: request.folder.clone(),
            shell: shell.clone(),
            kind: request.kind.clone(),
            size: request.size,
        };
        let events: Arc<dyn EngineEvents> = Arc::new(HubEvents(self.me.clone()));
        let running = engine.start(&id, &spec, events)?;
        let shell_name = shell
            .file_name()
            .and_then(|name| name.to_str())
            .unwrap_or("shell")
            .to_owned();
        let title = match &request.kind {
            TerminalKind::Shell => shell_name,
            TerminalKind::Run { command } => run_title(command),
        };
        let entry = TerminalEntry {
            id,
            request_id: request.request_id,
            title,
            kind: request.kind,
            channel_id: request.channel_id,
            conversation_id: request.conversation_id,
            folder: request.folder.to_string_lossy().into_owned(),
            shell: shell.to_string_lossy().into_owned(),
            started_at_ms: self.clock.now_ms(),
            ended: None,
        };
        let mut hub = self.lock();
        // A retry that raced this one: keep the first, end this one.
        if let Some(live) = hub
            .terminals
            .iter()
            .find(|live| live.owner == request.owner && live.entry.request_id == entry.request_id)
        {
            running.send(EngineCommand::Close);
            return Ok(live.entry.clone());
        }
        hub.terminals.push(Live {
            owner: request.owner.clone(),
            entry: entry.clone(),
            running,
            views: HashMap::new(),
            size_owner: None,
            size: request.size,
        });
        self.announce(&mut hub, &request.owner);
        Ok(entry)
    }

    fn ended(&self, terminal: &str, status: Option<i32>) {
        let at_ms = self.clock.now_ms();
        let mut hub = self.lock();
        let Some(live) = hub
            .terminals
            .iter_mut()
            .find(|live| live.entry.id == terminal)
        else {
            return;
        };
        live.entry.ended = Some(TerminalEnd { status, at_ms });
        let owner = live.owner.clone();
        self.announce(&mut hub, &owner);
    }

    /// Tell each listening connection of `owner` that the list changed.
    fn announce(&self, hub: &mut Hub, owner: &TerminalOwner) {
        self.changes.send_modify(|tick| *tick += 1);
        for (listener, out) in hub.links.values() {
            if listener == owner {
                // A full queue already holds a Listed, or soon reads one.
                let _ = out.try_send(TerminalSignal::Listed);
            }
        }
    }

    fn with_terminal<T>(
        &self,
        owner: &TerminalOwner,
        id: &str,
        act: impl FnOnce(&mut Live) -> Result<T, TerminalError>,
    ) -> Result<T, TerminalError> {
        let mut hub = self.lock();
        let live = hub
            .terminals
            .iter_mut()
            .find(|live| live.entry.id == id && &live.owner == owner)
            .ok_or(TerminalError::NotFound)?;
        act(live)
    }

    fn attach(
        &self,
        link: &TerminalLink,
        id: &str,
        view: &str,
        fit: GridSize,
    ) -> Result<(), TerminalError> {
        let key = link.key(view);
        self.with_terminal(&link.owner, id, |live| {
            if !live.views.contains_key(&key) && live.views.len() >= MAX_VIEWS_PER_TERMINAL {
                return Err(TerminalError::Limit);
            }
            live.views.insert(key.clone(), fit);
            live.running.send(EngineCommand::Attach {
                key,
                out: link.out.clone(),
            });
            Ok(())
        })
    }

    /// The view becomes the size owner; the terminal takes its fit.
    fn focus(
        &self,
        link: &TerminalLink,
        id: &str,
        view: &str,
        fit: GridSize,
    ) -> Result<(), TerminalError> {
        let key = link.key(view);
        self.with_terminal(&link.owner, id, |live| {
            if !live.views.contains_key(&key) {
                return Err(TerminalError::NotFound);
            }
            live.views.insert(key.clone(), fit);
            live.size_owner = Some(key);
            if live.size != fit {
                live.size = fit;
                live.running.send(EngineCommand::Resize(fit));
            }
            Ok(())
        })
    }

    /// The view's box changed; only the size owner's change resizes.
    fn fit(
        &self,
        link: &TerminalLink,
        id: &str,
        view: &str,
        fit: GridSize,
    ) -> Result<(), TerminalError> {
        let key = link.key(view);
        self.with_terminal(&link.owner, id, |live| {
            if !live.views.contains_key(&key) {
                return Err(TerminalError::NotFound);
            }
            live.views.insert(key.clone(), fit);
            if live.size_owner.as_ref() == Some(&key) && live.size != fit {
                live.size = fit;
                live.running.send(EngineCommand::Resize(fit));
            }
            Ok(())
        })
    }

    fn detach_key(live: &mut Live, key: &ViewKey) {
        if live.views.remove(key).is_some() {
            if live.size_owner.as_ref() == Some(key) {
                live.size_owner = None;
            }
            live.running
                .send(EngineCommand::Detach { key: key.clone() });
        }
    }

    fn detach(&self, link: &TerminalLink, id: &str, view: &str) -> Result<(), TerminalError> {
        let key = link.key(view);
        self.with_terminal(&link.owner, id, |live| {
            Self::detach_key(live, &key);
            Ok(())
        })
    }

    fn input(&self, owner: &TerminalOwner, id: &str, bytes: Vec<u8>) -> Result<(), TerminalError> {
        self.with_terminal(owner, id, |live| {
            if live.entry.ended.is_some() {
                return Err(TerminalError::NotFound);
            }
            live.running.send(EngineCommand::Input(bytes));
            Ok(())
        })
    }

    fn history(
        &self,
        link: &TerminalLink,
        id: &str,
        view: &str,
        start: u64,
        count: u16,
    ) -> Result<(), TerminalError> {
        let key = link.key(view);
        self.with_terminal(&link.owner, id, |live| {
            if !live.views.contains_key(&key) {
                return Err(TerminalError::NotFound);
            }
            live.running
                .send(EngineCommand::History { key, start, count });
            Ok(())
        })
    }

    /// End `id` and take it off the list.
    pub fn close(&self, owner: &TerminalOwner, id: &str) -> Result<(), TerminalError> {
        let mut hub = self.lock();
        let index = hub
            .terminals
            .iter()
            .position(|live| live.entry.id == id && &live.owner == owner)
            .ok_or(TerminalError::NotFound)?;
        let live = hub.terminals.remove(index);
        live.running.send(EngineCommand::Close);
        self.announce(&mut hub, owner);
        Ok(())
    }

    /// A connection went: every view it had goes with it.
    fn disconnect(&self, connection: u64) {
        let mut hub = self.lock();
        hub.links.remove(&connection);
        for live in &mut hub.terminals {
            let keys: Vec<ViewKey> = live
                .views
                .keys()
                .filter(|key| key.connection == connection)
                .cloned()
                .collect();
            for key in keys {
                Self::detach_key(live, &key);
            }
        }
    }

    /// The size owner and size of `id`, for tests and diagnostics.
    pub fn size_of(&self, owner: &TerminalOwner, id: &str) -> Option<(Option<ViewKey>, GridSize)> {
        self.with_terminal(owner, id, |live| Ok((live.size_owner.clone(), live.size)))
            .ok()
    }
}

/// The title of a Run: its first non-blank line, cut to 80 characters.
fn run_title(command: &str) -> String {
    let line = command
        .lines()
        .map(str::trim)
        .find(|line| !line.is_empty())
        .unwrap_or("run");
    let mut title: String = line.chars().take(80).collect();
    if line.chars().count() > 80 {
        title.push('…');
    }
    title
}

/// One connection's place in the hub. Dropped with the socket, it takes
/// every view of that connection with it.
pub struct TerminalLink {
    hub: std::sync::Weak<TerminalHub>,
    connection: u64,
    owner: TerminalOwner,
    out: mpsc::Sender<TerminalSignal>,
    listening: AtomicBool,
}

impl TerminalLink {
    fn key(&self, view: &str) -> ViewKey {
        ViewKey {
            connection: self.connection,
            view: view.to_owned(),
        }
    }

    fn hub(&self) -> Result<Arc<TerminalHub>, TerminalError> {
        self.hub.upgrade().ok_or(TerminalError::Unavailable)
    }

    pub fn owner(&self) -> &TerminalOwner {
        &self.owner
    }

    /// The owner's terminals; from now on this connection is told of changes.
    pub fn list(&self) -> Result<Vec<TerminalEntry>, TerminalError> {
        let hub = self.hub()?;
        if !self.listening.swap(true, Ordering::AcqRel) {
            hub.lock()
                .links
                .insert(self.connection, (self.owner.clone(), self.out.clone()));
        }
        Ok(hub.list(&self.owner))
    }

    pub fn open(
        &self,
        request: OpenRequest,
        shell: PathBuf,
    ) -> Result<TerminalEntry, TerminalError> {
        self.hub()?.open(request, shell)
    }

    pub fn attach(&self, id: &str, view: &str, fit: GridSize) -> Result<(), TerminalError> {
        self.hub()?.attach(self, id, view, fit)
    }

    pub fn focus(&self, id: &str, view: &str, fit: GridSize) -> Result<(), TerminalError> {
        self.hub()?.focus(self, id, view, fit)
    }

    pub fn fit(&self, id: &str, view: &str, fit: GridSize) -> Result<(), TerminalError> {
        self.hub()?.fit(self, id, view, fit)
    }

    pub fn detach(&self, id: &str, view: &str) -> Result<(), TerminalError> {
        self.hub()?.detach(self, id, view)
    }

    pub fn input(&self, id: &str, bytes: Vec<u8>) -> Result<(), TerminalError> {
        self.hub()?.input(&self.owner, id, bytes)
    }

    pub fn history(
        &self,
        id: &str,
        view: &str,
        start: u64,
        count: u16,
    ) -> Result<(), TerminalError> {
        self.hub()?.history(self, id, view, start, count)
    }

    pub fn close(&self, id: &str) -> Result<(), TerminalError> {
        self.hub()?.close(&self.owner, id)
    }
}

impl Drop for TerminalLink {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.disconnect(self.connection);
        }
    }
}

#[cfg(test)]
mod tests;
