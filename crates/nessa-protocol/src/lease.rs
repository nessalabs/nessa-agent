//! Lease frames: what a gateway and an environment it runs agents in say to
//! each other over one byte stream, such as `ssh <host> nessa env serve`'s
//! standard input and output (ADR 252, `docs/design/runtime-architecture.md`,
//! "Three transports, one contract").
//!
//! ```text
//! environment ── Hello{build, protocol, workspace} ──▶ gateway   (first frame, always)
//! gateway ── Grant{lease, agent, sandbox} ──▶ environment ── Granted{terms} | Refused ──▶ gateway
//! gateway ── Start{lease, channel, environment} ──▶ harness process on the host
//!   gateway ── Input / InputClosed ──▶ harness stdin
//!   gateway ◀── Output / OutputClosed ── harness stdout
//!   gateway ── Stop{grace, kill} ──▶ … ── Stopped{cleanup} ──▶ gateway
//! gateway ── End{lease} ──▶ environment ── Ended{cleanup} ──▶ gateway
//! environment ── Ending{lease, expired} ──▶ gateway   (past Granted's leaseMs: it stops
//!   everything) ── gateway ── End{lease} ──▶ environment ── Ended{cleanup} ──▶ gateway
//! environment ── Draining ──▶ gateway   (the host is stopping: said once, before anything
//!   else of the drain, so the gateway asks it for no new lease)
//! environment ── Ending{lease, drained} ──▶ gateway   (the host is stopping: it ends every
//!   lease it holds and admits none) ── gateway ── End{lease} ──▶ environment
//!   ── Ended{cleanup} ──▶ gateway; once every lease is answered the environment
//!   closes the connection
//! gateway ── Revoked ──▶ environment   (the gateway no longer lets it serve: it ends every
//!   lease it holds as revoked and admits none) ── gateway ── End{lease} ──▶ environment
//!   ── Ended{cleanup} ──▶ gateway, which then closes the connection
//! gateway ── Account{lease} ──▶ environment ── Accounted{cleanup} ──▶ gateway
//! gateway ── Keepalive ──▶ environment   (every KEEPALIVE_INTERVAL, always)
//! environment ── Keepalive ──▶ gateway   (when idle KEEPALIVE_INTERVAL)
//! environment ── Machine{report} ──▶ gateway   (after Hello, then every MACHINE_REPORT_INTERVAL)
//! environment ── Published{lease, artifact, request: None, file} ──▶ gateway   (no bytes)
//!   gateway ── sftp read of the file's outbox path ──▶ environment   (the artifact channel)
//!   or, on a connection with nothing beside it (a paired one):
//!   gateway ── ReadArtifact{lease, artifact, offset, length} ──▶ environment
//!     ── ArtifactRead{lease, artifact, offset, data} | ArtifactNotRead{.., reason} ──▶ gateway
//! gateway ── Collected{lease, artifact, outcome} ──▶ environment
//! gateway ── Publish{lease, request, path, mediaType} ──▶ environment
//!   staged as the lease's publish point stages ──▶ Published{lease, artifact, request, file}
//!   ──▶ (read, kept, as above) ──▶ Collected{lease, artifact, outcome}
//!   not staged ──▶ NotPublished{lease, request, reason}
//! gateway ── Fetch{lease, request, destination, digest, mediaType, size} ──▶ environment
//!   already exactly there ──▶ Fetched{lease, request, outcome: alreadyPlaced}
//!   not taken ──▶ FetchRefused{lease, request, reason}
//!   taken ──▶ FetchAccepted{lease, request, path} (an empty part in its inbox)
//!     gateway ── sftp write of the part, resumed by its length ──▶ environment
//!     gateway ── Delivered{lease, request} ──▶ verified, linked in place
//!       ──▶ Fetched{lease, request, outcome: placed | alreadyPlaced | refused}
//!     gateway gives up ── Undelivered{lease, request} ──▶ the part let go, unanswered
//!
//! a command lease (one bounded command, no harness):
//! gateway ── GrantCommand{lease, argv, cwd, timeoutMs} ──▶ environment ── Granted{terms} | Refused ──▶ gateway
//! gateway ── Run{lease} ──▶ the command ── Ran{end, stdout, stderr, cleanup} ──▶ gateway
//!   gateway ── End{lease} ──▶ (stops it) ── Ran{stopped} ── Ended{cleanup} ──▶ gateway
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
//! up, hours later. The environment says the same when it has nothing else
//! to say ([`FromEnvironment::Keepalive`]), so a gateway that holds no
//! process of its own on the connection, as with an environment that dialed
//! it, can tell the same of the environment.
//!
//! An environment admits a lease under its own policy, and may narrow it:
//! its `Granted` says the terms it granted ([`GrantedTerms`]), which are what
//! the gateway records, never what it asked. The sandbox is granted exactly
//! as asked or the lease is refused; a command's timeout may come back
//! shorter, never longer. A harness's lease may come back with a
//! `leaseMs` it was not asked for: a limit is narrower than none. The
//! environment ends such a lease itself once it has lasted that long, and
//! says so with [`FromEnvironment::Ending`]; the gateway keeps no timer of
//! its own for it.
//!
//! A command lease runs one command, once, and its `Ran` ends it: the
//! environment answers it with the command's captured output, at most
//! [`MAX_DATA_BYTES`] of both streams together, so one frame carries it.
//!
//! On the wire each frame is [`crate::pairing::encode_frame`]'s four-byte
//! big-endian length and a JSON body of at most [`MAX_FRAME_BYTES`]. A
//! harness's bytes travel base64-encoded, at most [`MAX_DATA_BYTES`] a frame.
//!
//! [`FromEnvironment::Published`] says only which file, by digest, size and
//! the path the environment staged it under, never its bytes. Over SSH the
//! gateway reads that path over the artifact channel beside this stream
//! (sftp on the same SSH connection). A paired connection has nothing beside
//! it, so there the gateway reads the file over this stream itself, a range
//! at a time ([`ToEnvironment::ReadArtifact`]), each at most
//! [`MAX_DATA_BYTES`]; the environment holds at most
//! [`MAX_ARTIFACT_READS_IN_FLIGHT`] reads of one artifact whose answers
//! are not yet handed to its writer, refusing more, and none answers after
//! the lease's `Ended`; the environment answers only
//! for a staged copy still waiting for its `Collected`, never a path the
//! gateway names. Either way the gateway verifies what it read against the
//! digest, then answers [`ToEnvironment::Collected`], after which the
//! environment lets the staged copy go.
//!
//! A file is published two ways, which meet at [`FromEnvironment::Published`]:
//! a harness asks the lease's publish point on its host, and that frame names
//! no request; or the gateway asks with [`ToEnvironment::Publish`], for the
//! agent's own tool call, and the frame names that call's `request`, so the
//! gateway knows whose call the file answers. A gateway's ask that is not
//! staged is answered [`FromEnvironment::NotPublished`] instead.
//!
//! A file goes the other way, into the workspace, only when the gateway
//! asks with [`ToEnvironment::Fetch`], for the agent's own tool call. The
//! environment answers where in its inbox the bytes are to be written
//! ([`FromEnvironment::FetchAccepted`]); the gateway writes them there over
//! the same artifact channel, says [`ToEnvironment::Delivered`], and the
//! environment checks the part as it is on disk against the size and digest
//! asked before it links it at the destination, never over anything there.
//! These frames carry no bytes of it either.
//!
//! The hello is read by [`read_hello`], which looks only at its `type`,
//! `build` and `protocol`: an environment speaking another protocol is
//! recognised as such whatever else its frames carry, and is refused before
//! anything is sent to it. The protocol is a [`fingerprint`] of the sources
//! that write, read and give meaning to these frames, taken when they are
//! compiled, so it changes whenever they do, with nothing to remember to
//! bump. The package version does not: it stays the same across revisions.
use base64::{engine::general_purpose::STANDARD, Engine};
use nessa_sdk::domain::agent_execution::leases::SandboxProfile;
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
/// Most an environment takes, once a command lease's command is stopped or
/// past its timeout, to answer its [`FromEnvironment::Ran`]: the command's
/// grace, its forced kill, and the last of its output.
pub const COMMAND_STOP_WAIT: Duration = Duration::from_secs(10);

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
/// Longest path of a staged artifact or a fetch's destination on the
/// environment, in bytes: the longest a conversation records.
pub const MAX_ARTIFACT_PATH_BYTES: usize =
    nessa_sdk::application::agent_execution::sessions::EnvironmentPath::MAX_BYTES;
