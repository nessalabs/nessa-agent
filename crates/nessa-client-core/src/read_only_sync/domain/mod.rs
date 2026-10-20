//! Immutable local cache reset intent; core types own scope and identity syntax.
//! A reset stays within one receiver/origin/conversation consistency boundary.
//! `held_file` names a file a conversation holds and the ranges it is read in;
//! the SDK's digest and media type values own their syntax.

mod held_file;
mod reset;
pub(crate) use held_file::{HeldFile, HeldRange, MAX_HELD_FILE_BYTES};
#[cfg(test)]
pub(crate) use held_file::HELD_FILE_RANGE_BYTES;
pub(crate) use reset::CacheReset;
