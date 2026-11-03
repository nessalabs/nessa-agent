//! This machine's pairing as an environment, on disk and on the wire: the
//! key, the gateway's pin and, once approved, the credential, kept by a
//! [`FilePairingState`] in `<data>/environment/pairing`, enrolled and asked
//! about through a [`NativeEnrollmentClient`] of the `PeerEnvironment` class.
//!
//! ```text
//! FilePairingEnvironment ──▶ FilePairingState (environment/pairing, locked)
//!                       ──▶ environment/paired-with (the address enrolled with)
//!                       ──▶ NativeEnrollmentClient ──▶ TCP to the gateway
//!                           (a status request, with a relay: direct, then
//!                           through the relay for the pending pin)
//! ```
//!
//! Arrows are what it holds and reaches. It speaks the application's
//! [`EnvironmentPairing`] port: the pairing client's errors are sorted
//! into answers here ([`answer_to_failure`]), and what to do with each
//! answer is decided in `application/approval.rs`.
use crate::env_serve::application::{
    Answer, EnrollingPairing, EnrollmentCode, EnvironmentPairing, Saved,
};
use nessa_auth::{
    adapters::pairing::{FilePairingState, ManualCode, NativeIdentity, OsEntropy},
    application::pairing::{ClientPendingStore, PrivateStateError},
    domain::pairing::ConsentClass,
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_client_core::retained::dial_gateway;
use nessa_protocol::relay::RelayAddress;
use nessa_protocol::{clock::Clock, pairing::wire::NativePairingStatus};
use std::{
    io::{self, Read, Write},
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// The pairing's directory, beneath `<data>/environment`.
const PAIRING: &str = "pairing";
/// Beside the pairing, beneath `<data>/environment`: the address of the
/// gateway this machine enrolled with. Read only while a pairing exists, so
/// a code file is never taken as one for a pairing it was not.
const PAIRED_WITH: &str = "paired-with";
/// The most a code file or `paired-with` is read of.
const MAX_READ: u64 = 4096;
/// How long a TCP connect to the gateway may take when pairing.
const CONNECT: Duration = Duration::from_secs(5);

/// What dialing needs once the owner approved this machine: its key and the
/// gateway's pin.
pub(crate) type Approved = (Arc<NativeIdentity>, [u8; 44]);

/// This machine's pairing with the gateway at one address.
pub(crate) struct FilePairingEnvironment {
    client: NativeEnrollmentClient,
    store: Arc<FilePairingState>,
    /// `<data>/environment`, canonical.
    root: PathBuf,
    directory: PathBuf,
    address: String,
    /// Where the gateway is asked for when a status request's direct dial
    /// does not connect; `None` for direct only.
    relay: Option<RelayAddress>,
}

impl FilePairingEnvironment {
    /// The pairing kept beneath `environment` (created empty when absent),
    /// with the gateway at `address`.
    ///
    /// # Errors
    /// The directory cannot be made or read, or another command holds it;
    /// the message says which.
    pub(crate) fn open(
        environment: &Path,
        address: &str,
        clock: Arc<dyn Clock>,
    ) -> Result<Self, String> {
        let root = environment
            .canonicalize()
            .map_err(|error| error.to_string())?;
        nessa_local_storage::create_directory_beneath(&root, Path::new(PAIRING))
            .map_err(|error| error.to_string())?;
        let store =
            Arc::new(
                FilePairingState::open(&root, Path::new(PAIRING)).map_err(|error| match error {
                    PrivateStateError::Locked => {
                        "another nessa env enroll or nessa env dial is using this machine's pairing"
                            .to_owned()
                    }
                    other => format!("{other:?}"),
                })?,
            );
        let client =
            NativeEnrollmentClient::enrolling(ConsentClass::PeerEnvironment, store.clone(), clock);
        Ok(Self {
            client,
            store,
            directory: root.join(PAIRING),
            root,
            address: address.to_owned(),
            relay: None,
        })
    }

    /// Ask the pairing's status through `relay` too, when the direct dial
    /// does not connect. Enrolling stays direct: the gateway's key, which
    /// the relay is asked for, is learned by enrolling.
    pub(crate) fn through(mut self, relay: Option<RelayAddress>) -> Self {
        self.relay = relay;
        self
    }

    /// The pin of the gateway this machine is paired with, once its owner
    /// approved it; `None` when nothing was ever paired here. Nothing is
    /// asked of the gateway, and nothing is made when there is no pairing.
    ///
    /// # Errors
    /// A pairing still waiting for approval, or one that cannot be read; the
    /// message says which.
    pub(crate) async fn approved_gateway(
        environment: &Path,
        clock: Arc<dyn Clock>,
    ) -> Result<Option<[u8; 44]>, String> {
        if !environment.join(PAIRING).exists() {
            return Ok(None);
        }
        // No address: only what is saved here is read.
        let pairing = Self::open(environment, "", clock)
            .map_err(|error| format!("{error}: name the peer with --peer"))?;
        let saved = pairing.saved().await;
        pairing.shutdown().await;
        match saved? {
            Saved::Approved((_, pin)) => Ok(Some(pin)),
            Saved::Nothing => Ok(None),
            Saved::Pending => Err(
                "this machine's pairing is waiting for its owner's approval: name the peer \
                 with --peer"
                    .into(),
            ),
        }
    }

    /// The pairing's own directory, `<data>/environment/pairing`: what to
    /// remove to pair this machine again.
    pub(crate) fn directory(&self) -> &Path {
        &self.directory
    }

    /// Let go of the pairing client's worker.
    pub(crate) async fn shutdown(&self) {
        self.client.shutdown().await;
    }

    /// A blocking stream to the gateway for a status request: direct, then,
    /// with a relay, through it for the key this pairing pinned. That key is
    /// the pending record's while approval is open, and the saved
    /// credential's once that record has replaced it. With neither, there is
    /// no key to ask for: direct only.
    async fn connect_or_relay(&self) -> io::Result<TcpStream> {
        let (store, address, relay) =
            (self.store.clone(), self.address.clone(), self.relay.clone());
        let dialing = tokio::task::spawn_blocking(move || {
            let pin = relay.as_ref().and_then(|_| gateway_pin(&store));
            let route = relay.as_ref().zip(pin.as_ref());
            dial_gateway(&address, route, None, CONNECT)
        });
        bounded(dialing).await
    }

    fn enroll_failure(&self, error: NativeClientError) -> String {
        match error {
            NativeClientError::Enrolled => format!(
                "this machine is already paired: run nessa env dial, or remove {} to pair \
                 again",
                self.directory.display()
            ),
            NativeClientError::PendingExists => format!(
                "this machine has a pairing waiting for approval: run nessa env dial, or \
                 remove {} to start over",
                self.directory.display()
            ),
            NativeClientError::OtherEnrollee => {
                "that code does not pair an environment: ask for an environment invitation".into()
            }
            NativeClientError::Refused => {
                "the gateway refused: the code may be wrong, used or expired".into()
            }
            NativeClientError::Crypto(_) => "the code is not the one the gateway gave".into(),
            other => format!("pairing failed: {other:?}"),
        }
    }
}

/// The gateway key a relay is asked for: the pending pairing's while it is
/// still open, otherwise the credential that replaced it.
fn gateway_pin(store: &FilePairingState) -> Option<[u8; 44]> {
    store
        .load_pending()
        .ok()
        .flatten()
        .map(|pending| *pending.gateway_pin())
        .or_else(|| {
            store
                .load_credential()
                .ok()
                .flatten()
                .map(|credential| *credential.gateway_pin())
        })
}

impl EnvironmentPairing for FilePairingEnvironment {
    type Approval = Approved;

    async fn saved(&self) -> Result<Saved<Approved>, String> {
        let store = self.store.clone();
        // Pending first: a pending record only ever becomes a credential,
        // so a pairing that exists never reads as neither.
        let (pending, credential) = tokio::task::spawn_blocking(move || {
            Ok::<_, PrivateStateError>((store.load_pending()?, store.load_credential()?))
        })
        .await
        .map_err(|error| error.to_string())?
        .map_err(|error| format!("the pairing was not read: {error:?}"))?;
        match (credential, pending) {
            (Some(credential), _) => {
                let (key, pin, _) = credential.into_enrollment().into_parts();
                let identity = NativeIdentity::restore(key)
                    .map_err(|error| format!("the pairing's key cannot be used: {error:?}"))?;
                Ok(Saved::Approved((Arc::new(identity), pin)))
            }
            (None, Some(_)) => Ok(Saved::Pending),
            (None, None) => Ok(Saved::Nothing),
        }
    }

    async fn ask(&self) -> Answer {
        let connected = self.connect_or_relay().await;
        let stream = match connected {
            Ok(stream) => stream,
            // A saved pin that is not a key is not changed by asking again.
            Err(error) if error.kind() == io::ErrorKind::InvalidInput => {
                return Answer::Unusable(format!(
                    "the pairing waiting for approval cannot be used: {error}"
                ))
            }
            Err(error) => {
                return Answer::Unanswered(format!(
                    "{} could not be reached: {error}",
                    self.address
                ))
            }
        };
        match self.client.status(stream, None).await {
            // Reading either ends the enrollment here, so a new code
            // enrolls again at once.
            Ok(NativePairingStatus::Unclaimed { .. } | NativePairingStatus::Terminal { .. }) => {
                Answer::Ended
            }
            // Read only once the credential is saved.
            Ok(NativePairingStatus::Active { .. }) => Answer::Approved,
            Ok(_) => Answer::Waiting,
            Err(error) => answer_to_failure(error, &self.directory),
        }
    }
}

impl EnrollingPairing for FilePairingEnvironment {
    type Code = ManualCode;

    fn address(&self) -> &str {
        &self.address
    }

    async fn recorded_address(&self) -> Result<Option<String>, String> {
        let unreadable = |error: io::Error| format!("{PAIRED_WITH} cannot be read: {error}");
        let file = match nessa_local_storage::open(
            &self.root.join(PAIRED_WITH),
            nessa_local_storage::OpenMode::Read,
        ) {
            Ok(file) => file,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(unreadable(error)),
        };
        let mut address = String::new();
        file.take(MAX_READ)
            .read_to_string(&mut address)
            .map_err(unreadable)?;
        Ok(Some(address))
    }

    async fn record_address(&self) -> Result<(), String> {
        let written = (|| {
            let mut file = nessa_local_storage::PrivateTempFile::new_in(&self.root)?;
            file.as_file_mut().write_all(self.address.as_bytes())?;
            file.as_file().sync_all()?;
            file.persist(&self.root.join(PAIRED_WITH))?;
            nessa_local_storage::sync_directory(&self.root)
        })();
        written.map_err(|error| format!("{PAIRED_WITH} could not be written: {error}"))
    }

    async fn send_code(&self, code: ManualCode) -> Result<(), String> {
        // Direct only: the relay is asked for a key this learns by enrolling.
        let address = self.address.clone();
        let stream = bounded(tokio::task::spawn_blocking(move || {
            dial_gateway(&address, None, None, CONNECT)
        }))
        .await
        .map_err(|error| format!("{}: {error}", self.address))?;
        self.client
            .enroll(stream, code, OsEntropy)
            .await
            .map(|_| ())
            .map_err(|error| self.enroll_failure(error))
    }
}

/// The owner's code, kept in a file such as a mounted secret.
pub(crate) struct FileEnrollmentCode(pub(crate) PathBuf);

impl EnrollmentCode<ManualCode> for FileEnrollmentCode {
    /// One code, with whitespace around it, as a secret mounted as a file
    /// usually has a line end.
    fn read(&self) -> Result<ManualCode, String> {
        let path = &self.0;
        let unreadable = |error: io::Error| format!("{}: {error}", path.display());
        let file = std::fs::File::open(path).map_err(unreadable)?;
        let mut bytes = Vec::with_capacity(MAX_READ as usize);
        let read = file.take(MAX_READ).read_to_end(&mut bytes);
        let code = read.map_err(unreadable).and_then(|_| {
            ManualCode::parse(bytes.trim_ascii())
                .map_err(|_| format!("{} holds no pairing code", path.display()))
        });
        // The code's own copy erases itself; this one is erased here.
        bytes.fill(0);
        code
    }
}

/// What a failed status request answers. Only a pairing record this
/// machine cannot use, or a gateway answering out of turn, is
/// [`Answer::Unusable`]; anything else may clear, including a credential
/// save that failed, which keeps the pending record for the next ask.
pub(crate) fn answer_to_failure(error: NativeClientError, directory: &Path) -> Answer {
    match error {
        NativeClientError::NoPending => Answer::NotPaired,
        NativeClientError::Storage(PrivateStateError::Corrupt | PrivateStateError::Conflict)
        | NativeClientError::Crypto(_)
        | NativeClientError::Wire(_)
        | NativeClientError::Phase
        | NativeClientError::OtherEnrollee => Answer::Unusable(format!(
            "the pairing's status cannot be read: {error:?}; remove {} and enroll again",
            directory.display()
        )),
        other => Answer::Unanswered(format!("{other:?}")),
    }
}

/// A connect on a blocking worker, waited for at most [`CONNECT`]: the
/// name lookup inside it waits as long as the system resolver does, and a
/// lookup that hangs is left to end on its own.
async fn bounded(dialing: tokio::task::JoinHandle<io::Result<TcpStream>>) -> io::Result<TcpStream> {
    tokio::time::timeout(CONNECT, dialing)
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))?
        .map_err(|_| io::Error::other("the dialing worker failed"))?
}

#[cfg(test)]
#[path = "../../../tests/env_serve/pairing.rs"]
mod tests;
