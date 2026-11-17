//! The desktop windows each person has open now, as each last published
//! itself (`workspace.publish`), for their agents to read (`read_window`).
//!
//! ```text
//! socket opens ──▶ WorkspaceWindows::open(org, principal) ──▶ WindowSlot
//! workspace.publish ──▶ WindowSlot::publish(window id, revision, window)  (a lower revision changes nothing)
//! socket closes ──▶ WindowSlot dropped ──▶ its window withdrawn
//! read_window ──▶ WorkspaceWindows::of(org, principal)
//!   ──▶ one per window id: its highest revision, on whichever connection
//!   ──▶ newest publication first
//! ```
//!
//! An agent's or a CLI's change to a window (`control_window`) goes to the
//! connection that holds that window's latest word, and its answer comes back
//! on the same connection:
//!
//! ```text
//! WorkspaceWindows::command(owner, window id, action)
//!   ──▶ the connection holding the window's highest revision
//!   ──▶ its command lane (the socket sends `workspace.command`)  ── full ──▶ Busy
//!   ──▶ oneshot answer, kept by command id on that connection
//! workspace.answer ──▶ WindowSlot::answer(command id, answer) ──▶ oneshot
//! socket closes ──▶ the answers it owed are dropped ──▶ the caller hears Gone
//! ```
//!
//! A window that reconnects publishes under the same id, so the connection
//! it left behind (a socket the gateway has not seen close yet, after a
//! laptop sleeps) never reads as a second window.
//!
//! The window is the authority for what it shows; this keeps only what a
//! connection that is open now last said. Nothing is written to disk on
//! purpose: a window that is gone shows nothing, so a layout kept past its
//! connection, or past a restart, would tell an agent about a screen nobody
//! has. A window that connects again publishes again. A person's windows
//! are bounded by their connections, so there is no count to keep here.
use crate::domain::{WindowAction, WorkspaceWindow};
use nessa_auth::domain::{OrganizationId, PrincipalId};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};
use tokio::sync::{mpsc, oneshot, Notify};

/// Commands one connection holds unanswered, sent and not yet sent. A window
/// answers each at once, so more than this means it has stopped reading.
pub const MAX_WINDOW_COMMANDS: usize = 8;

/// A change for one window, sent to the connection that holds it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct WindowCommand {
    /// Unique across the gateway's run, for the answer to name.
    pub id: u64,
    /// The window's own id: a connection may come to hold another window's.
    pub window_id: String,
    pub action: WindowAction,
}

/// What a window said of a command.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum WindowAnswer {
    /// Done. `revision` is the publication that shows it.
    Applied { revision: u64 },
    /// Already so: nothing changed. `revision` is the one that shows it.
    Unchanged { revision: u64 },
    /// The window could not do it, for `code` (a conversation it does not
    /// list, a panel tab the focused chat does not have…).
    Refused { code: String },
}

/// Why a command could not be handed to a window.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum CommandRefusal {
    /// No open connection holds that window now.
    Gone,
    /// The connection holding it has [`MAX_WINDOW_COMMANDS`] unanswered.
    Busy,
}

type Owner = (OrganizationId, PrincipalId);

/// One window a person has open, as it last published itself.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublishedWindow {
    /// The window's own id, kept across its connections.
    pub id: String,
    /// Its publication number, as the window counts them.
    pub revision: u64,
    pub window: Arc<WorkspaceWindow>,
}

/// Every open window, by owner, then by the order its connection opened.
#[derive(Default)]
pub struct WorkspaceWindows {
    next: AtomicU64,
    /// Counts publications across every connection, so the latest is known.
    published: AtomicU64,
    /// Numbers commands across every connection.
    commands: AtomicU64,
    windows: Mutex<HashMap<Owner, BTreeMap<u64, Publication>>>,
    /// Woken by every publication, for a caller waiting on a revision.
    publications: Notify,
}

/// A connection's place: what it published last, if anything, and its
/// window, if that publication had one; where its commands go, and the
/// answers it owes.
struct Publication {
    said: Option<Said>,
    window: Option<Arc<WorkspaceWindow>>,
    commands: mpsc::Sender<WindowCommand>,
    /// By command id: the window it was for, and its caller's answer.
    owed: HashMap<u64, (String, oneshot::Sender<WindowAnswer>)>,
}

