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
use crate::conversation::domain::WorkspaceWindow;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use std::{
    collections::{BTreeMap, HashMap},
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

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
    windows: Mutex<HashMap<Owner, BTreeMap<u64, Publication>>>,
}

/// A connection's place: what it published last, if anything, and its
/// window, if that publication had one.
struct Publication {
    said: Option<Said>,
    window: Option<Arc<WorkspaceWindow>>,
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
    /// empty until it does. Dropping it withdraws the window.
    pub fn open(
        self: &Arc<Self>,
        organization: OrganizationId,
        principal: PrincipalId,
    ) -> WindowSlot {
        let id = self.next.fetch_add(1, Ordering::Relaxed);
        let owner = (organization, principal);
        self.lock().entry(owner.clone()).or_default().insert(
            id,
            Publication {
                said: None,
                window: None,
            },
        );
        WindowSlot {
            windows: self.clone(),
            owner,
            id,
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
        WindowPublished {
            revision,
            applied: true,
        }
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
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/workspace_windows.rs"]
mod tests;
