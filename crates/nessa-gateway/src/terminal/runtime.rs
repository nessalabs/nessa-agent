//! The process's pty-runtime, made before any async runtime starts.
use super::*;
#[cfg(all(unix, feature = "terminals"))]
use std::path::PathBuf;
use std::sync::Arc;

#[cfg(all(unix, feature = "terminals"))]
static PREPARED: std::sync::OnceLock<Arc<pty_runtime::Runtime>> = std::sync::OnceLock::new();

/// Why [`prepare`] made no runtime. It runs before logging starts, so the
/// reason is kept here and logged by whoever composes the gateway.
static UNPREPARED: std::sync::OnceLock<String> = std::sync::OnceLock::new();

/// Build pty-runtime's runtime for this process. pty-runtime stages its
/// helper image by forking, so this runs on the main thread before any
/// async runtime starts, and only in a process that serves the gateway.
/// A runtime that cannot be built leaves terminals unavailable; the gateway
/// still starts.
pub fn prepare() {
    #[cfg(all(unix, feature = "terminals"))]
    {
        let options = pty_runtime::RuntimeOptions {
            max_sessions: 64,
            replay_bytes: 1024 * 1024,
            ..Default::default()
        };
        // Folders are checked by the gateway before a spawn (a channel's,
        // or an agent's); the runtime's own check only needs a root that
        // holds them all.
        match pty_runtime::Runtime::new(vec![PathBuf::from("/")], options) {
            Ok(runtime) => {
                let _ = PREPARED.set(Arc::new(runtime));
            }
            Err(error) => {
                let _ = UNPREPARED.set(format!("{error:?}"));
            }
        }
    }
}

/// Why [`prepare`] made no runtime, when it tried and failed.
pub fn unprepared_reason() -> Option<&'static str> {
    UNPREPARED.get().map(String::as_str)
}

/// The engine [`prepare`] made, when it made one.
pub fn prepared_engine() -> Option<Arc<dyn TerminalEngine>> {
    #[cfg(all(unix, feature = "terminals"))]
    {
        PREPARED.get().map(|runtime| {
            Arc::new(GhosttyEngine::new(Arc::clone(runtime))) as Arc<dyn TerminalEngine>
        })
    }
    #[cfg(not(all(unix, feature = "terminals")))]
    {
        None
    }
}
