//! Consistency owners for managed runtime installation state.
//!
//! ManagedInstallation owns current identity and reclamation progress together.
//! Application code persists its state before using any returned effect permit.

mod managed_installation;

pub use managed_installation::{
    AdmissionResult, ManagedInstallation, ManagedInstallationError, PendingReclamation,
    ReclamationObservation, ReclamationOperation, ReclamationUpdate, ReclamationWork,
    RemovalPermit,
};
