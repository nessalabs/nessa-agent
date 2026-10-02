//! The relay socket: where stand-ins reach the gateway's one connection to
//! each server.
//!
//! ```text
//! nessa mcp-relay ──hello {server, configuration}──▶ Relay::serve
//!                 ◀─{accepted} or {refused, message}─┘   │ admit, then McpServers::stand_in
//!                 ◀═══ MCP frames, both ways ═══════════▶ StandIn::serve
//! ```
//!
//! One line each way, then the stand-in's bytes. A hello that is not one
//! JSON line of at most [`MAX_HELLO_BYTES`] within [`HELLO_TIMEOUT`] is closed
//! without an answer.
use super::grants::ConversationGrants;
use crate::mcp_servers::domain::{admit, StandInRefusal};
use nessa_sdk::infrastructure::mcp::{McpServers, INITIALIZE_TIMEOUT};
use serde::{Deserialize, Serialize};
use std::{collections::BTreeMap, io, time::Duration};
use tokio::io::{
    AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt, BufReader,
};

/// The longest hello or answer line, newline included.
pub const MAX_HELLO_BYTES: usize = 4096;
/// How long a stand-in has to say hello once connected.
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(5);
/// How long a stand-in waits for its answer: the server may be starting.
pub const ANSWER_TIMEOUT: Duration = INITIALIZE_TIMEOUT.saturating_add(HELLO_TIMEOUT);

/// What a stand-in says first.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub server: String,
    pub configuration: String,
    /// The session token from the stand-in's environment; empty when it has
    /// none.
    pub session: String,
}

/// What the gateway answers.
#[derive(Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case", deny_unknown_fields)]
pub enum Answer {
    Accepted,
    Refused { reason: Refusal, message: String },
}

/// [`StandInRefusal`] on the wire.
#[derive(Clone, Copy, Debug, Serialize, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum Refusal {
    UnknownSession,
    UnknownServer,
    ConfigurationChanged,
    Unavailable,
}
impl From<StandInRefusal> for Refusal {
    fn from(refusal: StandInRefusal) -> Self {
        match refusal {
            StandInRefusal::UnknownSession => Self::UnknownSession,
            StandInRefusal::UnknownServer => Self::UnknownServer,
            StandInRefusal::ConfigurationChanged => Self::ConfigurationChanged,
            StandInRefusal::Unavailable => Self::Unavailable,
        }
    }
}

/// One JSON line of at most [`MAX_HELLO_BYTES`], newline included, or `None`.
pub async fn read_line<T: for<'de> Deserialize<'de>>(
    input: &mut (impl AsyncBufRead + Unpin),
) -> Option<T> {
    let mut line = Vec::new();
    input
        .take(MAX_HELLO_BYTES as u64)
        .read_until(b'\n', &mut line)
        .await
        .ok()?;
    if line.last() != Some(&b'\n') {
        return None;
    }
    serde_json::from_slice(&line).ok()
}

/// `value` as one line.
pub async fn write_line(
    output: &mut (impl AsyncWrite + Unpin),
    value: &impl Serialize,
) -> io::Result<()> {
    let mut line = serde_json::to_vec(value).map_err(io::Error::other)?;
    line.push(b'\n');
    output.write_all(&line).await?;
    output.flush().await
}

/// The gateway's side of the relay socket.
pub struct Relay {
    servers: McpServers,
    /// Each configured server's configuration digest, by name.
    configured: BTreeMap<String, String>,
    /// The tokens issued to open conversations' harnesses.
    grants: ConversationGrants,
}
impl Relay {
    pub fn new(
        servers: McpServers,
        configured: BTreeMap<String, String>,
        grants: ConversationGrants,
    ) -> Self {
        Self {
            servers,
            configured,
            grants,
        }
    }

