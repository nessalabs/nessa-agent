//! Identity-bearing state for an install while it is running.
//!
//! `InstallAttempt` is the sole owner of the evidence sequence:
//!
//! ```text
//! started -> verified -> installed | replaced | rolled back
//!        \-> digest rejected
//! ```
//! Arrows mean a transition the entity permits. Effects remain in application
//! and infrastructure; this module only prevents impossible histories.
mod install_attempt;

pub use install_attempt::{InstallAttempt, InstallAttemptError};
