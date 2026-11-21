//! Pure lease accounting. The caller owns authorization and all provider effects.
use crate::{Change, Identity, Inventory, Listener, SnapshotError, StaleGeneration};
use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::num::NonZeroU16;

/// A provider-reported local endpoint with a loopback address and nonzero port.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct LoopbackEndpoint(SocketAddr);

impl LoopbackEndpoint {
    /// Validate an endpoint returned by the provider.
    pub fn new(address: SocketAddr) -> Option<Self> {
        if address.ip().is_loopback() && NonZeroU16::new(address.port()).is_some() {
            Some(Self(address))
        } else {
            None
        }
    }

    /// The validated local socket address.
    pub fn address(self) -> SocketAddr {
        self.0
    }
}

/// An opaque reservation identifier scoped to one owner and workspace generation.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct LeaseId {
    workspace: Identity,
    generation: u64,
    sequence: u64,
}

#[derive(Debug)]
enum State {
    Opening,
    Active,
    ClosingOpening(ReleaseReason),
    ClosingResource,
}

#[derive(Debug)]
struct Lease {
    listener: Listener,
    state: State,
}

/// Why an acquired resource must be released.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ReleaseReason {
    /// The caller requested closure.
    Closed,
    /// The observed listener disappeared or changed.
    Gone,
    /// The workspace generation was replaced.
    Replaced,
    /// The provider returned a non-loopback address or port zero.
    InvalidEndpoint,
}

/// What the caller must do after a close or invalidation.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CloseAction {
    /// Await the opening result; release a late success.
    AwaitOpening,
    /// Release the caller-owned provider resource, then confirm release.
    Release,
    /// Cleanup has already been requested.
    AlreadyClosing,
}

/// Result of a successful provider opening.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Completion {
    /// The lease is active with this validated loopback endpoint.
    Active(LoopbackEndpoint),
    /// Release the returned resource, then confirm release.
    Release(ReleaseReason),
}

/// The provider completion does not match an outstanding opening.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum CompletionError {
    /// No reservation belongs to this owner.
    UnknownLease,
    /// This reservation has already received an opening outcome.
    NotOpening,
}

/// A requested operation cannot proceed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ForwardError {
    /// No matching current workspace, generation, revision and listener exists.
    StaleInventory,
    /// The workspace has been closed.
    WorkspaceClosed,
    /// Pending, active and closing leases occupy the configured capacity.
    Capacity,
    /// The sequence space is exhausted; create a new owner only after cleanup.
    IdExhausted,
    /// No reservation belongs to this owner.
    UnknownLease,
    /// The transition does not apply to the current lease state.
    WrongState,
}

/// A rejected snapshot and any cleanup required by its ambiguity.
#[derive(Debug, Eq, PartialEq)]
pub struct ApplyError {
    /// Why the snapshot was rejected.
    pub error: SnapshotError,
    /// Cleanup the caller must perform even though the snapshot was rejected.
    pub cleanup: Vec<(LeaseId, CloseAction)>,
}

/// Owns a workspace's observations and bounded forward reservations.
///
/// This type performs no authorization or I/O. A caller must retain ownership
/// of provider resources and honor every returned cleanup instruction.
#[derive(Debug)]
pub struct Ports {
    inventory: Inventory,
    capacity: usize,
    closed: bool,
    uncertain_revision: Option<u64>,
    next_sequence: Option<u64>,
    leases: BTreeMap<LeaseId, Lease>,
}

impl Ports {
    /// Start one owner for a workspace generation and a maximum number of leases.
    pub fn new(workspace: Identity, generation: u64, capacity: usize) -> Self {
        Self {
            inventory: Inventory::new(workspace, generation),
            capacity,
            closed: false,
            uncertain_revision: None,
            next_sequence: Some(0),
            leases: BTreeMap::new(),
        }
    }

    /// Number of pending, active and cleanup-pending reservations.
    pub fn occupied(&self) -> usize {
        self.leases.len()
    }

