//! Reusable agent input, tool observations, and permission decisions.
//! These concepts belong to Nessa, independent of ACP or a conversation store.
//!
//! application mapping --> value objects --> entities
//!
//! Arrows show construction: validated values form identity-bearing tools and
//! permission requests. No transport, authorization backend, or I/O lives here.
pub mod entities;
mod error;
pub mod value_objects;
pub use error::ExecutionError;
