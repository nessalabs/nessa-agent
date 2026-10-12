//! Whether a launch lets the harness run its own hooks.
//!
//! ```text
//! caller -> NativeHooksSwitch -> one open reads choice_for(session)
//!                             -> later opens of that session reuse it
//! ```
//!
//! The arrow is one read for one conversation open. The switch defaults off.
//! Off keeps today's suppression request. On omits that request and, for
//! Claude, loads the same setting sources the Claude CLI loads. It is not a
//! negotiated fact. A conversation that has already opened keeps the choice
//! from that open; a later change applies to a conversation that has not.

#![deny(missing_docs)]

use std::collections::HashMap;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex,
};

/// Shared launch choice for one harness, and the choice each conversation
/// already opened with.
///
/// Clones share the flag and the per-conversation memory. A change to the
/// flag is visible to the next conversation that has not opened yet. A
/// conversation that has opened keeps the value recorded for its session.
#[derive(Clone)]
pub struct NativeHooksSwitch {
    allowed: Arc<AtomicBool>,
    launches: Arc<Mutex<HashMap<String, bool>>>,
    /// When set, a conversation that is not already recorded stays off.
    ///
    /// The launch file could not be read, or the last snapshot could not be
    /// written. Granting the flag then would turn hooks on for a conversation
    /// that may have opened with them off. A successful snapshot clears it.
    withhold: Arc<AtomicBool>,
    /// Writes the remembered choices after a new session is recorded.
    /// The map lock is not held. The writer serializes its own snapshot.
    /// The default does nothing.
    persist: Arc<dyn Fn() + Send + Sync>,
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

    /// The next new conversation omits the suppression request.
    #[cfg(test)]
    pub fn on() -> Self {
        Self::from_shared(Arc::new(AtomicBool::new(true)))
    }

    /// Share a flag the settings store already owns.
    pub fn from_shared(allowed: Arc<AtomicBool>) -> Self {
        Self::sharing(
            allowed,
            Arc::new(Mutex::new(HashMap::new())),
            Arc::new(AtomicBool::new(false)),
            Arc::new(|| {}),
        )
    }

    /// Share the flag, the conversations already opened, whether an unknown
    /// conversation must stay off, and how a new memory is written down.
    pub fn sharing(
        allowed: Arc<AtomicBool>,
        launches: Arc<Mutex<HashMap<String, bool>>>,
        withhold: Arc<AtomicBool>,
        persist: Arc<dyn Fn() + Send + Sync>,
    ) -> Self {
        Self {
            allowed,
            launches,
            withhold,
            persist,
        }
    }

    /// Whether a conversation that has not opened yet should omit suppression.
    pub fn allowed(&self) -> bool {
        self.allowed.load(Ordering::Relaxed)
    }

    /// The choice for this conversation open.
    ///
    /// `session` is the conversation's session id. The first open records the
    /// flag's value then, or off when the launch memory is untrusted. A later
    /// open of the same session returns that record, including after the flag
    /// changes. No session id reads the flag and records nothing: a launch
    /// that names no conversation cannot be told apart from the next one.
    pub fn choice_for(&self, session: Option<&str>) -> bool {
        let Some(session) = session.filter(|session| !session.is_empty()) else {
            return self.allowed();
        };
        let mut launches = self
            .launches
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(stored) = launches.get(session).copied() {
            return stored;
        }
        let allowed = self.allowed() && !self.withhold.load(Ordering::Acquire);
        launches.insert(session.to_owned(), allowed);
        drop(launches);
        (self.persist)();
        allowed
    }
}