    /// Last accepted advisory inventory. It may be ineligible for requests
    /// after a closed workspace or an ambiguous snapshot.
    pub fn inventory(&self) -> &Inventory {
        &self.inventory
    }

    /// Apply a complete snapshot and return cleanup instructions for affected leases.
    /// Ambiguous snapshots return cleanup instructions and make inventory
    /// ineligible until a newer revision is accepted. An identical snapshot
    /// opens nothing. A successful recovery from ambiguity reports `Changed`,
    /// even if observations match the last accepted snapshot. After any accepted
    /// snapshot, including `Unchanged`, read `inventory().revision()` for the
    /// current revision before requesting a lease.
    pub fn apply(
        &mut self,
        workspace: &Identity,
        generation: u64,
        revision: u64,
        listeners: impl IntoIterator<Item = Listener>,
    ) -> Result<(Change, Vec<(LeaseId, CloseAction)>), ApplyError> {
        let reject = |error| ApplyError {
            error,
            cleanup: Vec::new(),
        };
        if self.closed {
            return Err(reject(SnapshotError::WorkspaceClosed));
        }
        if workspace != &self.inventory.workspace || generation != self.inventory.generation {
            return Err(reject(SnapshotError::WrongWorkspace));
        }
        if self
            .uncertain_revision
            .is_some_and(|uncertain| revision <= uncertain)
        {
            return Err(reject(SnapshotError::StaleRevision));
        }
        let change = match self
            .inventory
            .apply(workspace, generation, revision, listeners)
        {
            Ok(change) => change,
            Err(
                error @ (SnapshotError::DuplicateListener | SnapshotError::ConflictingRevision),
            ) => {
                self.uncertain_revision = Some(revision);
                return Err(ApplyError {
                    error,
                    cleanup: self.invalidate_all(ReleaseReason::Gone),
                });
            }
            Err(error) => return Err(reject(error)),
        };
        let recovered = self.uncertain_revision.take().is_some();
        let mut actions = Vec::new();
        for (id, lease) in &mut self.leases {
            if self.inventory.listeners.get(lease.listener.id()) != Some(&lease.listener) {
                let action = close_state(&mut lease.state, ReleaseReason::Gone);
                if action != CloseAction::AlreadyClosing {
                    actions.push((id.clone(), action));
                }
            }
        }
        Ok((if recovered { Change::Changed } else { change }, actions))
    }

    /// Replace the workspace incarnation and return cleanup instructions.
    pub fn replace_generation(
        &mut self,
        generation: u64,
    ) -> Result<Vec<(LeaseId, CloseAction)>, StaleGeneration> {
        self.inventory.replace_generation(generation)?;
        self.closed = false;
        self.uncertain_revision = None;
        Ok(self.invalidate_all(ReleaseReason::Replaced))
    }

    /// Close all current leases without allowing discovery to reopen them.
    pub fn close_all(&mut self) -> Vec<(LeaseId, CloseAction)> {
        self.closed = true;
        self.invalidate_all(ReleaseReason::Closed)
    }

    fn invalidate_all(&mut self, reason: ReleaseReason) -> Vec<(LeaseId, CloseAction)> {
        self.leases
            .iter_mut()
            .filter_map(|(id, lease)| {
                let action = close_state(&mut lease.state, reason);
                (action != CloseAction::AlreadyClosing).then_some((id.clone(), action))
            })
            .collect()
    }

    /// Reserve a slot for an explicitly requested and externally authorized open.
    /// The caller must supply the exact current workspace, generation, revision,
    /// and complete listener observation. This method does not open a resource.
    pub fn request(
        &mut self,
        workspace: &Identity,
        generation: u64,
        revision: u64,
        listener: &Listener,
    ) -> Result<LeaseId, ForwardError> {
        if self.closed {
            return Err(ForwardError::WorkspaceClosed);
        }
        if self.uncertain_revision.is_some()
            || &self.inventory.workspace != workspace
            || self.inventory.generation != generation
            || self.inventory.revision != Some(revision)
            || self.inventory.listeners.get(listener.id()) != Some(listener)
        {
            return Err(ForwardError::StaleInventory);
        }
        if self.leases.len() >= self.capacity {
            return Err(ForwardError::Capacity);
        }
        let sequence = self.next_sequence.ok_or(ForwardError::IdExhausted)?;
        self.next_sequence = sequence.checked_add(1);
        let id = LeaseId {
            workspace: self.inventory.workspace.clone(),
            generation,
            sequence,
        };
        self.leases.insert(
            id.clone(),
            Lease {
                listener: listener.clone(),
                state: State::Opening,
            },
        );
        Ok(id)
    }