/// Most fetches one lease may have taken and not yet answered on an
/// environment; the environment refuses another `busy` until one is. The
/// most a conversation records under way under one lease.
pub const MAX_FETCHES_IN_FLIGHT: usize =
    nessa_sdk::application::agent_execution::sessions::CurrentLease::MAX_ASKED_FETCHES;
/// Most [`ToEnvironment::ReadArtifact`]s of one artifact an environment
/// holds at once, each from its arrival until its answer is handed to the
/// environment's writer; one more is refused `busy`. So a gateway that
/// stops reading holds at most this many answers of one artifact there. A
/// gateway keeps within it by leaving no more than this unanswered.
pub const MAX_ARTIFACT_READS_IN_FLIGHT: usize = 4;

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
    /// The host is not configured to run commands.
    CommandsUnavailable,
    /// The command is not one a command lease can hold: no program, past its
    /// bounds, or a working directory outside the workspace's.
    InvalidCommand,
    /// The environment's owner has not granted this gateway the agent, or
    /// commands: its own per-gateway policy, or that policy could not be
    /// read.
    NotGranted,
    /// The environment cannot enforce the sandbox asked for, and never
    /// substitutes another.
    SandboxUnavailable,
    /// The gateway already said `Revoked` on this connection: nothing more
    /// is admitted on it. Not the owner's grant: the gateway's own
    /// revocation, which the ask crossed.
    Revoked,
    /// The environment is draining: it is ending every lease it holds before
    /// it stops, and admits no more. Not the owner's grant: the host going
    /// away, which the ask crossed.
    Draining,
}

