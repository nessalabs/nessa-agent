//! The person's terminals and the rules for several views of one: who owns
//! the size, which views are attached, and who hears the list.
use super::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

mod resume;

/// The engine side of a terminal: reserved while its program starts, with
/// what it was asked meanwhile, then running.
enum Process {
    Starting(Vec<EngineCommand>),
    Started(Box<dyn RunningTerminal>),
}

impl Process {
    fn send(&mut self, command: EngineCommand) {
        match self {
            Self::Starting(waiting) => waiting.push(command),
            Self::Started(running) => running.send(command),
        }
    }
}

struct Live {
    owner: TerminalOwner,
    entry: TerminalEntry,
    /// The OSC 7577 record kept for it, while one is.
    saved: Option<SavedRecord>,
    running: Process,
    /// Each attached view's fit.
    views: HashMap<ViewKey, GridSize>,
    /// The view whose fit the terminal has; `None` once it went.
    size_owner: Option<ViewKey>,
    size: GridSize,
}

#[derive(Default)]
struct Hub {
    terminals: Vec<Live>,
    /// Connections that asked for the list, by connection id, with the
    /// counter that tells each one the list changed.
    links: HashMap<u64, (TerminalOwner, Arc<watch::Sender<u64>>)>,
}

/// What keeps programs across restarts (OSC 7577): where records and
/// settings are kept, and what reads the processes a report names.
pub struct ResumeParts {
    pub store: Arc<dyn ResumeStore>,
    pub processes: Arc<dyn Processes>,
}

struct Resume {
    store: Arc<dyn ResumeStore>,
    processes: Arc<dyn Processes>,
    settings: Mutex<ResumeSettings>,
    /// Set as the gateway stops: from then on nothing kept is forgotten,
    /// since its programs end because the gateway does, not because the
    /// person ended them.
    frozen: AtomicBool,
    /// Held across "is it still open" and the write that follows, so a
    /// terminal closed meanwhile never has a record written after its
    /// close forgot it.
    writing: Mutex<()>,
}

impl Resume {
    fn settings(&self) -> ResumeSettings {
        self.settings
            .lock()
            .map(|settings| settings.clone())
            .unwrap_or_default()
    }

    fn writing(&self) -> std::sync::MutexGuard<'_, ()> {
        self.writing
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn forget(&self, id: &str) {
        if let Err(error) = self.store.remove(id) {
            tracing::warn!(target: "nessa::terminal", terminal = %id, %error, "resume record not forgotten");
        }
    }
}

/// One kept program, as Settings lists it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SavedProgram {
    pub terminal_id: String,
    pub title: String,
    /// The command, escaped for display.
    pub command: String,
    pub folder: Option<String>,
    pub saved_at_ms: u64,
    /// Whether the next restart starts it, under the settings as they are.
    pub comes_back: bool,
    /// Whether its terminal is open now.
    pub open: bool,
}

/// Why resume settings were not kept.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ResumeSettingsError {
    /// This gateway keeps no records.
    Unavailable,
    Invalid(RuleError),
    Storage(String),
}

/// The person's terminals on this gateway.
pub struct TerminalHub {
    engine: Option<Arc<dyn TerminalEngine>>,
    clock: Arc<dyn TerminalClock>,
    resume: Option<Resume>,
    hub: Mutex<Hub>,
    next_connection: AtomicU64,
    /// Ticks whenever any list changes, for tests and diagnostics.
    changes: watch::Sender<u64>,
    me: std::sync::Weak<TerminalHub>,
}

struct HubEvents(std::sync::Weak<TerminalHub>);

impl EngineEvents for HubEvents {
    fn exited(&self, terminal: &str, status: Option<i32>, program_status: Option<ProgramStatus>) {
        if let Some(hub) = self.0.upgrade() {
            hub.ended(terminal, status, program_status);
        }
    }

    fn program_status(&self, terminal: &str, status: Option<ProgramStatus>) {
        if let Some(hub) = self.0.upgrade() {
            hub.program_status_changed(terminal, status);
        }
    }

