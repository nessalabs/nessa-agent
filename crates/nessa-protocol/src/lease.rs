//! Lease frames: what a gateway and an environment it runs agents in say to
//! each other over one byte stream, such as `ssh <host> nessa env serve`'s
//! standard input and output (ADR 252, `docs/design/runtime-architecture.md`,
//! "Three transports, one contract").
//!
//! ```text
//! environment ── Hello{build, protocol, workspace} ──▶ gateway   (first frame, always)
//! gateway ── Grant{lease, agent} ──▶ environment ── Granted | Refused ──▶ gateway
//! gateway ── Start{lease, channel, environment} ──▶ harness process on the host
//!   gateway ── Input / InputClosed ──▶ harness stdin
//!   gateway ◀── Output / OutputClosed ── harness stdout
//!   gateway ── Stop{grace, kill} ──▶ … ── Stopped{cleanup} ──▶ gateway
//! gateway ── End{lease} ──▶ environment ── Ended{cleanup} ──▶ gateway
//! gateway ── Account{lease} ──▶ environment ── Accounted{cleanup} ──▶ gateway
//! gateway ── Keepalive ──▶ environment   (every KEEPALIVE_INTERVAL, always)
//! environment ── Published{lease, artifact, file} ──▶ gateway   (no bytes)
//!   gateway ── sftp read of the file's outbox path ──▶ environment   (the artifact channel)
//! gateway ── Collected{lease, artifact, outcome} ──▶ environment
//! ```
//!
//! Arrows are frames, in the order the two ends exchange them. Every frame
//! after the hello but the keepalive names the lease it belongs to, and
//! every frame about a harness also names its channel: the gateway's number
//! for one harness process under that lease. A frame naming no lease the
//! reader holds is dropped with evidence by the reader, never applied to
//! another lease. A harness's frames are sent in the order its binding gave
//! them: its input, then its input's end, then its stop.
//!
//! The gateway is the only one who can tell the connection is still wanted,
//! so it says so: a [`ToEnvironment::Keepalive`] at least every
//! [`KEEPALIVE_INTERVAL`]. An environment that reads nothing at all for
//! [`SILENCE_LIMIT`] takes the gateway as gone, as it does when the stream
//! ends: a network that went away silently ends no stream until TCP gives
//! up, hours later.
//!
//! On the wire each frame is [`crate::pairing::encode_frame`]'s four-byte
//! big-endian length and a JSON body of at most [`MAX_FRAME_BYTES`]. A
//! harness's bytes travel base64-encoded, at most [`MAX_DATA_BYTES`] a frame.
//!
//! A published artifact's bytes never ride these frames. [`FromEnvironment::Published`]
//! says only which file, by digest, size and the path the environment staged
//! it under; the gateway reads that path over the artifact channel beside
//! this stream (sftp on the same SSH connection) and verifies what it read
//! against the digest, then answers [`ToEnvironment::Collected`], after which
//! the environment lets the staged copy go.
//!
//! The hello is read by [`read_hello`], which looks only at its `type`,
//! `build` and `protocol`: an environment speaking another protocol is
//! recognised as such whatever else its frames carry, and is refused before
//! anything is sent to it. The protocol is a [`fingerprint`] of the sources
//! that write, read and give meaning to these frames, taken when they are
//! compiled, so it changes whenever they do, with nothing to remember to
//! bump. The package version does not: it stays the same across revisions.
use base64::{engine::general_purpose::STANDARD, Engine};
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::{collections::BTreeMap, time::Duration};

/// Most bytes one frame's body may have.
pub const MAX_FRAME_BYTES: usize = 256 * 1024;
/// Most harness bytes one [`ToEnvironment::Input`] or
/// [`FromEnvironment::Output`] carries; larger writes are split.
pub const MAX_DATA_BYTES: usize = 64 * 1024;

