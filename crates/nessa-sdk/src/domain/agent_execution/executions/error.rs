//! Typed rejection reasons for pending invocation admission.
#![deny(missing_docs)]

use std::{error::Error, fmt};

/// Rejection of queue construction or invocation admission.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SchedulingError {
    /// The identity was previously admitted during this queue's lifetime.
    Duplicate,
    /// The number of pending invocations already equals the configured capacity.
    Full,
    /// Capacity was zero; at least one pending invocation must be allowed.
    InvalidCapacity,
}

impl fmt::Display for SchedulingError {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str(match self {
            Self::Duplicate => "invocation identity was already admitted",
            Self::Full => "pending invocation queue is full",
            Self::InvalidCapacity => "pending invocation capacity must be positive",
        })
    }
}

impl Error for SchedulingError {}
