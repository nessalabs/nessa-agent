//! Authenticated catalogue dispatch and core-validated wire conversion.
//!
//! ```text
//! dispatch -> ReadCatalogue -> admitted source
//!          -> nessa_protocol catalogue_read codec -> core validator -> capped encoder
//! ```
//! Arrows are calls. The saved operation correlates returned source evidence;
//! the codec refuses invalid evidence before the writer can queue a frame.

mod dispatch;
pub(super) use dispatch::dispatch;
