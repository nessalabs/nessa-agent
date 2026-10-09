//! Lease frames: what a gateway and an environment it runs agents in say to
//! each other over one byte stream, such as `ssh <host> nessa env serve`'s
//! standard input and output (ADR 252, `docs/design/runtime-architecture.md`,
//! "Three transports, one contract").
//!
//! ```text
//! environment ── Hello{build, workspace} ──▶ gateway   (first frame, always)
//! gateway ── Grant{lease, agent} ──▶ environment ── Granted | Refused ──▶ gateway
//! gateway ── Start{lease, channel, environment} ──▶ harness process on the host
//!   gateway ── Input / InputClosed ──▶ harness stdin
//!   gateway ◀── Output / OutputClosed ── harness stdout
//!   gateway ── Stop{grace, kill} ──▶ … ── Stopped{cleanup} ──▶ gateway
//! gateway ── End{lease} ──▶ environment ── Ended{cleanup} ──▶ gateway
//! gateway ── Account{lease} ──▶ environment ── Accounted{cleanup} ──▶ gateway
//! ```
//!
//! Arrows are frames, in the order the two ends exchange them. Every frame
//! after the hello names the lease it belongs to, and every frame about a
//! harness also names its channel: the gateway's number for one harness
//! process under that lease. A frame naming no lease the reader holds is
//! dropped with evidence by the reader, never applied to another lease.
//!
//! On the wire each frame is [`crate::pairing::encode_frame`]'s four-byte
//! big-endian length and a JSON body of at most [`MAX_FRAME_BYTES`]. A
//! harness's bytes travel base64-encoded, at most [`MAX_DATA_BYTES`] a frame.
//!
//! The hello is read by [`read_hello`], which looks only at its `type` and
//! `build`: an environment of another build is recognised as such whatever
//! else its frames carry, and is refused before anything is sent to it.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::collections::BTreeMap;

/// Most bytes one frame's body may have.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
/// Most harness bytes one [`ToEnvironment::Input`] or
/// [`FromEnvironment::Output`] carries; larger writes are split.
pub const MAX_DATA_BYTES: usize = 64 * 1024;

/// A harness's bytes, base64 on the wire.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Data(pub Vec<u8>);

impl Serialize for Data {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&STANDARD.encode(&self.0))
    }
}

impl<'de> Deserialize<'de> for Data {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let text = String::deserialize(deserializer)?;
        let bytes = STANDARD.decode(text).map_err(serde::de::Error::custom)?;
        if bytes.len() > MAX_DATA_BYTES {
            return Err(serde::de::Error::custom("data larger than a frame carries"));
        }
        Ok(Self(bytes))
    }
}

/// What releasing a harness, or everything a lease started, took, as the
/// environment saw it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase", deny_unknown_fields)]
pub enum Cleanup {
    /// The process tree is gone; `forced` says whether that took a signal.
    Confirmed {
        /// Whether termination had to be forced.
        forced: bool,
    },
    /// Nothing was ever started under it, or the environment has no record
    /// of the lease at all.
    NotHeld,
    /// The environment could not confirm the process tree is gone.
    Uncertain,
}

/// Why an environment serves no leases on this connection. Sent instead of
/// anything else after the hello, and the environment then exits.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Unavailability {
    /// Another connection is being served; one is served at a time.
    Busy,
    /// The host's own configuration could not be read.
    NotConfigured,
}

/// Why an environment refused one lease.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GrantRefusal {
    /// The host has no runtime configured for the agent, or none it can
    /// start there.
    AgentUnavailable,
    /// The lease id was already granted here, on this connection or an
    /// earlier one whose grant the host recorded, or it is not one a lease
    /// can have.
    Duplicate,
    /// The environment could not record the lease in its own audit, so it
    /// does not run it.
    AuditUnavailable,
}

/// Why a harness did not start.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum StartFailure {
    /// The lease is not granted, or the channel is already in use.
    NotGranted,
    /// The executable could not be started.
    SpawnFailed,
    /// The environment could not record the start in its own audit, so it
    /// started nothing.
    AuditUnavailable,
}

/// The first frame an environment sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// The environment's build, `CARGO_PKG_VERSION`; a gateway speaks only to
    /// its own build.
    pub build: String,
    /// The absolute directory on the host its harnesses work in.
    pub workspace: String,
}

/// What a gateway sends an environment.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum ToEnvironment {
    /// Admit a lease to run `agent`'s harness.
    Grant {
        /// The lease's id.
        lease: String,
        /// The agent, by its protocol name (`AgentId::name`).
        agent: String,
    },
    /// Start the lease's agent harness as `channel`. `environment` is what the
    /// binding sets for this launch; the host adds its own executable,
    /// arguments, account variables and credentials.
    Start {
        /// The lease's id.
        lease: String,
        /// The gateway's number for this harness under the lease.
        channel: u32,
        /// Binding-owned variables.
        environment: BTreeMap<String, String>,
    },
    /// Bytes for the harness's standard input.
    Input {
        /// The lease's id.
        lease: String,
        /// The harness.
        channel: u32,
        /// The bytes.
        data: Data,
    },
    /// The harness's standard input ends.
    InputClosed {
        /// The lease's id.
        lease: String,
        /// The harness.
        channel: u32,
    },
    /// Stop the harness: `grace_ms` for it to leave by itself, then its
    /// process tree is stopped, at most `kill_ms` for each step.
    Stop {
        /// The lease's id.
        lease: String,
        /// The harness.
        channel: u32,
        /// How long it may take to leave by itself.
        grace_ms: u64,
        /// How long each forced step may take.
        kill_ms: u64,
    },
    /// End the lease: stop everything still running under it and answer what
    /// that took.
    End {
        /// The lease's id.
        lease: String,
    },
    /// Say what became of a lease from an earlier connection.
    Account {
        /// The lease's id.
        lease: String,
    },
}

