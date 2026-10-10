//! The gateway's own evidence about its SSH environments: each connection,
//! each refusal by version or business, each connection lost, each frame
//! dropped because it named no lease or harness the gateway holds (row L9),
//! and each install of this build on a host, begun, done or refused.
//! One immutable JSON file per record, like the gateway's other audits, with
//! when it was observed.
use serde::Serialize;
use std::io;

/// One record of what happened between this gateway and a host.
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(
    tag = "kind",
    rename_all = "snake_case",
    rename_all_fields = "camelCase"
)]
pub(crate) enum EnvironmentEvent {
    /// A host answered with this lease protocol's hello.
    Connected { host: String, workspace: String },
    /// A host answered with another lease protocol's hello, or none: refused.
    /// `build` and `protocol` are what its hello said, if it was one.
    VersionRefused {
        host: String,
        build: Option<String>,
        protocol: Option<String>,
    },
    /// A host is serving another connection.
    Busy { host: String },
    /// A host could not serve leases at all: its configuration.
    NotConfigured { host: String },
    /// A connection ended, ending every lease it carried.
    ConnectionLost { host: String, leases: Vec<String> },
    /// A frame named a lease or harness this connection does not hold, or
    /// was not one this build reads, and was dropped.
    FrameDropped {
        host: String,
        lease: Option<String>,
        channel: Option<u32>,
        frame: String,
    },
    /// A harness wrote more than the gateway queues for its binding; its
    /// output was ended and the harness stopped.
    OutputOverflow {
        host: String,
        lease: String,
        channel: u32,
    },
    /// This build is about to be sent to a host that has no copy of it,
    /// to be installed under `protocol` if its SHA-256 there is `digest`.
    /// Recorded before a byte is sent. `lease` is the lease whose opening
    /// found no copy, in every install record: it joins them to its
    /// issuance and its actor, and pairs this start with its outcome, as a
    /// lease opens at most one install.
    InstallStarted {
        host: String,
        lease: String,
        protocol: String,
        digest: String,
    },
    /// It verified on the host and is in place.
    Installed {
        host: String,
        lease: String,
        protocol: String,
        digest: String,
    },
    /// The upload's answer was lost, and the host's probe then found a copy
    /// of this protocol in place: this upload's or another gateway's of the
    /// same protocol, so no digest is claimed for it.
    InstallFound {
        host: String,
        lease: String,
        protocol: String,
    },
    /// Nothing was installed, and why; `seen` is what the host said, where
    /// that is the reason (its platform, the digest it saw, the protocol the
    /// copy spoke).
    InstallRefused {
        host: String,
        lease: String,
        reason: InstallRefusal,
        seen: Option<String>,
    },
}

/// Why this build was not installed on a host.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub(crate) enum InstallRefusal {
    /// The host runs on a system or processor this build was not made for.
    Platform,
    /// This build's own executable could not be read here.
    Source,
    /// What arrived on the host is not what was sent.
    Fingerprint,
    /// What arrived runs, and speaks another lease protocol.
    Version,
    /// What arrived does not run on the host.
    Unrunnable,
    /// The host has no tool to take a SHA-256 with.
    DigestTool,
    /// A step on the host failed: a directory, a file, its mode, the rename.
    Failed,
    /// The host's answer did not come, or was not one of these.
    Unanswered,
}

/// Where those records go.
pub(crate) trait EnvironmentAudit: Send + Sync {
    /// Record `event` durably.
    ///
    /// # Errors
    /// The record is not durable.
    fn record(&self, event: &EnvironmentEvent) -> io::Result<()>;
}
