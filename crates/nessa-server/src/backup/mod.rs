//! The parts of the export cut that meet the conversation context: the
//! export sources over its stores ([`infrastructure::capture_namespace`]),
//! its admission lock, and the committed-fence capture. The cut itself,
//! verification and restore are the `nessa-backup` crate.

pub mod infrastructure;

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
#[path = "../../tests/backup/restore.rs"]
mod restore_tests;

#[cfg(test)]
#[path = "../../tests/backup/gateway_restore.rs"]
mod gateway_restore_tests;
