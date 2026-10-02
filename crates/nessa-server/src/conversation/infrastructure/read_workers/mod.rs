//! Shared tracked blocking reads and retained shutdown completion.
//!
//! Record and catalogue sources call this owner for admission, worker tracking,
//! unexpected faults and join-all drain. The admitted closure owns its lease.
mod workers;
pub(super) use workers::{ReadWorkerError, ReadWorkers};