struct Said {
    id: String,
    revision: u64,
    /// When, among every connection's publications.
    at: u64,
}

impl WorkspaceWindows {
    pub fn new() -> Arc<Self> {
        Arc::new(Self::default())
    }

    /// A place for one connection of `principal`'s to publish its window in,
    /// empty until it does, and the commands sent to its window, for the
    /// connection to deliver. Dropping the slot withdraws the window.
    pub fn open(
        self: &Arc<Self>,
        organization: OrganizationId,
        principal: PrincipalId,
    ) -> (WindowSlot, mpsc::Receiver<WindowCommand>) {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let owner = (organization, principal);
        let (commands, receiver) = mpsc::channel(MAX_WINDOW_COMMANDS);
        self.lock().entry(owner.clone()).or_default().insert(
            id,
            Publication {
                said: None,
                window: None,
                commands,
                owed: HashMap::new(),
            },
        );
        (
            WindowSlot {
                windows: self.clone(),
                owner,
                id,
            },
            receiver,
        )
    }

    /// Hands `action` to the connection that holds `window_id`'s latest word,
    /// and the answer it will owe. The answer's sender is dropped, unsent,
    /// when that connection closes first.
    ///
    /// # Errors
    /// [`CommandRefusal::Gone`] when no open connection holds that window,
    /// [`CommandRefusal::Busy`] when the one holding it owes too many answers.
    pub fn command(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        window_id: &str,
        action: WindowAction,
    ) -> Result<oneshot::Receiver<WindowAnswer>, CommandRefusal> {
        let mut windows = self.lock();
        let owned = windows
            .get_mut(&(organization.clone(), principal.clone()))
            .ok_or(CommandRefusal::Gone)?;
        let holder = owned
            .values_mut()
            .filter(|publication| {
                publication.window.is_some()
                    && publication
                        .said
                        .as_ref()
                        .is_some_and(|said| said.id == window_id)
            })
            .max_by_key(|publication| {
                let said = publication.said.as_ref().expect("filtered on said");
                (said.revision, said.at)
            })
            .ok_or(CommandRefusal::Gone)?;
        // Answers whose caller stopped waiting are owed to nobody.
        holder.owed.retain(|_, (_, answer)| !answer.is_closed());
        if holder.owed.len() >= MAX_WINDOW_COMMANDS {
            return Err(CommandRefusal::Busy);
        }
        let id = self.commands.fetch_add(1, Ordering::Relaxed);
        let command = WindowCommand {
            id,
            window_id: window_id.to_owned(),
            action,
        };
        match holder.commands.try_send(command) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) => return Err(CommandRefusal::Busy),
            Err(mpsc::error::TrySendError::Closed(_)) => return Err(CommandRefusal::Gone),
        }
        let (answer, answered) = oneshot::channel();
        holder.owed.insert(id, (window_id.to_owned(), answer));
        Ok(answered)
    }

    /// Waits until `window_id`'s latest word is at least `revision`, on any
    /// of its owner's connections: true then, at once when it is already;
    /// false once no connection holds the window (it closed, or withdrew
    /// it). The caller bounds the wait.
    pub async fn published(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
        window_id: &str,
        revision: u64,
    ) -> bool {
        loop {
            let notified = self.publications.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            let held = self
                .of(organization, principal)
                .into_iter()
                .find(|window| window.id == window_id)
                .map(|window| window.revision);
            match held {
                Some(held) if held >= revision => return true,
                // Withdrawn: its connection closed, or it said it shows nothing.
                None => return false,
                Some(_) => notified.await,
            }
        }
    }

    /// The windows `principal` in `organization` has open and published:
    /// each window once, as its latest publication says, newest first.
    pub fn of(
        &self,
        organization: &OrganizationId,
        principal: &PrincipalId,
    ) -> Vec<PublishedWindow> {
        let windows = self.lock();
        let Some(owned) = windows.get(&(organization.clone(), principal.clone())) else {
            return Vec::new();
        };
        // A window's revisions go on rising across its connections, so its
        // latest word is its highest revision; a withdrawal counts too.
        let mut latest: HashMap<&str, (&Said, Option<&Arc<WorkspaceWindow>>)> = HashMap::new();
        for publication in owned.values() {
            let Some(said) = &publication.said else {
                continue;
            };
            let newer = latest
                .get(said.id.as_str())
                .is_none_or(|(held, _)| (held.revision, held.at) < (said.revision, said.at));
            if newer {
                latest.insert(&said.id, (said, publication.window.as_ref()));
            }
        }
        let mut shown: Vec<(u64, PublishedWindow)> = latest
            .into_values()
            .filter_map(|(said, window)| {
                Some((
                    said.at,
                    PublishedWindow {
                        id: said.id.clone(),
                        revision: said.revision,
                        window: window?.clone(),
                    },
                ))
            })
            .collect();
        shown.sort_by_key(|(at, _)| std::cmp::Reverse(*at));
        shown.into_iter().map(|(_, window)| window).collect()
    }

    fn lock(&self) -> MutexGuard<'_, HashMap<Owner, BTreeMap<u64, Publication>>> {
        // Every change here is one insert or remove: a panic elsewhere while
        // the lock was held leaves nothing half written.
        self.windows
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }
}

