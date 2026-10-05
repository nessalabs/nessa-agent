//! Native device enrollment and pinned recovery over the shared protocol socket.
//! The client owns private pending-state publication and one retained worker.
mod client;
pub use client::{NativeClientError, NativeEnrollmentClient, NativeRetryOutcome};
