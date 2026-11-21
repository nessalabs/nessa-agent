//! The seams the hub runs on: the engine that starts terminals, the events
//! it reports back, and the clock.
use super::*;
use std::sync::Arc;

/// What an engine tells the hub about a terminal it runs.
pub trait EngineEvents: Send + Sync {
    /// The program ended.
    fn exited(&self, terminal: &str, status: Option<i32>);
}

/// A running terminal's engine side.
pub trait RunningTerminal: Send + Sync {
    /// Ask its thread; a thread that has gone ignores it.
    fn send(&self, command: EngineCommand);
}

/// What starts terminals. The Ghostty engine in a build with terminals.
pub trait TerminalEngine: Send + Sync {
    fn start(
        &self,
        id: &str,
        spec: &StartSpec,
        events: Arc<dyn EngineEvents>,
    ) -> Result<Box<dyn RunningTerminal>, TerminalError>;
}

/// Milliseconds since the epoch, injected so tests can fix it.
pub trait TerminalClock: Send + Sync {
    fn now_ms(&self) -> u64;
}

/// The system clock.
pub struct SystemTerminalClock;

impl TerminalClock for SystemTerminalClock {
    fn now_ms(&self) -> u64 {
        crate::timer::clock::wall_now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|elapsed| elapsed.as_millis() as u64)
            .unwrap_or(0)
    }
}
