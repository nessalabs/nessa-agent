//! Run one cut: hold admission while each store is pinned and fences are
//! appended, then release it and read, mac, and publish from those snapshots.

mod deletion_inventory;
mod export;
mod ports;

pub use deletion_inventory::{capture_fences, InventoryFailure};
pub use export::{capture_export, ExportFailure, ExportRequest};
pub use ports::{
    AdmissionFailure, AdmissionLease, Authenticator, CopiedStore, DeletionLogStore, ExportClock,
    ExportSource, FenceSource, LogStoreError, MacError, MacKey, ManifestPublisher,
    MutationAdmission, PinnedStore, PublishedRecord,
};
