//! Immutable local cache reset intent; core types own scope and identity syntax.
//! A reset stays within one receiver/origin/conversation consistency boundary.

mod reset;
pub(crate) use reset::CacheReset;
