//! Durable representations and audit records for superseded runtime removal.
//!
//! `ManagedInstallation -> record -> private JSON` maps infrastructure data
//! through domain constructors so restored work cannot bypass invariants.
mod audit;
mod record;

pub use audit::DurableReclamationAudit;
pub(in crate::agent_install::infrastructure) use record::{StoredEvent, StoredManagedInstallation};