/// Most time a gateway lets pass without sending anything; it sends a
/// [`ToEnvironment::Keepalive`] when it has nothing else to say.
pub const KEEPALIVE_INTERVAL: Duration = Duration::from_secs(15);
/// How long an environment waits for any frame before it takes the gateway
/// as gone: three keepalives missed.
pub const SILENCE_LIMIT: Duration = Duration::from_secs(45);
/// Most an environment waits on a harness for one step of a
/// [`ToEnvironment::Stop`]: a larger grace or kill is taken as this.
pub const MAX_STOP_WAIT: Duration = Duration::from_secs(60);

/// The longest an environment's stop of one harness can take before it
/// answers, for a [`ToEnvironment::Stop`] asking `grace` and `kill`, each
/// capped at [`MAX_STOP_WAIT`] as the environment caps them: the input the
/// harness already accepted, and its end, delivered within `grace`; then the
/// grace; a signal, a forced kill and reaping the process, each within
/// `kill`; and then at most `kill` more for its output to end. Recording the
/// stop comes after these, and is the caller's margin to allow.
pub fn stop_steps(grace: Duration, kill: Duration) -> Duration {
    grace.min(MAX_STOP_WAIT) * 2 + kill.min(MAX_STOP_WAIT) * 4
}

/// Most bytes one published artifact may have: the most the gateway's
/// storage keeps of one file. The environment refuses a larger file before
/// staging any of it, and the gateway refuses a
/// [`FromEnvironment::Published`] naming more.
pub const MAX_ARTIFACT_BYTES: u64 =
    nessa_sdk::application::agent_execution::sessions::PublishedFile::MAX_BYTES;
/// Most artifacts one lease may have published and not yet collected; the
/// environment refuses another until one is answered.
pub const MAX_ARTIFACTS_IN_FLIGHT: usize = 4;
/// Most publishers one lease's publish point holds open at once, before any
/// has sent its request; another waits to be accepted until one is answered.
pub const MAX_OPEN_PUBLISHERS: usize = 16;
/// Longest artifact name, in bytes: a file name, for people only.
pub const MAX_ARTIFACT_NAME_BYTES: usize =
    nessa_sdk::application::agent_execution::sessions::ArtifactName::MAX_BYTES;
/// Longest path of a staged artifact on the environment, in bytes.
pub const MAX_ARTIFACT_PATH_BYTES: usize = 1024;

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

/// One file an environment staged for the gateway to collect: what it is,
/// and where the gateway reads it. Never its bytes.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct StagedArtifact {
    /// The file's name where it was published, for people only: no
    /// directory, at most [`MAX_ARTIFACT_NAME_BYTES`].
    pub name: String,
    /// Its declared media type, lowercase `type/subtype`.
    pub media_type: String,
    /// Its length in bytes, at most [`MAX_ARTIFACT_BYTES`].
    pub size: u64,
    /// The SHA-256 of its bytes, lowercase hex.
    pub digest: String,
    /// The absolute path the environment staged it under, which the gateway
    /// reads over the artifact channel; at most [`MAX_ARTIFACT_PATH_BYTES`].
    pub path: String,
}

/// What became of one published artifact, as the gateway answers it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase", deny_unknown_fields)]
pub enum Collection {
    /// Read, verified by its digest, held by the conversation and recorded
    /// in it with the lease as cause.
    Held,
    /// The conversation already held exactly this file; nothing was read,
    /// and the publish is recorded.
    AlreadyHeld,
    /// Not kept: nothing of it is visible in the conversation.
    Refused {
        /// Why.
        reason: CollectionRefusal,
    },
}

/// Why a published artifact was not kept.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CollectionRefusal {
    /// The lease's budget of artifact files or bytes would be passed.
    BudgetExceeded,
    /// What it says it is cannot be kept: a name, media type, size or digest
    /// this build does not accept.
    Invalid,
    /// The artifact channel could not be opened, or broke more often than
    /// the gateway retries.
    ChannelUnavailable,
    /// The bytes read were not the file described: another size or digest.
    Mismatch,
    /// The gateway could not keep the bytes, or record keeping them.
    NotKept,
    /// The lease ended before the artifact was collected.
    LeaseEnded,
}

