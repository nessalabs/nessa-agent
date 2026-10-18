//! A consistent export cut of the gateway's stores, and the deletion
//! inventory captured beside it.
//!
//! ```text
//! capture ──▶ MutationAdmission.hold     (conversation admission, write)
//!               │
//!               ├── ExportSource.pin         watermark, sqlite snapshot, small files,
//!               │                            and both outboxes' rows
//!               └── DeletionLog.append       fences already committed
//!             hold drops
//!               ├── PinnedStore.read         records from those snapshots
//!               └── ManifestPublisher.put    only after the mac succeeds
//!
//! committed fence ──▶ FenceCapture::capture
//!                       ├── Cedar: who may write
//!                       ├── decision audit
//!                       └── reconciliation lease  no admission, no bundle
//!                             ├── load and read current fences
//!                             └── no-op or DeletionLog replacement
//! ```
//!
//! Arrows are calls, in order. The export path is one capture. The fence
//! path is a deletion, unshare, credential revocation, or pairing
//! cancellation, a successful share retry, or startup after offline rotation:
//! it asks Cedar, stores that
//! decision, then reconciles the log. It does not take admission and does
//! not publish a bundle. The domain decides what a cut
//! may contain. The application runs that order. Infrastructure reads
//! stores and writes the bundle. Export takes admission and never a per-grant
//! gate. Restore and quarantine are not here.
//!
//! The ordering table is [the export cut](../../../../docs/design/export-cut.md).
//! The standalone reader enters through `entrypoint`, verifies through
//! `application::verify_bundle`, and reads only supplied bundle/key paths through
//! infrastructure. Its contract is [backup verification](../../../../docs/design/backup-verification.md).

pub mod application;
pub mod domain;
pub mod entrypoint;
pub mod infrastructure;

#[cfg(test)]
#[path = "../../tests/backup/verification.rs"]
mod verification_tests;

#[cfg(test)]
#[path = "../../tests/backup/export.rs"]
mod export_tests;

#[cfg(test)]
#[path = "../../tests/backup/deletion.rs"]
mod deletion_tests;

#[cfg(test)]
#[path = "../../tests/backup/capture.rs"]
mod capture_tests;

#[cfg(test)]
#[path = "../../tests/backup/files.rs"]
mod file_tests;

#[cfg(test)]
#[path = "../../tests/backup/lease.rs"]
mod lease_tests;

#[cfg(test)]
#[path = "../../tests/backup/verification_format.rs"]
mod verification_format_tests;

#[cfg(test)]
#[path = "../../tests/backup/verification_directory.rs"]
mod verification_directory_tests;

#[cfg(test)]
#[path = "../../tests/backup/verification_metadata.rs"]
mod verification_metadata_tests;