    fn resume(&self, terminal: &str, change: ResumeChange) {
        if let Some(hub) = self.0.upgrade() {
            hub.resume_changed(terminal, change);
        }
    }

    fn processes(&self) -> Arc<dyn Processes> {
        self.0
            .upgrade()
            .and_then(|hub| hub.resume.as_ref().map(|resume| resume.processes.clone()))
            .unwrap_or_else(|| Arc::new(SystemProcesses))
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
    pub runs_on: String,
    pub kind: TerminalKind,
    pub size: GridSize,
    /// Set when a restart opens a kept terminal again.
    pub reopen: Option<Reopened>,
}

enum Reserved {
    Existing(TerminalEntry),
    New {
        entry: TerminalEntry,
        engine: Arc<dyn TerminalEngine>,
    },
}

impl TerminalHub {
    /// A hub that starts terminals with `engine`; `None` refuses every open.
    pub fn new(
        engine: Option<Arc<dyn TerminalEngine>>,
        clock: Arc<dyn TerminalClock>,
    ) -> Arc<Self> {
        Self::with_resume(engine, clock, None)
    }

    /// A hub that also keeps programs across restarts, in `resume`.
    pub fn with_resume(
        engine: Option<Arc<dyn TerminalEngine>>,
        clock: Arc<dyn TerminalClock>,
        resume: Option<ResumeParts>,
    ) -> Arc<Self> {
        let resume = resume.map(|parts| Resume {
            settings: Mutex::new(parts.store.settings()),
            store: parts.store,
            processes: parts.processes,
            frozen: AtomicBool::new(false),
            writing: Mutex::new(()),
        });
        Arc::new_cyclic(|me| Self {
            engine,
            clock,
            resume,
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
    pub fn connect(&self, owner: TerminalOwner) -> (TerminalLink, TerminalSignals) {
        let (out, output) = mpsc::channel(CONNECTION_QUEUE);
        let (listed, list_changes) = watch::channel(0);
        let link = TerminalLink {
            hub: self.me.clone(),
            connection: self.next_connection.fetch_add(1, Ordering::Relaxed),
            owner,
            out,
            listed: Arc::new(listed),
            listening: AtomicBool::new(false),
        };
        (
            link,
            TerminalSignals {
                output,
                listed: Some(list_changes),
            },
        )
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, Hub> {
        self.hub
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    /// The owner's terminals, oldest first; one still starting is not one
    /// yet.
    pub fn list(&self, owner: &TerminalOwner) -> Vec<TerminalEntry> {
        self.lock()
            .terminals
            .iter()
            .filter(|live| &live.owner == owner && matches!(live.running, Process::Started(_)))
            .map(|live| live.entry.clone())
            .collect()
    }

    /// How many terminals are open on this gateway, whoever opened them:
    /// one still starting counts, one whose program ended does not. Read by
    /// `server.activity`, which answers for the whole gateway.
    pub fn open_count(&self) -> usize {
        self.lock()
            .terminals
            .iter()
            .filter(|live| live.entry.ended.is_none())
            .count()
    }

    fn reserve(
        &self,
        request: &OpenRequest,
        shell_display: &str,
    ) -> Result<Reserved, TerminalError> {
        let engine = self.engine.clone().ok_or(TerminalError::Unavailable)?;
        let shell_name = std::path::Path::new(shell_display)
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| !name.is_empty())
            .unwrap_or("shell")
            .to_owned();
        let title = match (&request.reopen, &request.kind) {
            (Some(reopen), _) => reopen.title.clone(),
            (None, TerminalKind::Shell) => shell_name,
            (None, TerminalKind::Run { command }) => run_title(command),
        };
        let now = self.clock.now_ms();
        let entry = TerminalEntry {
            id: match &request.reopen {
                Some(reopen) => reopen.id.clone(),
                None => format!("t_{}", uuid::Uuid::new_v4().simple()),
            },
            request_id: request.request_id.clone(),
            title,
            kind: request.kind.clone(),
            channel_id: request.channel_id.clone(),
            conversation_id: request.conversation_id.clone(),
            folder: request.folder.to_string_lossy().into_owned(),
            shell: shell_display.to_owned(),
            runs_on: request.runs_on.clone(),
            started_at_ms: now,
            ended: None,
            program_status: None,
            resume_command: request
                .reopen
                .as_ref()
                .map(|reopen| display_command(&reopen.start.argv)),
            resumed_at_ms: request.reopen.as_ref().map(|_| now),
        };
        let mut hub = self.lock();
        if hub.terminals.iter().any(|live| live.entry.id == entry.id) {
            return Err(TerminalError::Invalid);
        }
        if let Some(live) = hub
            .terminals
            .iter()
            .find(|live| live.owner == request.owner && live.entry.request_id == request.request_id)
        {
            return Ok(Reserved::Existing(live.entry.clone()));
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
        hub.terminals.push(Live {
            owner: request.owner.clone(),
            entry: entry.clone(),
            // Installed before the engine starts, so a report the program
            // makes at once is never overwritten by this older record.
            saved: request.reopen.as_ref().map(|reopen| reopen.saved.clone()),
            running: Process::Starting(Vec::new()),
            views: HashMap::new(),
            size_owner: None,
            size: request.size,
        });
        Ok(Reserved::New { entry, engine })
    }

    fn commit(
        &self,
        owner: &TerminalOwner,
        id: &str,
        started: Result<Box<dyn RunningTerminal>, TerminalError>,
    ) -> Result<TerminalEntry, TerminalError> {
        let mut hub = self.lock();
        let index = hub.terminals.iter().position(|live| live.entry.id == id);
        let running = match started {
            Ok(running) => running,
            Err(error) => {
                // A retry may have been answered with this reservation; it
                // goes, and that retry's terminal is not found from now on.
                if let Some(index) = index {
                    hub.terminals.remove(index);
                }
                return Err(error);
            }
        };
        let Some(index) = index else {
            // Closed while it started: it is not one of theirs any more.
            running.send(EngineCommand::Close);
            return Err(TerminalError::NotFound);
        };
        let live = &mut hub.terminals[index];
        let Process::Starting(waiting) =
            std::mem::replace(&mut live.running, Process::Started(running))
        else {
            unreachable!("only this open starts its reservation");
        };
        for command in waiting {
            live.running.send(command);
        }
        let entry = live.entry.clone();
        self.announce(&mut hub, owner);
        Ok(entry)
    }

    /// Start a local terminal, or answer the one `request.request_id` started.
    ///
    /// The request is reserved, under the lock, before its program starts:
    /// a retry that races it answers the same terminal and starts nothing,
    /// and the limit counts it. What its views ask, and an exit the engine
    /// reports, while it starts are kept on the reservation.
    pub fn open(
        &self,
        request: OpenRequest,
        shell: PathBuf,
    ) -> Result<TerminalEntry, TerminalError> {
        let shell_display = shell.to_string_lossy().into_owned();
        let (entry, engine) = match self.reserve(&request, &shell_display)? {
            Reserved::Existing(entry) => return Ok(entry),
            Reserved::New { entry, engine } => (entry, engine),
        };
        let spec = StartSpec {
            folder: request.folder,
            shell,
            kind: request.kind,
            size: request.size,
            resume_token: self.resume_token(),
            resume: request.reopen.map(|reopen| reopen.start),
        };
        let events: Arc<dyn EngineEvents> = Arc::new(HubEvents(self.me.clone()));
        // A start that fails or panics takes its reservation with it.
        let reservation = Reservation {
            hub: self,
            id: &entry.id,
        };
        let started = engine.start(&entry.id, &spec, events);
        std::mem::forget(reservation);
        self.commit(&request.owner, &entry.id, started)
    }

    fn ended(&self, terminal: &str, status: Option<i32>, program_status: Option<ProgramStatus>) {
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
        // Drop live (working/blocked) states atomically with exit; keep settled states.
        let post_exit = program_status.filter(|s| !s.state.is_live());
        if live.entry.program_status.as_ref().map(|s| &s.status) != post_exit.as_ref() {
            live.entry.program_status = post_exit.map(|status| StampedProgramStatus {
                status,
                updated_at_ms: at_ms,
            });
        }
        // The program ended on its own (or the person ended it): nothing
        // brings it back, unless the gateway is stopping, which ends every
        // terminal and keeps their records.
        let forget = self
            .resume
            .as_ref()
            .is_some_and(|resume| !resume.frozen.load(Ordering::Acquire))
            && (live.saved.take().is_some() | live.entry.resume_command.take().is_some());
        let owner = live.owner.clone();
        let listed = matches!(live.running, Process::Started(_));
        if listed {
            self.announce(&mut hub, &owner);
        }
        drop(hub);
        if forget {
            if let Some(resume) = &self.resume {
                // Again under the lock `freeze` takes: a freeze between the
                // first look and here keeps the record.
                let _writing = resume.writing();
                if !resume.frozen.load(Ordering::Acquire) {
                    resume.forget(terminal);
                }
            }
        }
    }

    fn program_status_changed(&self, terminal: &str, status: Option<ProgramStatus>) {
        let updated_at_ms = self.clock.now_ms();
        let mut hub = self.lock();
        let Some(live) = hub
            .terminals
            .iter_mut()
            .find(|live| live.entry.id == terminal)
        else {
            return;
        };
        // An exited terminal can never take a live (working/blocked) status.
        if live.entry.ended.is_some() && status.as_ref().is_some_and(|s| s.state.is_live()) {
            return;
        }
        if live.entry.program_status.as_ref().map(|s| &s.status) == status.as_ref() {
            return;
        }
        live.entry.program_status = status.map(|status| StampedProgramStatus {
            status,
            updated_at_ms,
        });
        let owner = live.owner.clone();
        let listed = matches!(live.running, Process::Started(_));
        if listed {
            self.announce(&mut hub, &owner);
        }
    }

    /// Tell each listening connection of `owner` that the list changed.
    fn announce(&self, hub: &mut Hub, owner: &TerminalOwner) {
        self.changes.send_modify(|tick| *tick += 1);
        for (listener, listed) in hub.links.values() {
            if listener == owner {
                // A counter, not a queue: busy output never crowds it out.
                listed.send_modify(|tick| *tick = tick.wrapping_add(1));
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
                key: key.clone(),
                out: link.out.clone(),
            });
            // Nobody sizes it (its owner closed, or it was never focused):
            // this view does, so a pane that comes back after its old view
            // went is not left showing a grid sized for a box that is gone.
            if live.size_owner.is_none() {
                Self::take_size(live, key, fit);
            }
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
            Self::take_size(live, key, fit);
            Ok(())
        })
    }

    /// `key` owns the size from now on; the terminal takes its fit.
    fn take_size(live: &mut Live, key: ViewKey, fit: GridSize) {
        live.size_owner = Some(key);
        if live.size != fit {
            live.size = fit;
            live.running.send(EngineCommand::Resize(fit));
        }
    }

    /// The view's box changed. The size owner's change resizes; an unowned
    /// terminal is taken by the view whose box changed.
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
            match &live.size_owner {
                Some(owner) if owner != &key => {}
                _ => Self::take_size(live, key, fit),
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
        let mut live = hub.terminals.remove(index);
        let listed = matches!(live.running, Process::Started(_));
        live.running.send(EngineCommand::Close);
        if listed {
            self.announce(&mut hub, owner);
        }
        drop(hub);
        // Closed on purpose: its program is not to come back.
        if let Some(resume) = &self.resume {
            let _writing = resume.writing();
            resume.forget(id);
        }
        Ok(())
    }

    /// The person's own name for `id`; every list shows it from now on.
    pub fn rename(
        &self,
        owner: &TerminalOwner,
        id: &str,
        title: String,
    ) -> Result<(), TerminalError> {
        let mut hub = self.lock();
        let live = hub
            .terminals
            .iter_mut()
            .find(|live| live.entry.id == id && &live.owner == owner)
            .ok_or(TerminalError::NotFound)?;
        if live.entry.title == title {
            return Ok(());
        }
        live.entry.title = title;
        self.announce(&mut hub, owner);
        drop(hub);
        // A kept terminal comes back under the name it has now.
        if let Some(resume) = &self.resume {
            let _writing = resume.writing();
            if let Some(saved) = self.saved_terminal(id) {
                if let Err(error) = resume.store.save(&saved) {
                    tracing::warn!(target: "nessa::terminal", terminal = %id, %error, "resume record not renamed");
                }
            }
        }
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

/// One connection's place in the hub. Dropped with the socket, it takes
/// every view of that connection with it.
pub struct TerminalLink {
    hub: std::sync::Weak<TerminalHub>,
    connection: u64,
    owner: TerminalOwner,
    out: mpsc::Sender<TerminalSignal>,
    listed: Arc<watch::Sender<u64>>,
    listening: AtomicBool,
}

impl TerminalLink {
    fn key(&self, view: &str) -> ViewKey {
        ViewKey {
            connection: self.connection,
            view: view.to_owned(),
        }
    }

    pub fn hub(&self) -> Result<Arc<TerminalHub>, TerminalError> {
        self.hub.upgrade().ok_or(TerminalError::Unavailable)
    }

    pub fn owner(&self) -> &TerminalOwner {
        &self.owner
    }

    /// The owner's terminals; from now on this connection is told of changes.
    pub fn list(&self) -> Result<Vec<TerminalEntry>, TerminalError> {
        let hub = self.hub()?;
        if !self.listening.swap(true, Ordering::AcqRel) {
            hub.lock().links.insert(
                self.connection,
                (self.owner.clone(), Arc::clone(&self.listed)),
            );
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

    pub fn rename(&self, id: &str, title: String) -> Result<(), TerminalError> {
        self.hub()?.rename(&self.owner, id, title)
    }
}

impl Drop for TerminalLink {
    fn drop(&mut self) {
        if let Some(hub) = self.hub.upgrade() {
            hub.disconnect(self.connection);
        }
    }
}

/// A reservation `open` holds while the engine starts its program. Dropped
/// by a panicking start, it removes the reservation so it neither counts
/// toward the limit nor answers a retry forever.
struct Reservation<'a> {
    hub: &'a TerminalHub,
    id: &'a str,
}

impl Drop for Reservation<'_> {
    fn drop(&mut self) {
        let mut hub = self.hub.lock();
        hub.terminals.retain(|live| {
            live.entry.id != self.id || !matches!(live.running, Process::Starting(_))
        });
    }
}

/// What one connection is sent: its views' output, in order, and word that
/// the owner's list changed. Several changes while nobody reads are one.
pub struct TerminalSignals {
    output: mpsc::Receiver<TerminalSignal>,
    /// `None` once the link went.
    listed: Option<watch::Receiver<u64>>,
}

impl TerminalSignals {
    /// The next signal; `None` once the connection's link went.
    pub async fn recv(&mut self) -> Option<TerminalSignal> {
        loop {
            let Some(listed) = self.listed.as_mut() else {
                return self.output.recv().await;
            };
            tokio::select! {
                biased;
                changed = listed.changed() => match changed {
                    Ok(()) => return Some(TerminalSignal::Listed),
                    Err(_) => self.listed = None,
                },
                signal = self.output.recv() => return signal,
            }
        }
    }

    /// The next signal waiting now, if any.
    pub fn try_recv(&mut self) -> Option<TerminalSignal> {
        if let Some(listed) = self.listed.as_mut() {
            if listed.has_changed().unwrap_or(false) {
                listed.borrow_and_update();
                return Some(TerminalSignal::Listed);
            }
        }
        self.output.try_recv().ok()
    }
}
