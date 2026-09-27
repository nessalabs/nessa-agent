//! Local credential authentication and administration.
//!
//! ```text
//! server composition -> registry -> private registry file
//! authentication -----> registry -> verification snapshots
//! ```
//! Arrows show calls into the registry, which owns the filesystem lock and keeps
//! each read or mutation inside one transaction boundary.

mod registry;
mod registry_refusal_audit;

pub use registry::{
    write_evidence_file, BootstrapOutcome, BootstrapRequest, LocalCredentialStore, LocalIdentity,
    LocalStoreConfig, LocalStoreError,
};
pub use registry_refusal_audit::DurableCredentialRegistryRefusalAudit;