/// What an environment sends a gateway after its [`Hello`].
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum FromEnvironment {
    /// The hello, as a frame. Always the first; [`read_hello`] reads it.
    Hello {
        /// See [`Hello::build`].
        build: String,
        /// See [`Hello::workspace`].
        workspace: String,
    },
    /// Nothing is served on this connection.
    Unavailable {
        /// Why.
        reason: Unavailability,
    },
    /// The lease is admitted.
    Granted {
        /// The lease's id.
        lease: String,
    },
    /// The lease is refused; nothing ran.
    Refused {
        /// The lease's id.
        lease: String,
        /// Why.
        reason: GrantRefusal,
    },
    /// The harness did not start; nothing runs for the channel.
    StartFailed {
        /// The lease's id.
        lease: String,
        /// The harness.
        channel: u32,
        /// Why.
        reason: StartFailure,
    },
    /// Bytes from the harness's standard output.
    Output {
        /// The lease's id.
        lease: String,
        /// The harness.
        channel: u32,
        /// The bytes.
        data: Data,
    },
    /// The harness's standard output ended.
    OutputClosed {
        /// The lease's id.
        lease: String,
        /// The harness.
        channel: u32,
    },
    /// What stopping the harness took.
    Stopped {
        /// The lease's id.
        lease: String,
        /// The harness.
        channel: u32,
        /// Its cleanup.
        cleanup: Cleanup,
    },
    /// The lease ended; everything under it was stopped.
    Ended {
        /// The lease's id.
        lease: String,
        /// What that took.
        cleanup: Cleanup,
    },
    /// What became of a lease from an earlier connection.
    Accounted {
        /// The lease's id.
        lease: String,
        /// Its cleanup, as the environment recorded it.
        cleanup: Cleanup,
    },
}

impl FromEnvironment {
    /// The lease the frame names, if it names one.
    pub fn lease(&self) -> Option<&str> {
        match self {
            Self::Hello { .. } | Self::Unavailable { .. } => None,
            Self::Granted { lease }
            | Self::Refused { lease, .. }
            | Self::StartFailed { lease, .. }
            | Self::Output { lease, .. }
            | Self::OutputClosed { lease, .. }
            | Self::Stopped { lease, .. }
            | Self::Ended { lease, .. }
            | Self::Accounted { lease, .. } => Some(lease),
        }
    }
}

impl ToEnvironment {
    /// The lease the frame names.
    pub fn lease(&self) -> &str {
        match self {
            Self::Grant { lease, .. }
            | Self::Start { lease, .. }
            | Self::Input { lease, .. }
            | Self::InputClosed { lease, .. }
            | Self::Stop { lease, .. }
            | Self::End { lease }
            | Self::Account { lease } => lease,
        }
    }
}

/// A frame body that is not one this build reads, or one too large to send.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FrameRefused(pub String);

impl std::fmt::Display for FrameRefused {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str(&self.0)
    }
}

impl std::error::Error for FrameRefused {}

/// The bytes on the wire for `frame`: its length and JSON body.
///
/// # Errors
/// A body larger than [`MAX_FRAME_BYTES`].
pub fn encode<T: Serialize>(frame: &T) -> Result<Vec<u8>, FrameRefused> {
    let body = serde_json::to_vec(frame).map_err(|error| FrameRefused(error.to_string()))?;
    crate::pairing::encode_frame(MAX_FRAME_BYTES, &body)
        .map_err(|_| FrameRefused("frame larger than the bound".into()))
}

/// The frame a body carries.
///
/// # Errors
/// A body that is not one of `T`'s frames.
pub fn decode<'a, T: Deserialize<'a>>(body: &'a [u8]) -> Result<T, FrameRefused> {
    serde_json::from_slice(body).map_err(|error| FrameRefused(error.to_string()))
}

/// The hello a first frame carries, read by its `type` and `build` alone so
/// another build's hello is recognised whatever else it holds. `None` is a
/// first frame that is not a hello at all, which is another build too.
pub fn read_hello(body: &[u8]) -> Option<Hello> {
    #[derive(Deserialize)]
    struct Loose {
        #[serde(rename = "type")]
        kind: String,
        build: String,
        #[serde(default)]
        workspace: String,
    }
    let loose: Loose = serde_json::from_slice(body).ok()?;
    (loose.kind == "hello").then_some(Hello {
        build: loose.build,
        workspace: loose.workspace,
    })
}

#[cfg(test)]
#[path = "../tests/lease/frames.rs"]
mod tests;
