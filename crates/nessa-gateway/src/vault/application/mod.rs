//! Verified migration receipts; source adapters retain their cross-process locks.
mod migration;
pub use migration::{verify_and_retire, Migration, MigrationSnapshot};
