//! The timer waits are measured on (issues #758 and #762): every deadline,
//! interval and wait in the environment role and in the relay is measured on
//! one shared [`Timer`], never on a private copy of the runtime clock.
//!
//! Wall-clock credential expiry and elapsed-only protocol clocks stay separate.
//! This module is the server's name for `nessa-timer`.

pub(crate) use nessa_timer::{within, Moment, Timer, TokioTimer};

/// A deadline measured from now. Only the Unix environment role calls it.
#[cfg(unix)]
pub(crate) use nessa_timer::timeout;

#[cfg(test)]
pub(crate) mod manual {
    pub(crate) use nessa_timer::manual::ManualTimer;
}