/// A sandbox profile on the wire: what encloses a lease's work on the
/// environment.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Sandbox {
    /// See [`SandboxProfile::HarnessDefault`].
    HarnessDefault,
    /// See [`SandboxProfile::None`].
    None,
}

impl From<SandboxProfile> for Sandbox {
    fn from(profile: SandboxProfile) -> Self {
        match profile {
            SandboxProfile::HarnessDefault => Self::HarnessDefault,
            SandboxProfile::None => Self::None,
        }
    }
}

impl From<Sandbox> for SandboxProfile {
    fn from(sandbox: Sandbox) -> Self {
        match sandbox {
            Sandbox::HarnessDefault => Self::HarnessDefault,
            Sandbox::None => Self::None,
        }
    }
}

/// What an environment granted a lease: the terms the gateway records.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct GrantedTerms {
    /// The sandbox enclosing its work: the one asked for a harness's lease,
    /// [`Sandbox::None`] for a command's, which nothing encloses.
    pub sandbox: Sandbox,
    /// How long its command may run, at most the timeout asked; only for a
    /// command lease.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub timeout_ms: Option<u64>,
    /// How long a harness's lease may last from its grant before the
    /// environment ends it ([`FromEnvironment::Ending`]); only for a
    /// harness's lease, and `None` when it may last until ended.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub lease_ms: Option<u64>,
}

/// Why an environment is ending a lease the gateway did not ask it to end.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum EndingCause {
    /// The lease lasted as long as it was granted ([`GrantedTerms::lease_ms`]).
    Expired,
    /// The environment is draining before it stops: it ends every lease it
    /// holds, with what that took, rather than leaving them to be lost.
    Drained,
}

