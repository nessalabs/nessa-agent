//! Whether a launch lets the harness run its own hooks.
//!
//! ```text
//! caller -> NativeHooksSwitch.allowed() -> one process start
//!                                       -> the next start reads again
//! ```
//!
//! The arrow is one read for one process start. The switch defaults off.
//! Off keeps today's suppression request. On omits that request and, for
//! Claude, loads the same setting sources the Claude CLI loads. It is not a
//! negotiated fact. A process already running keeps the choice it started
//! with. The next start of that conversation, and of every other one, reads
//! the flag again.

#![deny(missing_docs)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Shared launch choice for one harness.
///
/// Clones share the flag. A change is visible to the next process start.
/// The start that reads it keeps that value for its own process.
#[derive(Clone)]
pub struct NativeHooksSwitch {
    allowed: Arc<AtomicBool>,
}

impl std::fmt::Debug for NativeHooksSwitch {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter
            .debug_struct("NativeHooksSwitch")
            .field("allowed", &self.allowed())
            .finish()
    }
}

impl Default for NativeHooksSwitch {
    fn default() -> Self {
        Self::off()
    }
}

impl NativeHooksSwitch {
    /// Suppression stays on. This is the launch every binding starts with.
    pub fn off() -> Self {
        Self::from_shared(Arc::new(AtomicBool::new(false)))
    }

    /// The next process start omits the suppression request.
    #[cfg(test)]
    pub fn on() -> Self {
        Self::from_shared(Arc::new(AtomicBool::new(true)))
    }

    /// Share a flag the settings store already owns.
    pub fn from_shared(allowed: Arc<AtomicBool>) -> Self {
        Self { allowed }
    }

    /// The choice a process start should use.
    ///
    /// One start reads this once and keeps the bool. A later start reads it
    /// again, so a change reaches a conversation that is already open the
    /// next time its agent process starts.
    pub fn allowed(&self) -> bool {
        self.allowed.load(Ordering::Relaxed)
    }
}