    /// Report a successful provider opening. Even an invalid endpoint represents
    /// an acquired resource and must be released as directed by the result.
    /// On an error the caller still owns any resource it received and must
    /// release it independently; the owner did not attach that resource.
    pub fn opened(
        &mut self,
        id: &LeaseId,
        endpoint: SocketAddr,
    ) -> Result<Completion, CompletionError> {
        let lease = self
            .leases
            .get_mut(id)
            .ok_or(CompletionError::UnknownLease)?;
        match lease.state {
            State::Opening => match LoopbackEndpoint::new(endpoint) {
                Some(endpoint) => {
                    lease.state = State::Active;
                    Ok(Completion::Active(endpoint))
                }
                None => {
                    lease.state = State::ClosingResource;
                    Ok(Completion::Release(ReleaseReason::InvalidEndpoint))
                }
            },
            State::ClosingOpening(reason) => {
                lease.state = State::ClosingResource;
                Ok(Completion::Release(reason))
            }
            State::Active | State::ClosingResource => Err(CompletionError::NotOpening),
        }
    }

    /// Report that an opening failed without acquiring a resource.
    pub fn open_failed(&mut self, id: &LeaseId) -> Result<(), CompletionError> {
        match self.leases.get(id).map(|lease| &lease.state) {
            None => Err(CompletionError::UnknownLease),
            Some(State::Opening | State::ClosingOpening(_)) => {
                self.leases.remove(id);
                Ok(())
            }
            Some(State::Active | State::ClosingResource) => Err(CompletionError::NotOpening),
        }
    }

    /// Close a reservation. The caller follows the returned cleanup instruction.
    pub fn close(&mut self, id: &LeaseId) -> Result<CloseAction, ForwardError> {
        let lease = self.leases.get_mut(id).ok_or(ForwardError::UnknownLease)?;
        Ok(close_state(&mut lease.state, ReleaseReason::Closed))
    }

    /// Confirm the caller released a resource after a release instruction.
    pub fn released(&mut self, id: &LeaseId) -> Result<(), ForwardError> {
        match self.leases.get(id).map(|lease| &lease.state) {
            None => Err(ForwardError::UnknownLease),
            Some(State::ClosingResource) => {
                self.leases.remove(id);
                Ok(())
            }
            Some(State::Opening | State::Active | State::ClosingOpening(_)) => {
                Err(ForwardError::WrongState)
            }
        }
    }
    /// Report confirmed provider loss of an acquired resource. Pending opens
    /// remain reserved until the provider reports an opening outcome.
    pub fn lost(&mut self, id: &LeaseId) -> Result<(), ForwardError> {
        match self.leases.get(id).map(|lease| &lease.state) {
            None => Err(ForwardError::UnknownLease),
            Some(State::Active | State::ClosingResource) => {
                self.leases.remove(id);
                Ok(())
            }
            Some(State::Opening | State::ClosingOpening(_)) => Err(ForwardError::WrongState),
        }
    }
}

fn close_state(state: &mut State, reason: ReleaseReason) -> CloseAction {
    match state {
        State::Opening => {
            *state = State::ClosingOpening(reason);
            CloseAction::AwaitOpening
        }
        State::Active => {
            *state = State::ClosingResource;
            CloseAction::Release
        }
        State::ClosingOpening(_) | State::ClosingResource => CloseAction::AlreadyClosing,
    }
}