/// How a command lease's command ended.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(
    tag = "outcome",
    rename_all = "camelCase",
    rename_all_fields = "camelCase",
    deny_unknown_fields
)]
pub enum CommandEnd {
    /// It exited by itself with `code`.
    Exited {
        /// Its exit status.
        code: i32,
    },
    /// A signal the environment did not send ended it.
    Signalled {
        /// The signal's number.
        signal: i32,
    },
    /// Its timeout passed and the environment stopped it.
    TimedOut,
    /// The gateway's `End`, or its connection's end, stopped it.
    Stopped,
    /// Its host is draining before it stops, and stopped it: a system
    /// stop, not a loss.
    Drained,
    /// Its program could not be started; nothing ran.
    NotStarted,
    /// The environment could not observe how it ended.
    Unknown,
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

/// Why an environment did not publish a file at all: nothing of it reached
/// the gateway. A harness is told it by its publish point; the gateway, for
/// a publish it asked, by [`FromEnvironment::NotPublished`].
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PublishRefusal {
    /// The path is not a regular file inside the workspace, after every
    /// link in it is followed.
    OutsideWorkspace,
    /// No such file, or not one that can be read.
    Unreadable,
    /// Empty, or larger than [`MAX_ARTIFACT_BYTES`].
    Size,
    /// A media type that is not lowercase `type/subtype`, or a file name
    /// that cannot be shown.
    Invalid,
    /// This lease already has as many artifacts in flight as it may
    /// ([`MAX_ARTIFACTS_IN_FLIGHT`]).
    Busy,
    /// The copy could not be staged.
    StagingFailed,
    /// The publish could not be recorded in the environment's audit.
    AuditUnavailable,
    /// The lease ended, or is not held there, before the file was offered.
    LeaseEnded,
}

/// Why an environment did not place a file the gateway asked it to fetch:
/// nothing is at the destination that was not there before.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FetchRefusal {
    /// This environment takes no files from the gateway: its host's
    /// `envServe.artifactsIn` is off.
    Unavailable,
    /// The destination is not a name inside the workspace once every link
    /// in its directory is followed, or its last part is a link.
    OutsideWorkspace,
    /// The destination's directory does not exist.
    NoDirectory,
    /// Something other than exactly this file is at the destination.
    Occupied,
    /// Empty, or larger than [`MAX_ARTIFACT_BYTES`].
    Size,
    /// A digest, media type or destination that cannot be read.
    Invalid,
    /// This lease already has as many fetches in flight as it may
    /// ([`MAX_FETCHES_IN_FLIGHT`]).
    Busy,
    /// The bytes delivered are not the file asked for: another size or
    /// another digest.
    Mismatch,
    /// Its part could not be made, read or linked in place.
    PlacingFailed,
    /// The fetch could not be recorded in the environment's audit.
    AuditUnavailable,
    /// The lease ended, or is not held there, before the file was placed.
    LeaseEnded,
}

/// What became of a file the gateway asked an environment to fetch.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "outcome", rename_all = "camelCase", deny_unknown_fields)]
pub enum FetchOutcome {
    /// Verified and linked at the destination, which held nothing before.
    Placed {
        /// The destination, absolute, as the environment resolved it.
        path: String,
    },
    /// The destination already was exactly this file: nothing changed.
    AlreadyPlaced {
        /// The destination, absolute, as the environment resolved it.
        path: String,
    },
    /// Not placed.
    Refused {
        /// Why.
        reason: FetchRefusal,
    },
}

