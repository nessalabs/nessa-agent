//! The person's terminals and the rules for several views of one: who owns
//! the size, which views are attached, and who hears the list.
use super::*;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use tokio::sync::{mpsc, watch};

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

    /// Start a terminal, or answer the one `request.request_id` started.
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
        let engine = self.engine.clone().ok_or(TerminalError::Unavailable)?;
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
            id: format!("t_{}", uuid::Uuid::new_v4().simple()),
            request_id: request.request_id.clone(),
            title,
            kind: request.kind.clone(),
            channel_id: request.channel_id.clone(),
            conversation_id: request.conversation_id.clone(),
            folder: request.folder.to_string_lossy().into_owned(),
            shell: shell.to_string_lossy().into_owned(),
            started_at_ms: self.clock.now_ms(),
            ended: None,
        };
        {
            let mut hub = self.lock();
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
            hub.terminals.push(Live {
                owner: request.owner.clone(),
                entry: entry.clone(),
                running: Process::Starting(Vec::new()),
                views: HashMap::new(),
                size_owner: None,
                size: request.size,
            });
        }
        let spec = StartSpec {
            folder: request.folder,
            shell,
            kind: request.kind,
            size: request.size,
        };
        let events: Arc<dyn EngineEvents> = Arc::new(HubEvents(self.me.clone()));
        // A start that fails or panics takes its reservation with it.
        let reservation = Reservation {
            hub: self,
            id: &entry.id,
        };
        let started = engine.start(&entry.id, &spec, events);
        std::mem::forget(reservation);
        let mut hub = self.lock();
        let index = hub
            .terminals
            .iter()
            .position(|live| live.entry.id == entry.id);
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
