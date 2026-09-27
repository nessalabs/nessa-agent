//! Identity-bearing state for an install while it is running.
//!
//! `InstallAttempt` is the sole owner of the evidence sequence:
//!
//! ```text
//! started -> verified -> installed | replaced | rolled back | recovery incomplete
//!        \-> digest rejected
//! ```
//! Arrows mean a transition the entity permits. Each row occupies a stable
//! event slot, so the same owner also admits restored facts, accepts exact
//! replay, and rejects a different fact in an occupied slot. Effects remain in
//! application and infrastructure.
mod install_attempt;

pub use install_attempt::{InstallAttempt, InstallAttemptError, InstallEventAdmission};