/// Why an environment did not answer a [`ToEnvironment::ReadArtifact`] with
/// the file's bytes.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ArtifactReadRefusal {
    /// No such artifact waits for its `Collected` under the lease here: it
    /// was answered, never published, or the lease ended and its staged
    /// copies went with it.
    NotStaged,
    /// Its staged copy could not be read.
    Unreadable,
    /// The artifact already has [`MAX_ARTIFACT_READS_IN_FLIGHT`] reads
    /// unanswered.
    Busy,
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
    /// Admit a lease to run `agent`'s harness, enclosed by `sandbox`.
    Grant {
        /// The lease's id.
        lease: String,
        /// The agent, by its protocol name (`AgentId::name`).
        agent: String,
        /// The sandbox asked for: granted as it is, or refused.
        sandbox: Sandbox,
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
    /// Admit a command lease to run `argv` once, in `cwd` beneath the
    /// workspace (`None` for the workspace), for at most `timeout_ms`.
    GrantCommand {
        /// The command lease's id.
        lease: String,
        /// The program and its arguments, never a shell line.
        argv: Vec<String>,
        /// The directory beneath the workspace.
        cwd: Option<String>,
        /// How long it may run.
        timeout_ms: u64,
    },
    /// Run the granted command lease's command; its `Ran` answers.
    Run {
        /// The command lease's id.
        lease: String,
    },
    /// The gateway is still there; nothing else.
    Keepalive,
    /// The gateway no longer lets this environment serve it: the credential
    /// it was admitted with was revoked there. The environment ends every
    /// lease it holds on this connection, recording that as their cause,
    /// and admits no more on it; it answers each lease's `End` with what
    /// that end took, and the gateway then closes the connection. Names no
    /// lease: it is about the connection.
    Revoked,
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
    /// Publish the file at `path` under the lease, as its publish point
    /// would for a harness: staged by digest and offered with a
    /// [`FromEnvironment::Published`] naming `request`, or answered
    /// [`FromEnvironment::NotPublished`].
    Publish {
        /// The lease's id.
        lease: String,
        /// The gateway's name for this ask: the agent's tool call.
        request: String,
        /// The file: absolute, or relative to the workspace.
        path: String,
        /// Its media type, when the agent named one; else it is read from
        /// the file's extension.
        media_type: Option<String>,
    },
    /// Fetch a file the conversation holds into the workspace at
    /// `destination`, for the agent's own tool call: answered
    /// [`FromEnvironment::FetchAccepted`] with where to write it,
    /// [`FromEnvironment::FetchRefused`], or, when the destination already
    /// is exactly this file, [`FromEnvironment::Fetched`] at once.
    Fetch {
        /// The lease's id.
        lease: String,
        /// The gateway's name for this ask: the agent's tool call.
        request: String,
        /// Where it goes: absolute, or relative to the workspace.
        destination: String,
        /// Its SHA-256, lowercase hex.
        digest: String,
        /// Its media type, lowercase `type/subtype`.
        media_type: String,
        /// Its length in bytes.
        size: u64,
    },
    /// Every byte of a fetch the environment took is written to the part
    /// it named: check it and place it.
    Delivered {
        /// The lease's id.
        lease: String,
        /// The fetch's request.
        request: String,
    },
    /// The gateway gives up on a fetch the environment took: nothing more
    /// is written, and nothing is placed. Not answered.
    Undelivered {
        /// The lease's id.
        lease: String,
        /// The fetch's request.
        request: String,
    },
    /// Send at most `length` bytes, at most [`MAX_DATA_BYTES`], of the
    /// lease's published `artifact`'s staged copy from `offset`: answered
    /// [`FromEnvironment::ArtifactRead`] or
    /// [`FromEnvironment::ArtifactNotRead`]. Only on a connection with no
    /// artifact channel beside it.
    ReadArtifact {
        /// The lease's id.
        lease: String,
        /// The environment's number for the artifact under the lease.
        artifact: u32,
        /// Where in the file the range starts.
        offset: u64,
        /// How many bytes are asked; more than [`MAX_DATA_BYTES`] is read
        /// as that many.
        length: u32,
    },
}

/// How often an environment says what its machine is doing, busy or idle
/// ([`FromEnvironment::Machine`]).
pub const MACHINE_REPORT_INTERVAL: Duration = Duration::from_secs(15);

/// The most bytes of a [`MachineReport`]'s host name or operating system
/// version; a longer one is cut by its sender at a character boundary.
pub const MAX_MACHINE_TEXT_BYTES: usize = 128;

/// The largest byte figure a [`MachineReport`] may carry: the largest whole
/// number a JSON reader keeps exactly (2^53 − 1), so the figure reaches the
/// person as it was sent. The product schema's `DeviceMachine` holds the same.
pub const MAX_MACHINE_FIGURE: u64 = (1 << 53) - 1;

