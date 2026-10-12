//! Whether a launch lets the harness run its own hooks.
//!
//! ```text
//! caller -> NativeHooksSwitch -> launch reads allowed()
//! ```
//!
//! The arrow is a read at process start. The switch defaults off. Off keeps
//! today's suppression request. On omits that request. It is not a negotiated
//! fact and not a test-only environment variable.

#![deny(missing_docs)]

use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

/// Shared launch choice for one harness.
///
/// Clones share the flag. A change is visible to the next launch without
/// rebuilding the provider. The flag is read when the process is started.
#[derive(Clone, Debug)]
pub struct NativeHooksSwitch {
    allowed: Arc<AtomicBool>,
}

impl Default for NativeHooksSwitch {
    fn default() -> Self {
        Self::off()
    }
}

impl NativeHooksSwitch {
    /// Suppression stays on. This is the launch every binding starts with.
    pub fn off() -> Self {
        Self {
            allowed: Arc::new(AtomicBool::new(false)),
        }
    }

    /// The next launch omits the suppression request.
    pub fn on() -> Self {
        Self {
            allowed: Arc::new(AtomicBool::new(true)),
        }
    }

    /// Share a flag the settings store already owns.
    pub fn from_shared(allowed: Arc<AtomicBool>) -> Self {
        Self { allowed }
    }

    /// Whether the next launch should omit suppression.
    pub fn allowed(&self) -> bool {
        self.allowed.load(Ordering::Relaxed)
    }

    /// Change what the next launch does. A process already running is unchanged.
    pub fn set(&self, allowed: bool) {
        self.allowed.store(allowed, Ordering::Relaxed);
    }
}
