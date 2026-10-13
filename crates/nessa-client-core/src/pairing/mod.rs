//! Native device enrollment and pinned recovery over the shared protocol socket.
//! The client owns private pending-state publication and one retained worker.
//! `owner_phone` prepares/restores an immutable original without network; Auth
//! owns reservation/private publication and canonical crypto.
mod owner_phone;
pub use owner_phone::{
    prepare_owner_phone, restore_owner_phone, OwnerPhonePreparation, OwnerPhonePreparationError,
};
mod client;
pub use client::{NativeClientError, NativeEnrollmentClient, NativeRetryOutcome};
