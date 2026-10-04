//! Connection-local advisory registrations and bounded physical notice ownership.
//! Source handles own registration; passive admission owns authority; the generic
//! product writer owns acknowledgement, frame completion and absolute deadlines.

mod connection;
mod delivery;
mod owner;
mod registration;
pub(super) use delivery::{Notice, WatchDeliveries, WatchFrame};
pub(super) use registration::{WatchHandle, WatchRefusal, WatchSelector, WatchToken};

#[cfg(test)]
pub(super) use owner::tests::principal as watch_principal;
pub(super) use owner::{ProductWatchPermit, WatchOwners, WatchPrincipal};

pub(super) use connection::{ConnectionWatches, WatchAcknowledgement, WatchOutcome, WatchReply};

pub use owner::WatchTaskFault;