/// What a publication did.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct WindowPublished {
    /// The revision held now.
    pub revision: u64,
    /// False when a later revision was already held, so this changed nothing.
    pub applied: bool,
}

/// One connection's window. Dropped with its connection, which withdraws it.
pub struct WindowSlot {
    windows: Arc<WorkspaceWindows>,
    owner: Owner,
    id: u64,
}

impl WindowSlot {
    /// Holds `window` as this connection's, or withdraws it when `None`,
    /// unless a later revision of the window is held already, on this or
    /// another of its connections: requests may be answered out of order,
    /// and the newest word wins. `id` is the
    /// window's own, the same on each of its connections.
    pub fn publish(
        &self,
        id: &str,
        revision: u64,
        window: Option<WorkspaceWindow>,
    ) -> WindowPublished {
        let mut windows = self.windows.lock();
        let at = self.windows.published.fetch_add(1, Ordering::Relaxed);
        let owned = windows
            .get_mut(&self.owner)
            .expect("a slot's place lives until the slot is dropped");
        // The window's highest revision on any of its connections is the
        // guard: a late answer on a connection it left behind says less.
        let newest = owned
            .values()
            .filter_map(|other| other.said.as_ref())
            .filter(|said| said.id == id)
            .map(|said| said.revision)
            .max();
        if let Some(held) = newest.filter(|held| *held > revision) {
            return WindowPublished {
                revision: held,
                applied: false,
            };
        }
        // What a connection this window left behind holds is older than this:
        // forgotten, so it cannot read as open once this connection closes.
        for (slot, other) in owned.iter_mut() {
            let older = other
                .said
                .as_ref()
                .is_some_and(|said| said.id == id && said.revision < revision);
            if *slot != self.id && older {
                other.said = None;
                other.window = None;
            }
        }
        let publication = owned
            .get_mut(&self.id)
            .expect("a slot's place lives until the slot is dropped");
        publication.said = Some(Said {
            id: id.to_owned(),
            revision,
            at,
        });
        publication.window = window.map(Arc::new);
        drop(windows);
        self.windows.publications.notify_waiters();
        WindowPublished {
            revision,
            applied: true,
        }
    }

    /// Settles the command `command` for `window_id` with `answer`, when
    /// this connection owes it. False for a command it does not owe:
    /// answered already, never sent here, for another window, or no longer
    /// awaited.
    pub fn answer(&self, window_id: &str, command: u64, answer: WindowAnswer) -> bool {
        let mut windows = self.windows.lock();
        let Some(owed) = windows
            .get_mut(&self.owner)
            .and_then(|owned| owned.get_mut(&self.id))
        else {
            return false;
        };
        if owed
            .owed
            .get(&command)
            .is_none_or(|(window, _)| window != window_id)
        {
            return false;
        }
        let (_, sender) = owed.owed.remove(&command).expect("checked above");
        sender.send(answer).is_ok()
    }
}

impl Drop for WindowSlot {
    fn drop(&mut self) {
        let mut windows = self.windows.lock();
        if let Some(owned) = windows.get_mut(&self.owner) {
            owned.remove(&self.id);
            if owned.is_empty() {
                windows.remove(&self.owner);
            }
        }
        drop(windows);
        // A wait for this window's publication hears it is gone.
        self.windows.publications.notify_waiters();
    }
}

#[cfg(test)]
#[path = "../../tests/conversation/workspace_windows.rs"]
mod tests;
