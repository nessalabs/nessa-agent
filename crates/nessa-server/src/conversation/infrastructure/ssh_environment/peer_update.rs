//! A paired environment whose build speaks another lease protocol is told
//! this gateway's, and, when it asks, sent this build for its platform
//! (`nessa_protocol::build_update`), so it can run it and be served without
//! anyone going to that machine.
//!
//! ```text
//! HostLink::over: first frame is a Hello naming another protocol
//!   ──▶ VersionRefused{protocol} sent ──▶ the connection ends
//! HostLink::over: first frame is UpdateWanted{os, arch, libc}
//!   ──▶ HostInstaller::opened_for(that platform)
//!         none ──▶ UpdateRefused recorded ──▶ UpdateRefused{reason} sent
//!   ──▶ UpdateStarted recorded (or nothing is sent)
//!   ──▶ Update{digest, size, protocol} ──▶ UpdatePart … ──▶ UpdateEnd
//!   ──▶ UpdateSent recorded ──▶ the connection ends
//!         cut short, or past the deadline ──▶ UpdateRefused{failed} recorded
//! ```
//!
//! Arrows are steps, in order. The build is the one an SSH host of that
//! platform is installed with (`install.rs`): this build where it runs,
//! else the release build it pins, opened only when its bytes are the ones
//! expected. Whether the environment takes it is its owner's, decided on
//! that machine, which checks every byte and the protocol the copy states
//! before it runs it. A connection is admitted (its key serves this
//! gateway) before its first frame is read, so only a paired environment
//! is ever sent a build.
use super::audit::{EnvironmentAudit, EnvironmentEvent, InstallRefusal};
use super::install::{Build, HostInstaller, NotOpened, OpenedCopy};
use crate::env_serve::install::Platform;
use nessa_protocol::build_update::{
    encode, part, FromGateway, UPDATE_PART_BYTES, UPDATE_SEND_DEADLINE,
};
use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex, PoisonError},
    time::{Duration, Instant},
};
use tokio::io::{AsyncWrite, AsyncWriteExt};
use tokio::sync::{OwnedSemaphorePermit, Semaphore};

/// Most builds sent at once, to every host together.
const MOST_SENDS: usize = 4;

/// How long after a host began to be sent the build it is sent none again
/// (a host that keeps asking is told `busy`): as long as a send may last,
/// so one host never holds two of the [`MOST_SENDS`] places.
const HOST_ASKS_EVERY: Duration = UPDATE_SEND_DEADLINE;

/// Sends this build to paired environments that ask for it.
pub(crate) struct PeerUpdates {
    installer: Arc<HostInstaller>,
    audit: Arc<dyn EnvironmentAudit>,
    deadline: Duration,
    sends: Arc<Semaphore>,
    /// When each host last began to be sent the build, for
    /// [`HOST_ASKS_EVERY`]; hosts past it are let go on the next ask.
    asked: Mutex<HashMap<String, Instant>>,
}

impl PeerUpdates {
    pub(crate) fn new(installer: Arc<HostInstaller>, audit: Arc<dyn EnvironmentAudit>) -> Self {
        Self {
            installer,
            audit,
            deadline: UPDATE_SEND_DEADLINE,
            sends: Arc::new(Semaphore::new(MOST_SENDS)),
            asked: Mutex::default(),
        }
    }

    /// With `deadline` for each send.
    #[cfg(test)]
    pub(crate) fn within(mut self, deadline: Duration) -> Self {
        self.deadline = deadline;
        self
    }

    /// Tell an environment whose hello named another protocol which one
    /// this gateway speaks. Nothing is recorded here: the refusal is, by
    /// whoever read the hello.
    pub(crate) async fn refuse_version(&self, to: &mut (dyn AsyncWrite + Send + Unpin)) {
        let refused = FromGateway::VersionRefused {
            protocol: self.installer.protocol().into(),
        };
        if let Err(error) = send(to, &refused).await {
            tracing::debug!(%error, "a version refusal could not be sent");
        }
        let _ = to.shutdown().await;
    }

