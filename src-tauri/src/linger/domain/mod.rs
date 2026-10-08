//! What setup may claim about logged-out operation.
//!
//! ```text
//! read × call ──show()──▶ shown
//! ```
//!
//! `show` is the claim. A call that returned is not a later process's read.

mod show;

pub(crate) use show::{
    show, LingerCall, LingerObservation, LingerShown, LingerSnapshot, LoginUserId,
};
