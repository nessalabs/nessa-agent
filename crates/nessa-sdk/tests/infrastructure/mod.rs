//! External parsing and storage integration tests.
//!
//! ```text
//! infrastructure -> session storage leases / private snapshots
//! infrastructure -> metadata JSON
//! ACP contracts live in acp/ but the library includes them through a test-only
//! path declaration for crate-private controls; do not declare them here.
//! ```
//! Arrows show which test layer exercises each feature.

mod model_metadata;
mod session_storage;
