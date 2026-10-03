//! Connection-local advisory registrations and bounded physical notice ownership.
//! Source handles own registration; passive admission owns authority; the generic
//! product writer owns acknowledgement, frame completion and absolute deadlines.

mod connection;
mod delivery;
mod owner;
mod registration;
pub(super) use delivery::{Notice, WatchDeliveries, WatchFrame};
pub(super) use registration::{WatchHandle, WatchSelector, WatchToken};

pub(super) use owner::{ProductWatchPermit, WatchOwners};

pub(super) use connection::{ConnectionWatches, WatchAcknowledgement, WatchOutcome, WatchReply};

pub use owner::WatchTaskFault;
