//! Advisory, host-supplied listener snapshots. This crate performs no discovery,
//! authorization, network operations, or forwarding.
//!
//! A host must supply stable listener identities and a monotonically increasing
//! generation for each replacement of a workspace. Inventory is not proof that a
//! listener still exists and does not authorize access to it.

use std::collections::BTreeMap;
use std::net::IpAddr;
use std::num::NonZeroU16;

/// An opaque, host-supplied identity. The host owns uniqueness and stability.
#[derive(Clone, Debug, Eq, Ord, PartialEq, PartialOrd)]
pub struct Identity(String);

impl Identity {
    /// Reject an empty or whitespace-only identity.
    pub fn new(value: impl Into<String>) -> Result<Self, InvalidIdentity> {
        let value = value.into();
        if value.trim().is_empty() {
            return Err(InvalidIdentity);
        }
        Ok(Self(value))
    }

    /// The original host-supplied value.
    pub fn as_str(&self) -> &str {
        &self.0
    }
}

/// An identity contained no non-whitespace characters.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidIdentity;

/// One observed listener. A numeric port is an attribute, not its identity.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct Listener {
    id: Identity,
    address: IpAddr,
    port: NonZeroU16,
}

impl Listener {
    /// Construct an observation with a nonzero port.
    pub fn new(id: Identity, address: IpAddr, port: u16) -> Result<Self, InvalidPort> {
        Ok(Self {
            id,
            address,
            port: NonZeroU16::new(port).ok_or(InvalidPort)?,
        })
    }

    /// The host's listener identity, unique within this workspace generation.
    pub fn id(&self) -> &Identity {
        &self.id
    }
    /// The observed bind address, which may be a wildcard.
    pub fn address(&self) -> IpAddr {
        self.address
    }
    /// The observed nonzero port.
    pub fn port(&self) -> NonZeroU16 {
        self.port
    }
}

/// Port zero does not identify an observed listener.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct InvalidPort;

/// Owns the latest complete snapshot for one host-identified workspace.
#[derive(Debug)]
pub struct Inventory {
    workspace: Identity,
    generation: u64,
    revision: Option<u64>,
    listeners: BTreeMap<Identity, Listener>,
}

/// A replacement generation did not advance.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct StaleGeneration;

/// Result of accepting a snapshot.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum Change {
    /// The observed set changed, including the first snapshot, or a `Ports`
    /// owner recovered eligibility after an ambiguous snapshot.
    Changed,
    /// The observation is identical and eligibility did not change.
    Unchanged,
}

/// A snapshot was rejected without changing the inventory.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum SnapshotError {
    /// The snapshot belongs to a different workspace or generation.
    WrongWorkspace,
    /// The revision precedes the accepted revision.
    StaleRevision,
    /// The same revision described a different snapshot.
    ConflictingRevision,
    /// More than one observation claimed the same listener identity.
    DuplicateListener,
    /// The workspace owner is closed.
    WorkspaceClosed,
}

impl Inventory {
    /// Start an empty inventory for one workspace incarnation.
    pub fn new(workspace: Identity, generation: u64) -> Self {
        Self {
            workspace,
            generation,
            revision: None,
            listeners: BTreeMap::new(),
        }
    }

    /// Retire the current incarnation and start an empty newer generation.
    /// A repeated or older generation leaves the current inventory intact.
    pub fn replace_generation(&mut self, generation: u64) -> Result<(), StaleGeneration> {
        if generation <= self.generation {
            return Err(StaleGeneration);
        }
        self.generation = generation;
        self.revision = None;
        self.listeners.clear();
        Ok(())
    }

    /// The accepted revision, or none before the first complete snapshot.
    pub fn revision(&self) -> Option<u64> {
        self.revision
    }

    /// Observations in listener identity order. They remain advisory.
    pub fn listeners(&self) -> impl Iterator<Item = &Listener> {
        self.listeners.values()
    }

    /// Accept a complete snapshot for this workspace incarnation.
    ///
    /// Newer identical snapshots advance the revision without a change event.
    /// Equal revisions may be replayed only with identical content. Rejections
    /// leave the previously accepted snapshot and revision intact.
    pub fn apply(
        &mut self,
        workspace: &Identity,
        generation: u64,
        revision: u64,
        listeners: impl IntoIterator<Item = Listener>,
    ) -> Result<Change, SnapshotError> {
        if workspace != &self.workspace || generation != self.generation {
            return Err(SnapshotError::WrongWorkspace);
        }
        if self.revision.is_some_and(|current| revision < current) {
            return Err(SnapshotError::StaleRevision);
        }
        let mut next = BTreeMap::new();
        for listener in listeners {
            if next.insert(listener.id.clone(), listener).is_some() {
                return Err(SnapshotError::DuplicateListener);
            }
        }
        let changed = self.revision.is_none() || next != self.listeners;
        if self.revision == Some(revision) && changed {
            return Err(SnapshotError::ConflictingRevision);
        }
        self.revision = Some(revision);
        self.listeners = next;
        Ok(if changed {
            Change::Changed
        } else {
            Change::Unchanged
        })
    }
}

mod forwards;
pub use forwards::{
    ApplyError, CloseAction, Completion, CompletionError, ForwardError, LeaseId, LoopbackEndpoint,
    Ports, ReleaseReason,
};
