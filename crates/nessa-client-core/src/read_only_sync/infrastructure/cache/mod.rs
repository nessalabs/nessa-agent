//! One private cache connection owns catalogue progress, record progress and semantic checkpoints.
//!
//! ```text
//! core CommitPlan -> scope/D CAS -> SDK borrowed guard -> records/D/A/checkpoint tx
//!                                                    |
//!                                              commit guard, update progress
//! ```
//! Arrows are calls. The active borrowed guard commits after the SQLite commit. Restart
//! restores a terminal checkpoint and only its contiguous downloaded suffix.
//! `offline` selects actual saved scopes; `catalogue_rows` admits descriptors,
//! retained fences and payloads before producing passive application metadata.
//! Raw catalogue payload bytes remain the stored representation.
//! `reset` commits explicit local intent, prior/new progress and audit receipt
//! with the same transaction; deletion fences and reset history are retained.
//! `purge` deletes one receiver's rows after an authenticated Terminal status,
//! with its receipt in the same transaction; that receipt fences the receiver.

mod catalogue;
mod catalogue_reset;
mod catalogue_rows;
mod offline;
mod purge;
mod raw_records;
mod records;
mod reset;
mod rows;
pub(crate) use records::ReadOnlyCache;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/infrastructure/cache.rs"]
mod tests;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/infrastructure/catalogue.rs"]
mod catalogue_tests;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/infrastructure/offline.rs"]
mod offline_tests;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/infrastructure/driver.rs"]
mod driver_tests;

// It drives the example's command line, which `cli` builds.
#[cfg(all(test, feature = "cli"))]
#[path = "../../../../tests/read_only_sync/infrastructure/offline_commands.rs"]
mod offline_command_tests;

// It drives the example's command line, which `cli` builds.
#[cfg(all(test, feature = "cli"))]
#[path = "../../../../tests/read_only_sync/infrastructure/reset_commands.rs"]
mod reset_command_tests;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/infrastructure/purge.rs"]
mod purge_tests;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/infrastructure/fixtures.rs"]
mod fixtures;

#[cfg(test)]
#[path = "../../../../tests/read_only_sync/infrastructure/allocations.rs"]
mod allocations;

// It drives the example's command line, which `cli` builds.
#[cfg(all(test, feature = "cli"))]
#[path = "../../../../tests/read_only_sync/infrastructure/saved_output.rs"]
mod saved_output_tests;