/// A machine as its environment describes it: what it is, and how busy it
/// is at the moment it was read. Every figure is the sender's word, shown to
/// the person and never used to decide anything.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct MachineReport {
    /// The machine's host name, at most [`MAX_MACHINE_TEXT_BYTES`].
    pub hostname: String,
    /// `macos`, `linux`, `windows` or another `std::env::consts::OS`.
    pub os: String,
    /// The operating system's version, at most [`MAX_MACHINE_TEXT_BYTES`];
    /// empty when not known.
    pub os_version: String,
    /// `std::env::consts::ARCH`: `aarch64`, `x86_64` …
    pub arch: String,
    /// Logical processors.
    pub cpus: u32,
    /// Processor use across all of them, in tenths of a percent, 0 to 1000.
    pub cpu_permille: u32,
    /// Memory installed, in bytes.
    pub memory_total_bytes: u64,
    /// Memory in use, in bytes.
    pub memory_used_bytes: u64,
    /// Size of the disk holding the environment's data, in bytes.
    pub disk_total_bytes: u64,
    /// Bytes in use on that disk.
    pub disk_used_bytes: u64,
}

impl MachineReport {
    /// `text` cut to [`MAX_MACHINE_TEXT_BYTES`] at a character boundary.
    pub fn bounded(text: &str) -> String {
        let mut end = text.len().min(MAX_MACHINE_TEXT_BYTES);
        while !text.is_char_boundary(end) {
            end -= 1;
        }
        text[..end].to_owned()
    }

