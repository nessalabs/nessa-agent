//! Durable representations and audit records for superseded runtime removal.
//!
//! `ManagedInstallation -> record -> private JSON` maps infrastructure data
//! through domain constructors so restored work cannot bypass invariants.
mod record;

pub(in crate::agent_install::infrastructure) use record::StoredManagedInstallation;
