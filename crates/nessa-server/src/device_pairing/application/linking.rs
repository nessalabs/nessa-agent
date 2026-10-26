//! Turning native linking on or off from Settings (`pairing.setLinking`).
//!
//! Whether native pairing runs is decided once, at startup, from `config.json`:
//! its listener, its key and the paired environments it serves are all
//! composed then. So a change is saved where startup reads it, and the
//! gateway starts again to apply it. The port is composition's
//! (`composition::native_linking`); the socket asks Cedar for
//! `credential.manage` before it is reached. Every change is audited
//! ([`LinkingAudit`]): a requested record before anything is locked, an
//! outcome record before the answer and before any restart, and, when it
//! restarts, how that restart ended.
use std::{future::Future, pin::Pin};

/// Saves the choice and applies it.
pub trait NativeLinking: Send + Sync {
    /// Turn native linking on or off for `caller`. A change the running
    /// gateway already matches restarts nothing.
    fn set(
        &self,
        caller: LinkingCaller,
        enabled: bool,
    ) -> Pin<Box<dyn Future<Output = Result<LinkingChange, LinkingError>> + Send + '_>>;
}

/// Who asked: the authenticated session's identity, as the audit names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkingCaller {
    pub organization_id: String,
    pub principal_id: String,
    pub credential_id: String,
}

/// The saved choice, and how it takes effect.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkingChange {
    pub enabled: bool,
    pub applies: LinkingApply,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkingApply {
    /// The running gateway already matches it.
    Unchanged,
    /// The gateway is starting again with it.
    Restarting,
}

/// Why the change was not answered as made.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkingError {
    /// Another change holds `config.json`'s lock, or the gateway is already
    /// restarting or stopping. Nothing was saved.
    Busy,
    /// Linking would listen where this machine cannot: the port is taken, or
    /// the address is not this machine's. Saving it would leave a gateway
    /// that cannot start. Nothing was saved.
    AddressInUse,
    /// `config.json` could not be read, checked or written. Nothing was saved.
    Unavailable,
    /// A record of the change could not be kept. Before the change, nothing
    /// was saved; after it, the choice is saved and nothing restarted, so it
    /// applies at the next start.
    AuditUnavailable,
}

/// One record of a change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct LinkingAuditRecord {
    /// Shared by a change's requested record and its outcome.
    pub operation_id: String,
    pub caller: LinkingCaller,
    /// What was asked.
    pub enabled: bool,
    /// Whether the run that answered serves native pairing.
    pub running: bool,
    pub phase: LinkingAuditPhase,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkingAuditPhase {
    /// Before the lock is taken. When it cannot be kept, nothing is done.
    Requested,
    /// Saved, and how it applies. `written` is false when `config.json`
    /// already said so and was left as it was. `moved` names a listen
    /// address the change replaced: what it said, and what it says now.
    Applied {
        applies: LinkingApply,
        written: bool,
        moved: Option<ListenMoved>,
    },
    /// Refused, and nothing was saved.
    Refused(LinkingError),
    /// How the restart it asked for ended.
    Restart(LinkingRestart),
}

/// A listen address a change replaced, kept with its outcome: Settings'
/// old loopback default, opened to the network.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ListenMoved {
    pub from: std::net::SocketAddr,
    pub to: std::net::SocketAddr,
}

/// How a change's restart ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LinkingRestart {
    /// The run drained and the process is replacing itself.
    Exec,
    /// The process could not replace itself; the run ended as a failure.
    ExecFailed,
    /// The run ended without starting again: a stop signal won, or the drain
    /// failed. The change stays saved for the next start.
    Cancelled,
}

/// The record could not be made durable.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LinkingAuditUnavailable;

/// Where changes are recorded.
pub trait LinkingAudit: Send + Sync {
    fn record(&self, record: &LinkingAuditRecord) -> Result<(), LinkingAuditUnavailable>;
}
