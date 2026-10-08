//! What setup may claim about logged-out operation, and the records of a choice.
//!
//! ```text
//! observation × attempt ──show()──▶ shown
//! accept / decline ──▶ intent, outcome, decline
//! ```
//!
//! `show` is the claim. A record is evidence of a choice this process made;
//! it is not a later process's observation.

mod decision;
mod show;

pub(crate) use decision::{
    LingerCause, LingerCorrelation, LingerDecline, LingerInitiator, LingerIntent, LingerOutcome,
};
pub(crate) use show::{
    show, LingerAttempt, LingerCall, LingerObservation, LingerShown, LingerSnapshot, LoginUserId,
};
