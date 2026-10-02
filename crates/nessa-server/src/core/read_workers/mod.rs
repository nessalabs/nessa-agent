//! Tracked blocking reads and retained shutdown completion across source adapters.
//!
//! Record and catalogue infrastructure call this owner for admission, physical
//! worker tracking, unexpected faults and join-all drain. Each admitted closure
//! keeps its own source lease; this owner does not allocate socket capacity or
//! decide authorization. Attachment range reads can consume the same lifecycle.
//!
//! ```text
//! source adapter -> ReadWorkers::run -> owned OS thread + result
//! source shutdown -> retained drain -> all actual thread joins
//! ```
//! Arrows are calls. Cancelled observers leave the original physical owners live.
mod workers;
pub(crate) use workers::{ReadWorkerError, ReadWorkers};

#[cfg(test)]
#[path = "../../../tests/core/read_workers.rs"]
mod tests;
