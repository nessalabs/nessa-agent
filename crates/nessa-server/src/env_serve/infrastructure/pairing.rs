//! This machine's pairing as an environment, on disk and on the wire: the
//! key, the gateway's pin and, once approved, the credential, kept by a
//! [`FilePairingState`] in `<data>/environment/pairing`, enrolled and asked
//! about through a [`NativeEnrollmentClient`] of the `PeerEnvironment` class.
//!
//! ```text
//! FilePairingEnvironment ──▶ FilePairingState (environment/pairing, locked)
//!                       ──▶ NativeEnrollmentClient ──▶ TCP to the gateway
//! ```
//!
//! Arrows are what it holds and reaches. It speaks the application's
//! [`EnvironmentPairing`] port: the pairing client's errors are sorted
//! into answers here ([`answer_to_failure`]), and what to do with each
//! answer is decided in `application/approval.rs`.
use crate::env_serve::application::{Answer, EnvironmentPairing, Saved};
use nessa_auth::{
    adapters::pairing::{FilePairingState, ManualCode, NativeIdentity, OsEntropy},
    application::pairing::{ClientPendingStore, PrivateStateError},
    domain::pairing::ConsentClass,
};
use nessa_client_core::pairing::{NativeClientError, NativeEnrollmentClient};
use nessa_protocol::{clock::Clock, pairing::wire::NativePairingStatus};
use std::{
    io,
    net::TcpStream,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

/// The pairing's directory, beneath `<data>/environment`.
const PAIRING: &str = "pairing";
/// How long a TCP connect to the gateway may take when pairing.
const CONNECT: Duration = Duration::from_secs(5);

/// What dialing needs once the owner approved this machine: its key and the
/// gateway's pin.
pub(crate) type Approved = (Arc<NativeIdentity>, [u8; 44]);

/// This machine's pairing with the gateway at one address.
pub(crate) struct FilePairingEnvironment {
    client: NativeEnrollmentClient,
    store: Arc<FilePairingState>,
    directory: PathBuf,
    address: String,
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
            address: address.to_owned(),
        })
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

    /// Pair with the gateway using the owner's `code`.
    ///
    /// # Errors
    /// The gateway could not be reached or did not pair this machine; the
    /// message says what to do.
    pub(crate) async fn enroll(&self, code: ManualCode) -> Result<NativePairingStatus, String> {
        let stream = connect(&self.address)
            .await
            .map_err(|error| format!("{}: {error}", self.address))?;
        self.client
            .enroll(stream, code, OsEntropy)
            .await
            .map_err(|error| self.enroll_failure(error))
    }

    /// Let go of the pairing client's worker.
    pub(crate) async fn shutdown(&self) {
        self.client.shutdown().await;
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
        let stream = match connect(&self.address).await {
            Ok(stream) => stream,
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

/// A blocking stream to the gateway for the pairing client, connected
/// without blocking the runtime; each address the name resolves to is tried.
async fn connect(address: &str) -> io::Result<TcpStream> {
    let stream = tokio::time::timeout(CONNECT, tokio::net::TcpStream::connect(address))
        .await
        .map_err(|_| io::Error::from(io::ErrorKind::TimedOut))??
        .into_std()?;
    stream.set_nonblocking(false)?;
    Ok(stream)
}

#[cfg(test)]
#[path = "../../../tests/env_serve/pairing.rs"]
mod tests;
