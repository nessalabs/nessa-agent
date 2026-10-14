//! The deletion inventory, asked after a fence has committed.
//!
//! ```text
//! delete | unshare | revoke ──▶ fence committed
//!                                  │
//!                                  └── FenceCapture::capture
//!                                        ├── stored
//!                                        └── refused or not stored: the fence stays
//! ```
//!
//! Arrows are calls. The command's result does not wait on an upload and does
//! not change when capture fails. Who may capture is decided by the backup
//! context's Cedar policy, inside the adapter, not here.

use async_trait::async_trait;

/// Reconcile the deletion inventory with fences already committed.
#[async_trait]
pub trait FenceCapture: Send + Sync {
    /// Read the committed fences and append any the log does not yet hold.
    ///
    /// [`FenceCaptureError::Refused`] writes nothing. [`FenceCaptureError::NotStored`]
    /// leaves the previous log. Neither undoes the fence.
    async fn capture(&self) -> Result<(), FenceCaptureError>;
}

/// Why the inventory was not updated. The fence is unchanged.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FenceCaptureError {
    /// Cedar refused the record. Nothing was written.
    Refused,
    /// The log could not be read or replaced.
    NotStored,
}
