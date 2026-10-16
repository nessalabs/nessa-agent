//! Native device enrollment and pinned recovery over the shared protocol socket.
//! The client owns private pending-state publication and one retained worker.
//! `owner_phone` prepares/restores an immutable original without network; Auth
//! owns reservation/private publication and canonical crypto.
//! `owner_recovery` uses that same combined private owner and existing physical
//! dispatcher for pinned compact recovery, explicit settlement and actual selected
//! record correlation. Its recovered public value has no external constructor.
//!
//! ```text
//! retained/device -> owner_recovery -> client::dispatch_owned -> pinned TLS
//!                         | -> Auth original/enrollment store
//!                         | -> actual selected record -> retained reader/product
//! ```
//! Arrows name calls, not new authority; gateway admission remains independent.
mod owner_phone;
pub use owner_phone::{
    prepare_owner_phone, restore_owner_phone, OwnerPhonePreparation, OwnerPhonePreparationError,
};
pub(crate) mod owner_recovery;
pub use owner_recovery::{
    current_owner_phone, issued_owner_phone, restore_expected_owner_phone, OwnerPhoneRecovered,
    OwnerPhoneRecoveryFailure, OwnerPhoneSettlement, OwnerPhoneSettlementFailure, OwnerPhoneState,
};
mod client;
pub use client::{NativeClientError, NativeEnrollmentClient, NativeRetryOutcome};
