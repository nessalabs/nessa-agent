//! This device's enrollment: pairing with a code, and the pinned status that
//! decides whether it may read, must purge, or neither.
//!
//! ```text
//! profile --> FilePairingState --> NativeEnrollmentClient (pinned TLS)
//!         --> NativePairingStatus --> application PinnedStatus
//! ```
//! Arrows are construction and calls. The client saves an issued credential
//! when it sees Active; this module only maps what the gateway said.
use super::profile::Profile;
use crate::app::dependencies::RuntimeDependencies;
use crate::device_pairing::infrastructure::{
    wire::NativePairingStatus, NativeClientError, NativeEnrollmentClient,
};
use crate::read_only_sync::application::device::PinnedStatus;
use nessa_auth::{
    adapters::pairing::{ManualCode, OsEntropy},
    application::pairing::ClientPendingStore,
    domain::pairing::{AttemptOutcome, TerminalCause},
};
use nessa_sync::replication::domain::Id;
use serde_json::{json, Value};
use std::{
    io::Read,
    net::{SocketAddr, TcpStream},
    sync::Arc,
    time::Duration,
};

/// How long a TCP connect to the gateway may take.
const CONNECT: Duration = Duration::from_secs(5);
/// Bytes read from standard input for a code: `XXXX-XXXX`, a line ending, and
/// one more to tell an overlong line apart.
const CODE_INPUT_BYTES: usize = 12;

/// One enrollment client over this device's private state.
pub(super) struct Device {
    client: NativeEnrollmentClient,
    runtime: tokio::runtime::Runtime,
    gateway: SocketAddr,
}
impl Device {
    /// An enrollment client over `store`: the private state itself, or that
    /// state behind the cache purge an ended enrollment runs first.
    pub(super) fn new(
        profile: &Profile,
        store: Arc<dyn ClientPendingStore>,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()?;
        Ok(Self {
            client: NativeEnrollmentClient::new(store, RuntimeDependencies::default().clock),
            runtime,
            gateway: profile.gateway,
        })
    }
    /// The gateway's native address.
    pub(super) fn gateway(&self) -> SocketAddr {
        self.gateway
    }
    /// Read the pinned status; an Active one is saved by the client first.
    pub(super) fn status(&self) -> Result<NativePairingStatus, NativeClientError> {
        let stream = connect(self.gateway)?;
        self.runtime.block_on(self.client.status(stream, None))
    }
    /// Enroll with `code`; the device's key and the gateway pin are saved
    /// before the claim is confirmed.
    pub(super) fn pair(&self, code: ManualCode) -> Result<NativePairingStatus, NativeClientError> {
        let stream = connect(self.gateway)?;
        self.runtime
            .block_on(self.client.enroll(stream, code, OsEntropy))
    }
}
impl Drop for Device {
    fn drop(&mut self) {
        self.runtime.block_on(self.client.shutdown());
    }
}

fn connect(address: SocketAddr) -> Result<TcpStream, NativeClientError> {
    TcpStream::connect_timeout(&address, CONNECT)
        .map_err(|error| NativeClientError::Io(error.kind()))
}

/// Read one code line from `input`, bounded; the copy is overwritten after
/// parsing. A trailing line ending is framing, not part of the code.
pub(super) fn read_code(input: &mut dyn Read) -> Option<ManualCode> {
    let mut bytes = Vec::with_capacity(CODE_INPUT_BYTES);
    let read = input
        .take(CODE_INPUT_BYTES as u64)
        .read_to_end(&mut bytes)
        .is_ok();
    let mut line = bytes.as_slice();
    for ending in [&b"\r\n"[..], &b"\n"[..]] {
        if let Some(stripped) = line.strip_suffix(ending) {
            line = stripped;
            break;
        }
    }
    let code = read.then(|| ManualCode::parse(line).ok()).flatten();
    bytes.fill(0);
    code
}

/// What the application decides from; `None` for an Active status whose
/// receiver is not a valid sync identity.
pub(super) fn pinned(status: &NativePairingStatus) -> Option<PinnedStatus> {
    Some(match status {
        NativePairingStatus::Active {
            receiver,
            access_epoch,
            ..
        } => PinnedStatus::Active {
            receiver: Id::new(receiver.as_str()).ok()?,
            access_epoch: *access_epoch,
        },
        NativePairingStatus::Terminal { .. } => PinnedStatus::Terminal,
        NativePairingStatus::Pending(_)
        | NativePairingStatus::Unclaimed { .. }
        | NativePairingStatus::Claimed(_)
        | NativePairingStatus::Approved(_)
        | NativePairingStatus::Staging(_) => PinnedStatus::NotActive,
    })
}

/// Public status fields for stdout: phase, and what Active and Terminal add.
/// No key, pin or private selector.
pub(super) fn status_json(status: &NativePairingStatus) -> Value {
    match status {
        NativePairingStatus::Pending(_) => json!({"phase":"pending"}),
        NativePairingStatus::Unclaimed { outcome, .. } => {
            json!({"phase":"unclaimed","outcome":attempt_outcome(*outcome)})
        }
        NativePairingStatus::Claimed(_) => json!({"phase":"claimed"}),
        NativePairingStatus::Approved(_) => json!({"phase":"approved"}),
        NativePairingStatus::Staging(_) => json!({"phase":"staging"}),
        NativePairingStatus::Active {
            credential,
            receiver,
            access_epoch,
            ..
        } => json!({"phase":"active","credentialId":credential.as_str(),
            "receiver":receiver.as_str(),"accessEpoch":access_epoch.to_string()}),
        NativePairingStatus::Terminal { cause, .. } => {
            json!({"phase":"terminal","cause":terminal_cause(*cause)})
        }
    }
}

pub(super) fn terminal_cause(cause: TerminalCause) -> &'static str {
    match cause {
        TerminalCause::CredentialRevoked => "credentialRevoked",
        TerminalCause::Denied => "denied",
        TerminalCause::Cancelled => "cancelled",
        TerminalCause::Expired => "expired",
        TerminalCause::Restarted => "restarted",
    }
}

fn attempt_outcome(outcome: AttemptOutcome) -> &'static str {
    match outcome {
        AttemptOutcome::Pending => "pending",
        AttemptOutcome::Failed(_) => "failed",
        AttemptOutcome::Claimed => "claimed",
        AttemptOutcome::Superseded => "superseded",
    }
}

/// A typed client failure for stdout, without peer diagnostics.
pub(super) fn client_failure(error: NativeClientError) -> Value {
    let code = match error {
        NativeClientError::Busy => "busy",
        NativeClientError::PendingExists => "pendingExists",
        NativeClientError::Enrolled => "enrolled",
        NativeClientError::OriginalNotRetryable => "originalNotRetryable",
        NativeClientError::NoPending => "noEnrollment",
        NativeClientError::Storage(_) => "storage",
        NativeClientError::Crypto(_) => "crypto",
        NativeClientError::Wire(_) => "wire",
        NativeClientError::Phase => "phase",
        NativeClientError::Refused => "refused",
        NativeClientError::Io(_) => "io",
        NativeClientError::Entropy => "entropy",
        NativeClientError::WorkerFault(_) => "workerFault",
    };
    json!({"code":code})
}
