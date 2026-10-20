//! Publication lease for one phone scope.
//!
//! The lease does not enroll, fold, or purge. It names the receiver a saved
//! read was admitted under. A later bind, successful purge, or close advances
//! the generation, and the previous stamp reads nothing.
//!
//! ```text
//! bind(receiver) --> stamp
//! accept(stamp) --same lease, generation, and receiver--> read the existing cache
//!               --anything else------------------------> no bytes
//! release_receiver --> unbound, a later bind may admit the next receiver
//! close -------------> closed
//! ```
//!
//! Arrows are the lease's own transitions. Catalogue presence reads one stored
//! identity from the existing cache. A missing row is not a tombstone.

use super::{cache_failure, ReadFailure, RetainedCache};
use nessa_auth::domain::ResourceId;
use nessa_sync::replication::domain::Id;
use uuid::Uuid;

pub use crate::read_only_sync::application::CataloguePresence;

/// Why a stamp cannot publish.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PhoneFence {
    /// No receiver is bound.
    Unbound,
    /// The session was closed.
    Closed,
    /// A newer receiver or a released binding replaced this stamp.
    Replaced,
}

/// The lease, receiver, and generation a publication was admitted under.
/// Reconstructing one does not bind the lease. Two leases do not share an
/// instance, so a generation that starts again still fails `accept`.
#[derive(Clone, Debug, Eq, PartialEq)]
pub struct PhoneStamp {
    lease: Uuid,
    generation: u64,
    receiver: String,
}
impl PhoneStamp {
    /// A stamp value. It publishes nothing until the lease that minted `lease` accepts it.
    pub fn issue(lease: Uuid, generation: u64, receiver: impl Into<String>) -> Self {
        Self {
            lease,
            generation,
            receiver: receiver.into(),
        }
    }
    /// Lease instance this stamp was minted for.
    pub fn lease(&self) -> Uuid {
        self.lease
    }
    /// Generation assigned when this receiver was bound.
    pub fn generation(&self) -> u64 {
        self.generation
    }
    /// Receiver this stamp names.
    pub fn receiver(&self) -> &str {
        &self.receiver
    }
}

/// One phone scope's publication generation.
#[derive(Debug)]
pub struct PhoneLease {
    lease: Uuid,
    generation: u64,
    receiver: Option<String>,
    closed: bool,
}
impl PhoneLease {
    /// Unbound and open. Nothing can publish yet. Each call mints a new instance,
    /// so a later session does not accept an earlier session's stamp.
    pub fn new() -> Self {
        Self {
            lease: Uuid::new_v4(),
            generation: 0,
            receiver: None,
            closed: false,
        }
    }
    /// The stamp a caller must present, when a receiver is bound.
    pub fn stamp(&self) -> Result<PhoneStamp, PhoneFence> {
        if self.closed {
            return Err(PhoneFence::Closed);
        }
        match &self.receiver {
            Some(receiver) => Ok(PhoneStamp {
                lease: self.lease,
                generation: self.generation,
                receiver: receiver.clone(),
            }),
            None => Err(PhoneFence::Unbound),
        }
    }
    /// Admit `receiver`. The same receiver keeps its generation. A different
    /// one advances the generation, so the previous stamp fails `accept`.
    pub fn bind(&mut self, receiver: &str) -> Result<PhoneStamp, PhoneFence> {
        if self.closed {
            return Err(PhoneFence::Closed);
        }
        if self.receiver.as_deref() != Some(receiver) {
            self.generation = self.generation.saturating_add(1);
            self.receiver = Some(receiver.to_owned());
        }
        Ok(PhoneStamp {
            lease: self.lease,
            generation: self.generation,
            receiver: receiver.to_owned(),
        })
    }
    /// A successful terminal purge dropped this receiver. Old stamps fail.
    /// The session stays open so a later enrollment can bind.
    pub fn release_receiver(&mut self) {
        if self.closed || self.receiver.is_none() {
            return;
        }
        self.generation = self.generation.saturating_add(1);
        self.receiver = None;
    }
    /// Session close. Later binds stay closed.
    pub fn close(&mut self) {
        if self.closed {
            return;
        }
        self.closed = true;
        self.generation = self.generation.saturating_add(1);
        self.receiver = None;
    }
    /// Whether `stamp` may read this scope's current receiver.
    pub fn accept(&self, stamp: &PhoneStamp) -> Result<(), PhoneFence> {
        if self.closed {
            return Err(PhoneFence::Closed);
        }
        match &self.receiver {
            Some(receiver)
                if self.lease == stamp.lease
                    && receiver == &stamp.receiver
                    && self.generation == stamp.generation =>
            {
                Ok(())
            }
            Some(_) => Err(PhoneFence::Replaced),
            None => Err(PhoneFence::Unbound),
        }
    }
}
impl Default for PhoneLease {
    fn default() -> Self {
        Self::new()
    }
}

impl RetainedCache {
    fn receiver_purged(&mut self, receiver: &str) -> Result<bool, ReadFailure> {
        let Ok(resource) = ResourceId::new(receiver) else {
            return Ok(false);
        };
        Ok(self
            .cache
            .purge_receipt(&resource)
            .map_err(|error| cache_failure(&error))?
            .is_some())
    }
    /// One conversation's stored catalogue identity. This reads the cache and
    /// does not contact the gateway. A conversation a page did not mention is
    /// left as it was stored.
    pub fn catalogue_presence(
        &mut self,
        receiver: &str,
        conversation: &str,
    ) -> Result<CataloguePresence, ReadFailure> {
        let receiver_id = Id::new(receiver).map_err(|_| ReadFailure::Protocol)?;
        let conversation_id = Id::new(conversation).map_err(|_| ReadFailure::Protocol)?;
        if self.receiver_purged(receiver)? {
            return Ok(CataloguePresence::Tombstone);
        }
        let Some(scope) = self.catalogue_scope(&receiver_id)? else {
            // A purge can commit after the check above and delete catalogue
            // progress in that same commit. Absence is only the state with no
            // scope and still no receipt.
            if self.receiver_purged(receiver)? {
                return Ok(CataloguePresence::Tombstone);
            }
            return Ok(CataloguePresence::Absent);
        };
        self.cache
            .catalogue_presence(
                &receiver_id,
                scope.origin(),
                scope.stream(),
                &conversation_id,
            )
            .map_err(|error| cache_failure(&error))
    }
}

#[cfg(test)]
#[path = "../../tests/retained/phone.rs"]
mod tests;