/// The first frame an environment sends.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Hello {
    /// The environment's package version, `CARGO_PKG_VERSION`: for people
    /// and records only. Revisions that speak differently share it.
    pub build: String,
    /// The lease protocol the environment speaks, its [`fingerprint`]; a
    /// gateway speaks only to its own. Empty from an environment that sends
    /// none, which is another protocol.
    pub protocol: String,
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
    /// The gateway is still there; nothing else.
    Keepalive,
    /// What became of the lease's published `artifact`: the environment
    /// lets its staged copy go.
    Collected {
        /// The lease's id.
        lease: String,
        /// The environment's number for the artifact under the lease.
        artifact: u32,
        /// What became of it.
        outcome: Collection,
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
        /// See [`Hello::protocol`].
        protocol: String,
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
    /// The lease's harness published a file, staged for the gateway to
    /// collect over the artifact channel.
    Published {
        /// The lease's id.
        lease: String,
        /// The environment's number for the artifact under the lease.
        artifact: u32,
        /// What it is and where it is staged.
        file: StagedArtifact,
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
            | Self::Accounted { lease, .. }
            | Self::Published { lease, .. } => Some(lease),
        }
    }
}

impl ToEnvironment {
    /// The lease the frame names, if it names one.
    pub fn lease(&self) -> Option<&str> {
        match self {
            Self::Keepalive => None,
            Self::Grant { lease, .. }
            | Self::Start { lease, .. }
            | Self::Input { lease, .. }
            | Self::InputClosed { lease, .. }
            | Self::Stop { lease, .. }
            | Self::End { lease }
            | Self::Account { lease }
            | Self::Collected { lease, .. } => Some(lease),
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

/// The hello a first frame carries, read by its `type`, `build` and
/// `protocol` alone so another protocol's hello is recognised whatever else
/// it holds. `None` is a first frame that is not a hello at all, which is
/// another protocol too.
pub fn read_hello(body: &[u8]) -> Option<Hello> {
    #[derive(Deserialize)]
    struct Loose {
        #[serde(rename = "type")]
        kind: String,
        build: String,
        #[serde(default)]
        protocol: String,
        #[serde(default)]
        workspace: String,
    }
    let loose: Loose = serde_json::from_slice(body).ok()?;
    (loose.kind == "hello").then_some(Hello {
        build: loose.build,
        protocol: loose.protocol,
        workspace: loose.workspace,
    })
}

/// The lease protocol named by `sources`, the files that write, read and
/// give meaning to its frames: 16 hex digits of their FNV-1a hash, in order,
/// each file's length first so moving bytes between files changes it too.
/// Carriage returns are skipped, so a checkout with Windows line endings
/// names the same protocol. A `const fn`, to be taken over `include_bytes!`
/// when the sources are compiled.
pub const fn fingerprint(sources: &[&[u8]]) -> [u8; 16] {
    const OFFSET: u64 = 0xcbf2_9ce4_8422_2325;
    const PRIME: u64 = 0x0100_0000_01b3;
    let mut hash = OFFSET;
    let mut file = 0;
    while file < sources.len() {
        let source = sources[file];
        let mut length = 0u64;
        let mut at = 0;
        while at < source.len() {
            if source[at] != b'\r' {
                length += 1;
            }
            at += 1;
        }
        let mut shift = 0;
        while shift < 64 {
            hash = (hash ^ ((length >> shift) & 0xff)).wrapping_mul(PRIME);
            shift += 8;
        }
        at = 0;
        while at < source.len() {
            if source[at] != b'\r' {
                hash = (hash ^ source[at] as u64).wrapping_mul(PRIME);
            }
            at += 1;
        }
        file += 1;
    }
    let digits = b"0123456789abcdef";
    let mut text = [0u8; 16];
    let mut place = 0;
    while place < 16 {
        text[place] = digits[((hash >> (60 - 4 * place)) & 0xf) as usize];
        place += 1;
    }
    text
}

#[cfg(test)]
#[path = "../tests/lease/frames.rs"]
mod tests;
