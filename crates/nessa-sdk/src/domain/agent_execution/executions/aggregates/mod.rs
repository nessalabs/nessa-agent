//! Owns pending admission, priority, capacity, and once-only invocation identities.
//! The application retains this aggregate for its agent lifetime and owns running
//! execution, persistence, and effects outside this consistency boundary.
//!
//! ```text
//! application runner -> InvocationQueue -> execution values
//!                           |-> steering FIFO (dispatch first)
//!                           |-> ordinary FIFO
//!                           +-> admitted identity history
//! ```
//! The runner depends on scheduling decisions; the queue owns both pending
//! FIFOs and their shared capacity and identity history.
#![deny(missing_docs)]

mod invocation_queue;
pub use invocation_queue::InvocationQueue;