    /// Serve one stand-in's connection until it, or its server, ends. Its end —
    /// a clean close, a broken socket, a relay killed outright — closes its
    /// session and stops its server's process group.
    pub async fn serve(&self, connection: impl AsyncRead + AsyncWrite + Unpin) {
        let (input, mut output) = tokio::io::split(connection);
        let mut input = BufReader::new(input);
        let Ok(Some(hello)) =
            tokio::time::timeout(HELLO_TIMEOUT, read_line::<Hello>(&mut input)).await
        else {
            return;
        };
        let refused = |reason: StandInRefusal, message: String| Answer::Refused {
            reason: reason.into(),
            message,
        };
        // Whose it is, before anything about the server is said.
        let Some(owner) = self.grants.owner(&hello.session) else {
            let message = "this MCP stand-in belongs to no open conversation; start a new session";
            let answer = refused(StandInRefusal::UnknownSession, message.into());
            let _ = write_line(&mut output, &answer).await;
            return;
        };
        if let Err(reason) = admit(&hello.server, &hello.configuration, &self.configured) {
            let message = match reason {
                StandInRefusal::UnknownServer => "no MCP server is configured under that name",
                _ => "the MCP server is configured differently now; start a new session",
            };
            let _ = write_line(&mut output, &refused(reason, message.into())).await;
            return;
        }
        // One session, and one server process, for this stand-in alone: its
        // harness session's, ended with it or with its grant.
        let session = match self.servers.open(&hello.server, owner).await {
            Ok(session) => session,
            Err(error) => {
                let answer = refused(StandInRefusal::Unavailable, said(&error.to_string()));
                let _ = write_line(&mut output, &answer).await;
                return;
            }
        };
        if write_line(&mut output, &Answer::Accepted).await.is_err() {
            session.close().await;
            return;
        }
        // The reader keeps whatever it buffered past the hello.
        session.serve(input, output).await;
    }

    /// Accept stand-ins on `relay` until the task is dropped.
    #[cfg(unix)]
    pub async fn listen(self: std::sync::Arc<Self>, relay: BoundRelay) {
        loop {
            match relay.listener.accept().await {
                Ok((connection, _)) => {
                    let relay = self.clone();
                    tokio::spawn(async move { relay.serve(connection).await });
                }
                Err(error) => {
                    tracing::warn!(%error, "MCP relay socket could not accept a stand-in");
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
            }
        }
    }
}

/// `message` as a refusal says it: control characters as spaces, at most
/// 512 characters, so the answer stays one line within [`MAX_HELLO_BYTES`]
/// however JSON escapes it.
pub(crate) fn said(message: &str) -> String {
    message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(512)
        .collect()
}

/// The relay socket, bound, and the lock that says this gateway holds it.
/// Both are held for as long as this lives; the lock is released when it is
/// dropped or the process ends, however it ends.
#[cfg(unix)]
#[derive(Debug)]
pub struct BoundRelay {
    listener: tokio::net::UnixListener,
    _lock: std::fs::File,
}

/// Bind the relay socket at `socket`, in a private directory of its own.
///
/// An exclusive lock on `<socket>.lock` beside it, held as long as the
/// [`BoundRelay`], decides which gateway holds the socket: while another
/// holds it, the bind fails with `AddrInUse` and nothing is touched; once
/// none does, a socket an earlier run left behind is replaced. Anything at
/// the path that is not a socket fails the bind.
/// `<socket>.lock`: the lock beside the relay socket, named after all of it.
#[cfg(unix)]
pub(crate) fn lock_path(socket: &std::path::Path) -> std::path::PathBuf {
    let mut name = socket.as_os_str().to_owned();
    name.push(".lock");
    name.into()
}

#[cfg(unix)]
pub async fn bind(socket: &std::path::Path) -> io::Result<BoundRelay> {
    use std::os::unix::fs::{FileTypeExt, OpenOptionsExt};
    use std::os::unix::io::AsRawFd;
    let directory = socket
        .parent()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "socket has no directory"))?;
    nessa_local_storage::create_directory(directory)?;
    let lock = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .write(true)
        .mode(0o600)
        .custom_flags(libc::O_NOFOLLOW)
        .open(lock_path(socket))?;
    // SAFETY: flock has no memory preconditions; the descriptor is open.
    if unsafe { libc::flock(lock.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } != 0 {
        return Err(io::Error::new(
            io::ErrorKind::AddrInUse,
            "another gateway holds this relay socket",
        ));
    }
    match std::fs::symlink_metadata(socket) {
        Ok(found) if found.file_type().is_socket() => std::fs::remove_file(socket)?,
        Ok(_) => return Err(io::Error::new(io::ErrorKind::AlreadyExists, "not a socket")),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error),
    }
    Ok(BoundRelay {
        listener: tokio::net::UnixListener::bind(socket)?,
        _lock: lock,
    })
}
