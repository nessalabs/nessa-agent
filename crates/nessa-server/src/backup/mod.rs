//! A consistent export cut of the gateway's stores, and the deletion
//! inventory captured beside it.
//!
//! ```text
//! capture ──▶ MutationAdmission.hold     (the conversation admission lock)
//!               │
//!               ├── ExportSource.capture     one per writer-inventory store
//!               ├── DeletionLog.append       fences already committed
//!               └── ManifestPublisher.put    only after the mac succeeds
//!             hold drops on every path
//! ```
//!
//! Arrows are calls one capture makes, in order. The domain decides what a
//! cut may contain. The application runs that order. Infrastructure reads
//! stores and writes the bundle. Restore and quarantine are not here.
//!
//! The ordering table is [the export cut](../../../../docs/design/export-cut.md).

pub mod application;
pub mod domain;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/backup/export.rs"]
mod export_tests;

#[cfg(test)]
#[path = "../../tests/backup/deletion.rs"]
mod deletion_tests;

#[cfg(test)]
#[path = "../../tests/backup/files.rs"]
mod file_tests;

#[cfg(test)]
#[path = "../../tests/backup/lease.rs"]
mod lease_tests;
