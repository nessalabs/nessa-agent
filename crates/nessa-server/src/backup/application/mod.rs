//! Run one cut: join admission, read every store, append fences, publish
//! the mac, and release admission on every path.

mod deletion_inventory;
mod export;
mod ports;

pub use deletion_inventory::{capture_fences, InventoryFailure};
pub use export::{capture_export, ExportFailure, ExportRequest};
pub use ports::{
    AdmissionFailure, AdmissionLease, Authenticator, DeletionLogStore, ExportClock, ExportSource,
    FenceSource, MacKey, ManifestPublisher, MutationAdmission, PublishedRecord,
};
