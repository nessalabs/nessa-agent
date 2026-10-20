//! Immutable local cache reset intent; core types own scope and identity syntax.
//! A reset stays within one receiver/origin/conversation consistency boundary.
//! `held_file` names a file a conversation holds and the ranges it is read in;
//! the SDK's digest and media type values own their syntax.
//! `link` is the weak-link schedule: who may copy next, and whether a metered
//! quote allows it. It does not read a clock or a socket.

mod held_file;
mod link;
mod reset;
#[cfg(test)]
pub(crate) use held_file::MAX_HELD_FILE_BYTES;
pub(crate) use held_file::{HeldFile, HeldRange, HELD_FILE_RANGE_BYTES};
pub(crate) use link::{
    Admit, Billing, Candidate, Deferral, Interest, LinkSchedule, LinkSpec, Presence, Quote, Route,
    Settle, Subscription,
};
pub(crate) use reset::CacheReset;
