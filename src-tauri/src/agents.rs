//! Whether a coding agent can actually run here, and why not when it cannot.
//!
//! Setup offers a choice of agent, and an offer the runtime cannot honour is
//! worse than no offer: it fails later, somewhere the person cannot connect to
//! the choice they made. So the choice is made against what is installed and
//! signed in, asked at the moment it is offered rather than assumed.
//!
//! Nothing here reads a secret. It asks whether a credential exists, never
//! what it is — knowing that Claude is signed in requires no access to the
//! token, and an app that reads one has to be trusted with it.

use std::path::PathBuf;

use serde::Serialize;
use tauri::{AppHandle, Manager};

/// Where Claude Code keeps its sign-in on macOS. Not read — only asked after.
const CLAUDE_KEYCHAIN_SERVICE: &str = "Claude Code-credentials";

/// What stands between an agent and running.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "kebab-case")]
pub enum Readiness {
    /// Installed and signed in. The only state that may be offered.
    Ready,
    /// Installed, but nothing here is signed in to it.
    NeedsAuthentication,
    /// Not installed, or Nessa has no adapter for it.
    Unavailable,
}

/// Every agent Nessa knows how to ask about, and what it found.
#[derive(Debug, Clone, Serialize)]
pub struct AgentsReadiness {
    claude: Readiness,
    codex: Readiness,
}

/// The bundled agent runtime, wherever this build keeps it.
fn runtime_root(app: &AppHandle) -> Option<PathBuf> {
    if let Ok(resources) = app.path().resource_dir() {
        let bundled = resources.join("runtime");
        if bundled.is_dir() {
            return Some(bundled);
        }
    }
    // A dev build runs from the workspace rather than a bundle, and the
    // harness sits beside the crate. A shipped binary never finds this.
    if cfg!(debug_assertions) {
        let checkout = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("runtime");
        if checkout.is_dir() {
            return Some(checkout);
        }
    }
    None
}

/// Whether the Claude adapter is installed with this build.
fn claude_installed(app: &AppHandle) -> bool {
    runtime_root(app).is_some_and(|root| root.join("claude-acp").is_dir())
}

/// Whether anything on this machine is signed in to Claude.
///
/// Three places, because Claude Code itself accepts three. An API key in the
/// environment is what a machine account uses; the credentials file is what
/// Claude Code writes where a keychain is not available, and is honoured
/// through `CLAUDE_CONFIG_DIR` the same way Claude Code honours it; and on
/// macOS the sign-in is a keychain item, which is asked about through the
/// platform seam.
fn claude_authenticated() -> bool {
    if std::env::var("ANTHROPIC_API_KEY").is_ok_and(|key| !key.trim().is_empty()) {
        return true;
    }
    let config = std::env::var("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .ok()
        .or_else(|| {
            std::env::var(if cfg!(windows) { "USERPROFILE" } else { "HOME" })
                .ok()
                .map(|home| PathBuf::from(home).join(".claude"))
        });
    if config.is_some_and(|dir| {
        dir.join(".credentials.json")
            .metadata()
            .is_ok_and(|file| file.len() > 0)
    }) {
        return true;
    }
    crate::platform::current().has_stored_credential(CLAUDE_KEYCHAIN_SERVICE)
}

/// What each agent's runtime reports, right now.
pub fn readiness(app: &AppHandle) -> AgentsReadiness {
    let claude = if !claude_installed(app) {
        Readiness::Unavailable
    } else if claude_authenticated() {
        Readiness::Ready
    } else {
        Readiness::NeedsAuthentication
    };
    AgentsReadiness {
        claude,
        // Listed so the roadmap is part of the choice, and honest about there
        // being no adapter for it yet.
        codex: Readiness::Unavailable,
    }
}

#[tauri::command]
pub fn agents_readiness(app: AppHandle) -> AgentsReadiness {
    readiness(&app)
}
