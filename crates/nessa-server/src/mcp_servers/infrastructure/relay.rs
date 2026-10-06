//! The relay socket: where stand-ins reach the gateway's one connection to
//! each server.
//!
//! ```text
//! nessa mcp-relay ──hello {server, configuration}──▶ Relay::serve
//!                 ◀─{accepted} or {refused, message}─┘   │ admit against McpServers::configured, then open_as what was admitted
//!                 ◀═══ MCP frames, both ways ═══════════▶ StandIn::serve
//! ```
//!
//! One line each way, then the stand-in's bytes. A hello that is not one
//! JSON line of at most [`MAX_HELLO_BYTES`] within [`HELLO_TIMEOUT`] is closed
//! without an answer.
use super::grants::ConversationGrants;
use crate::mcp_servers::domain::{
    admit, configuration_digest, remote_configuration_digest, ConfigurationKey, StandInRefusal,
};
use nessa_sdk::infrastructure::mcp::{
    McpError, McpOwner, McpServerLaunch, McpServers, McpSession, RemoteMcpServer,
    INITIALIZE_TIMEOUT,
};
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
/// What a stand-in whose token no live grant holds is told.
const NO_CONVERSATION: &str =
    "this MCP stand-in belongs to no open conversation; start a new session";
/// What a stand-in of a server since removed is told.
const UNKNOWN: &str = "no MCP server is configured under that name";
/// What a stand-in of a server since edited is told.
const CHANGED: &str = "the MCP server is configured differently now; start a new session";

/// The digest a stand-in for `launch` carries, keyed with this process's
/// `key`: its command, arguments and whole environment
/// ([`configuration_digest`]). The one place a launch's fields are chosen
/// for it, read by the stand-ins handed to each open and by the relay that
/// admits them. The working directory is the gateway's, one for every server
/// in a run, and not in it.
pub fn launch_digest(key: &ConfigurationKey, launch: &McpServerLaunch) -> String {
    configuration_digest(
        key,
        &launch.server.command,
        &launch.server.args,
        &launch.environment,
    )
}

/// What a stand-in says first.
#[derive(Serialize, Deserialize, PartialEq, Eq)]
#[serde(deny_unknown_fields)]
pub struct Hello {
    pub server: String,
    pub configuration: String,
    /// The session token from the stand-in's environment; empty when it has
    /// none.
    pub session: String,
}

impl std::fmt::Debug for Hello {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // The session token is a bearer secret: never printed.
        f.debug_struct("Hello")
            .field("server", &self.server)
            .field("configuration", &self.configuration)
            .field("session", &"..")
            .finish()
    }
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

/// Why, and with what message, a stand-in whose session could not open is
/// refused. An opening refused [`McpError::Closed`] had its grant revoked
/// while it opened — the only reason `open` gives it — so it is as stale as a
/// token refused at the door; one refused
/// [`McpError::ConfigurationChanged`] or [`McpError::NotConfigured`] met a
/// set replaced since its admission, and is refused as the admission would
/// refuse it now; anything else, its server could not be made ready.
pub(crate) fn opening_refused(error: McpError) -> (StandInRefusal, String) {
    match error {
        McpError::Closed => (StandInRefusal::UnknownSession, NO_CONVERSATION.into()),
        // Replaced between the admission and the opening: refused as it
        // would have been had the replacement landed first.
        McpError::ConfigurationChanged => (StandInRefusal::ConfigurationChanged, CHANGED.into()),
        McpError::NotConfigured => (StandInRefusal::UnknownServer, UNKNOWN.into()),
        error => (StandInRefusal::Unavailable, said(&error.to_string())),
    }
}

/// What a hello was admitted against: the stdio launch or the remote server
/// as configured at admission. Opening uses that value, so a replacement
/// since is [`McpError::ConfigurationChanged`].
pub(crate) enum AdmittedServer {
    /// A local process.
    Stdio(McpServerLaunch),
    /// A remote endpoint.
    Remote(RemoteMcpServer),
}

/// The gateway's side of the relay socket.
pub struct Relay {
    /// The live set, whose digests each hello is admitted against as they
    /// are then, and where its session is opened.
    servers: McpServers,
    /// The tokens issued to open conversations' harnesses.
    grants: ConversationGrants,
    /// This process's key for each configuration's digest.
    key: ConfigurationKey,
}
impl Relay {
    pub fn new(servers: McpServers, grants: ConversationGrants, key: ConfigurationKey) -> Self {
        Self {
            servers,
            grants,
            key,
        }
    }

    /// The server `hello` names as it is configured now, when the hello's
    /// digest is that configuration's ([`launch_digest`]): against the set
    /// as it is now, a stand-in of a server since edited — its environment
    /// alone included — is refused `configuration-changed`, of one since
    /// removed `unknown-server`.
    pub(crate) fn admitted(&self, hello: &Hello) -> Result<AdmittedServer, StandInRefusal> {
        let configured = self.servers.configured();
        let remotes = self.servers.configured_remotes();
        let mut digests = configured
            .iter()
            .map(|launch| (launch.server.name.clone(), launch_digest(&self.key, launch)))
            .collect::<BTreeMap<_, _>>();
        for remote in &remotes {
            digests.insert(
                remote.name().to_owned(),
                remote_configuration_digest(
                    &self.key,
                    &remote.id().to_string(),
                    remote.url().as_str(),
                ),
            );
        }
        admit(&hello.server, &hello.configuration, &digests)?;
        if let Some(launch) = configured
            .into_iter()
            .find(|launch| launch.server.name == hello.server)
        {
            return Ok(AdmittedServer::Stdio(launch));
        }
        remotes
            .into_iter()
            .find(|remote| remote.name() == hello.server)
            .map(AdmittedServer::Remote)
            .ok_or(StandInRefusal::UnknownServer)
    }

    /// One session, and one server process, on `admitted` for `owner`'s
    /// stand-in alone: its harness session's, ended with it or with its
    /// grant. Opened only on the configuration the stand-in was admitted
    /// against: a set replaced since is refused as the admission would refuse
    /// it now ([`McpServers::open_as`]).
    pub(crate) async fn open_admitted(
        &self,
        admitted: &AdmittedServer,
        owner: McpOwner,
    ) -> Result<McpSession, (StandInRefusal, String)> {
        match admitted {
            AdmittedServer::Stdio(launch) => self.servers.open_as(launch, owner).await,
            AdmittedServer::Remote(remote) => self.servers.open_remote_as(remote, owner).await,
        }
        .map_err(opening_refused)
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
            let message = NO_CONVERSATION;
            let answer = refused(StandInRefusal::UnknownSession, message.into());
            let _ = write_line(&mut output, &answer).await;
            return;
        };
        let admitted = match self.admitted(&hello) {
            Ok(admitted) => admitted,
            Err(reason) => {
                let message = match reason {
                    StandInRefusal::UnknownServer => UNKNOWN,
                    _ => CHANGED,
                };
                let _ = write_line(&mut output, &refused(reason, message.into())).await;
                return;
            }
        };
        let session = match self.open_admitted(&admitted, owner).await {
            Ok(session) => session,
            Err((reason, message)) => {
                let _ = write_line(&mut output, &refused(reason, message)).await;
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