    /// Whether every field is within its bound and each used figure within
    /// its total. A report that is not is dropped by its reader.
    pub fn is_bounded(&self) -> bool {
        self.hostname.len() <= MAX_MACHINE_TEXT_BYTES
            && self.os.len() <= MAX_MACHINE_TEXT_BYTES
            && self.os_version.len() <= MAX_MACHINE_TEXT_BYTES
            && self.arch.len() <= MAX_MACHINE_TEXT_BYTES
            && self.cpu_permille <= 1000
            && self.memory_total_bytes <= MAX_MACHINE_FIGURE
            && self.disk_total_bytes <= MAX_MACHINE_FIGURE
            && self.memory_used_bytes <= self.memory_total_bytes
            && self.disk_used_bytes <= self.disk_total_bytes
    }
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
    /// Nothing else to say: the environment is still there. Sent when its
    /// side was idle for [`KEEPALIVE_INTERVAL`].
    Keepalive,
    /// What the machine is and how busy it is now: sent once after the
    /// hello and again every [`MACHINE_REPORT_INTERVAL`], busy or idle, so
    /// the gateway can show its devices. It also says the environment is
    /// still there, as [`Self::Keepalive`] does.
    Machine {
        /// See [`MachineReport`].
        report: MachineReport,
    },
    /// The environment is stopping: it admits no lease from now on, and
    /// ends each it holds with [`EndingCause::Drained`]. Said once, before
    /// any of those endings, so the gateway sends new work elsewhere rather
    /// than finding each refused.
    Draining,
    /// Nothing is served on this connection.
    Unavailable {
        /// Why.
        reason: Unavailability,
    },
    /// The lease is admitted, on `terms`.
    Granted {
        /// The lease's id.
        lease: String,
        /// What was granted, which may be narrower than what was asked.
        terms: GrantedTerms,
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
    /// The environment is ending the lease for `cause`, unasked: it stops
    /// everything under it, and answers the gateway's `End` with what that
    /// took.
    Ending {
        /// The lease's id.
        lease: String,
        /// Why.
        cause: EndingCause,
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
    /// A file was published under the lease, staged for the gateway to
    /// collect over the artifact channel.
    Published {
        /// The lease's id.
        lease: String,
        /// The environment's number for the artifact under the lease.
        artifact: u32,
        /// The gateway's [`ToEnvironment::Publish`] it answers; `None` for a
        /// file a harness published through the lease's publish point.
        request: Option<String>,
        /// What it is and where it is staged.
        file: StagedArtifact,
    },
    /// The gateway's [`ToEnvironment::Publish`] was not staged: nothing of
    /// the file is offered.
    NotPublished {
        /// The lease's id.
        lease: String,
        /// The ask it answers.
        request: String,
        /// Why.
        reason: PublishRefusal,
    },
    /// A fetch is taken: its bytes are to be written to `path`, an empty
    /// part in the environment's inbox, then [`ToEnvironment::Delivered`].
    FetchAccepted {
        /// The lease's id.
        lease: String,
        /// The fetch's request.
        request: String,
        /// Where its bytes go, absolute, on the environment.
        path: String,
    },
    /// A fetch is not taken: nothing is to be written.
    FetchRefused {
        /// The lease's id.
        lease: String,
        /// The fetch's request.
        request: String,
        /// Why.
        reason: FetchRefusal,
    },
    /// What became of a fetch: the last word on it.
    Fetched {
        /// The lease's id.
        lease: String,
        /// The fetch's request.
        request: String,
        /// Placed, already placed, or why not.
        outcome: FetchOutcome,
    },
    /// A command lease's command ended, and the lease with it.
    Ran {
        /// The command lease's id.
        lease: String,
        /// How the command ended.
        end: CommandEnd,
        /// The newest of its standard output that was kept.
        stdout: Data,
        /// The newest of its standard error that was kept.
        stderr: Data,
        /// Bytes it printed that were not kept, the oldest first.
        dropped_bytes: u64,
        /// What releasing it took.
        cleanup: Cleanup,
    },
    /// The bytes of a published artifact's staged copy a
    /// [`ToEnvironment::ReadArtifact`] asked for: as many as asked, or
    /// fewer only where the file ends, none at or past its end.
    ArtifactRead {
        /// The lease's id.
        lease: String,
        /// The artifact.
        artifact: u32,
        /// The offset asked, which the bytes start at.
        offset: u64,
        /// The bytes.
        data: Data,
    },
    /// A [`ToEnvironment::ReadArtifact`] is not answered with bytes.
    ArtifactNotRead {
        /// The lease's id.
        lease: String,
        /// The artifact.
        artifact: u32,
        /// The offset asked.
        offset: u64,
        /// Why.
        reason: ArtifactReadRefusal,
    },
}

impl FromEnvironment {
    /// The lease the frame names, if it names one.
    pub fn lease(&self) -> Option<&str> {
        match self {
            Self::Hello { .. }
            | Self::Unavailable { .. }
            | Self::Keepalive
            | Self::Draining
            | Self::Machine { .. } => None,
            Self::Granted { lease, .. }
            | Self::Refused { lease, .. }
            | Self::StartFailed { lease, .. }
            | Self::Output { lease, .. }
            | Self::OutputClosed { lease, .. }
            | Self::Stopped { lease, .. }
            | Self::Ending { lease, .. }
            | Self::Ended { lease, .. }
            | Self::Accounted { lease, .. }
            | Self::Published { lease, .. }
            | Self::NotPublished { lease, .. }
            | Self::FetchAccepted { lease, .. }
            | Self::FetchRefused { lease, .. }
            | Self::Fetched { lease, .. }
            | Self::ArtifactRead { lease, .. }
            | Self::ArtifactNotRead { lease, .. }
            | Self::Ran { lease, .. } => Some(lease),
        }
    }
}

impl ToEnvironment {
    /// The lease the frame names, if it names one.
    pub fn lease(&self) -> Option<&str> {
        match self {
            Self::Keepalive | Self::Revoked => None,
            Self::Grant { lease, .. }
            | Self::Start { lease, .. }
            | Self::Input { lease, .. }
            | Self::InputClosed { lease, .. }
            | Self::Stop { lease, .. }
            | Self::End { lease }
            | Self::Account { lease }
            | Self::Collected { lease, .. }
            | Self::Publish { lease, .. }
            | Self::Fetch { lease, .. }
            | Self::Delivered { lease, .. }
            | Self::Undelivered { lease, .. }
            | Self::ReadArtifact { lease, .. }
            | Self::GrantCommand { lease, .. }
            | Self::Run { lease } => Some(lease),
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
