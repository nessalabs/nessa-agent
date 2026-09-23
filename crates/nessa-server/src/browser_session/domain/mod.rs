//! Browser-session state and immutable values, independent of cookies and persistence.
//!
//! Boundary text -> validated origin/lifetime -> authoritative session state
//!
//! The arrows show infrastructure mapping outside data into domain values before
//! the application coordinates storage or credential verification.
pub mod value_objects;