    /// Send `host`, which asked for it, this build for its `platform`.
    pub(crate) async fn send_build(
        &self,
        host: &str,
        platform: Platform,
        to: &mut (dyn AsyncWrite + Send + Unpin),
    ) {
        let shown = platform.to_string();
        let Some(_sending) = self.admitted(host) else {
            tracing::info!(host, "a paired host asked for this build again too soon");
            let busy = FromGateway::UpdateRefused {
                reason: "busy".into(),
            };
            let _ = send(to, &busy).await;
            let _ = to.shutdown().await;
            return;
        };
        let opened = match self.installer.opened_for(&platform).await {
            Ok(opened) => opened,
            Err(NotOpened { reason, seen, .. }) => {
                self.refused(host, &shown, reason, seen);
                let refusal = FromGateway::UpdateRefused {
                    reason: refusal_word(reason).into(),
                };
                let _ = send(to, &refusal).await;
                let _ = to.shutdown().await;
                return;
            }
        };
        let digest = opened.build.digest.clone();
        let started = EnvironmentEvent::UpdateStarted {
            host: host.into(),
            platform: shown.clone(),
            digest: digest.clone(),
        };
        if let Err(error) = self.audit.record(&started) {
            tracing::error!(%error, "an update could not be recorded; nothing is sent");
            let _ = to.shutdown().await;
            return;
        }
        let sent = tokio::time::timeout(self.deadline, self.stream(opened, to)).await;
        match sent {
            Ok(Ok(())) => {
                let event = EnvironmentEvent::UpdateSent {
                    host: host.into(),
                    digest,
                };
                if let Err(error) = self.audit.record(&event) {
                    tracing::error!(%error, "a sent update could not be recorded");
                }
                tracing::info!(host, platform = shown, "sent this build to a paired host");
            }
            Ok(Err(error)) => {
                tracing::warn!(host, %error, "sending this build was cut short");
                self.refused(
                    host,
                    &shown,
                    InstallRefusal::Failed,
                    Some(error.to_string()),
                );
            }
            Err(_) => {
                tracing::warn!(host, "sending this build did not finish in time");
                self.refused(
                    host,
                    &shown,
                    InstallRefusal::Failed,
                    Some("deadline".into()),
                );
            }
        }
        let _ = to.shutdown().await;
    }

    /// A place among the sends, unless `host` began one a moment ago or
    /// every place is taken.
    fn admitted(&self, host: &str) -> Option<OwnedSemaphorePermit> {
        let now = Instant::now();
        let mut asked = self.asked.lock().unwrap_or_else(PoisonError::into_inner);
        asked.retain(|_, at| now.duration_since(*at) < HOST_ASKS_EVERY);
        if asked.contains_key(host) {
            return None;
        }
        let permit = self.sends.clone().try_acquire_owned().ok()?;
        asked.insert(host.to_owned(), now);
        Some(permit)
    }

    /// Every byte of `opened`, framed, then its end.
    async fn stream(
        &self,
        opened: OpenedCopy<'_>,
        to: &mut (dyn AsyncWrite + Send + Unpin),
    ) -> io::Result<()> {
        let OpenedCopy {
            build: Build { file, digest },
            sending,
        } = opened;
        // Held only while the copy was opened, which put the executable's
        // shared place back at its start: the parts are read by position,
        // which moves no place, so an SSH host's upload is not held up
        // behind this host's reading.
        drop(sending);
        let size = file.metadata()?.len();
        send(
            to,
            &FromGateway::Update {
                digest,
                size,
                protocol: self.installer.protocol().into(),
            },
        )
        .await?;
        let file = Arc::new(file);
        let mut at = 0;
        while at < size {
            // By position: the handle's place is an SSH upload's.
            let reading = file.clone();
            let bytes = tokio::task::spawn_blocking(move || {
                let mut buffer = vec![0; UPDATE_PART_BYTES];
                let read = std::os::unix::fs::FileExt::read_at(&*reading, &mut buffer, at)?;
                buffer.truncate(read);
                Ok::<_, io::Error>(buffer)
            })
            .await
            .map_err(io::Error::other)??;
            if bytes.is_empty() {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "the build ended before its size",
                ));
            }
            at += bytes.len() as u64;
            send(to, &part(&bytes)).await?;
        }
        send(to, &FromGateway::UpdateEnd).await
    }

    fn refused(&self, host: &str, platform: &str, reason: InstallRefusal, seen: Option<String>) {
        tracing::warn!(
            host,
            platform,
            ?reason,
            ?seen,
            "a paired host was sent no build"
        );
        let event = EnvironmentEvent::UpdateRefused {
            host: host.into(),
            platform: platform.into(),
            reason,
            seen,
        };
        if let Err(error) = self.audit.record(&event) {
            tracing::error!(%error, "a refused update could not be recorded");
        }
    }
}

/// The word an environment is told for `reason`.
fn refusal_word(reason: InstallRefusal) -> &'static str {
    match reason {
        InstallRefusal::Platform => "platform",
        InstallRefusal::Release => "release",
        _ => "source",
    }
}

async fn send(to: &mut (dyn AsyncWrite + Send + Unpin), frame: &FromGateway) -> io::Result<()> {
    let bytes = encode(frame).map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))?;
    to.write_all(&bytes).await?;
    to.flush().await
}

#[cfg(test)]
#[path = "../../../../tests/conversation/peer_update.rs"]
mod tests;
