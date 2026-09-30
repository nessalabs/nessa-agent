//! Authenticated catalogue dispatch and core-validated wire conversion.
//!
//! ```text
//! dispatch -> ReadCatalogue -> admitted source
//!          -> wire -> core validator -> passive_read capped encoder
//! ```
//! Arrows are calls. The saved operation correlates returned source evidence;
//! the codec refuses invalid evidence before the writer can queue a frame.

mod dispatch;
pub(crate) mod wire;
pub(super) use dispatch::dispatch;
