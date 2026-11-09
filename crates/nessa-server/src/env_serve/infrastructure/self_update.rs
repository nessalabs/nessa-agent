//! A paired environment taking its gateway's build when the gateway refused
//! this one's lease protocol (`nessa_protocol::build_update`), and running it
//! in place of this build, now and on every later start of this one.
//!
//! ```text
//! fetch(connection, protocol the gateway named)
//!   ──▶ UpdateWanted{this platform}
//!   ──▶ Update{digest, size, protocol} (protocol must be the one named)
//!   ──▶ UpdatePart … into a private file in `builds`, measured as it comes,
//!       never past `size` nor MOST_UPDATE_BYTES
//!   ──▶ UpdateEnd: size and SHA-256 are the ones said?        no ──▶ removed, Refused
//!   ──▶ made runnable; `<it> env protocol` says that protocol? no ──▶ removed, Refused
//!   ──▶ renamed to <builds>/<digest>/nessa
//!   ──▶ <builds>/current: "<digest> <the build it runs in place of>": this
//!       build, or, when this build is itself a copy current names, the one
//!       that copy runs in place of
//!   ──▶ every other build kept in `builds` removed
//! start of `nessa env dial`: current names a copy, put in place of this very
//!   build (its digest is the second word), and the copy is there?
//!   ──▶ exec it, with the same arguments; otherwise this build runs
//! ```
//!
//! Arrows are steps, in order. Nothing runs before every byte is checked,
//! and the rename is the only step that puts a copy where it is found, so a
//! copy cut short or refused is never run. Whether to ask at all is this
//! host's owner's (`admit_update`, decided by the lease policy), never the
//! gateway's. `current` names the build it replaced, so a newer image of
//! this machine (whose build is another) runs itself rather than an older
//! copy, and the copy, whose own digest is not that one, never execs again.
use crate::env_serve::application::{write_frame, FrameStream};
use crate::env_serve::install::Platform;
use nessa_protocol::build_update::{
    from_gateway, part_bytes, FromEnvironment, FromGateway, MOST_UPDATE_BYTES, UPDATE_SEND_DEADLINE,
};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::unix::fs::{OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    time::Duration,
};
use tokio::io::{AsyncRead, AsyncWrite};
use uuid::Uuid;

/// How long the copy may take to say its protocol.
const PROTOCOL_WAIT: Duration = Duration::from_secs(30);

/// How many times a copy that is momentarily busy is tried again.
const BUSY_TRIES: u64 = 8;

/// How long the whole fetch may take: the gateway's send, and a minute
/// more for dialing it and checking the copy.
pub(crate) const FETCH_DEADLINE: Duration =
    UPDATE_SEND_DEADLINE.saturating_add(Duration::from_secs(60));

/// Why no build was taken.
#[derive(Debug)]
pub(crate) enum NotUpdated {
    /// The gateway sent none, and said why.
    Refused(String),
    /// What it sent is not what it said, or does not speak that protocol.
    Unverified(&'static str),
    /// The connection ended, or said something else, before the end.
    Cut,
    /// A file here could not be written, read or run.
    Io(io::Error),
}

impl std::fmt::Display for NotUpdated {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Refused(reason) => write!(formatter, "the gateway sent no build: {reason}"),
            Self::Unverified(what) => write!(formatter, "the build sent was refused: {what}"),
            Self::Cut => formatter.write_str("the connection ended before the build did"),
            Self::Io(error) => write!(formatter, "the build could not be kept: {error}"),
        }
    }
}

impl From<io::Error> for NotUpdated {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}

/// Ask the gateway on `input`/`output` for its build, which it said speaks
/// `protocol`, and keep it in `builds`, in place of the build whose
/// SHA-256 is `this_build`. Answers where it is kept.
///
/// # Errors
/// [`NotUpdated`]; nothing is kept then.
pub(crate) async fn fetch<R, W>(
    input: R,
    mut output: W,
    protocol: &str,
    builds: &Path,
    this_build: &str,
) -> Result<PathBuf, NotUpdated>
where
    R: AsyncRead + Unpin,
    W: AsyncWrite + Unpin,
{
    let platform = Platform::this_build();
    write_frame(
        &mut output,
        &FromEnvironment::UpdateWanted {
            os: platform.os,
            arch: platform.arch,
            libc: platform.libc,
        },
    )
    .await?;
    let mut stream = FrameStream::new(input);
    let (digest, size) = match next(&mut stream).await? {
        FromGateway::Update {
            digest,
            size,
            protocol: sent,
        } => {
            if sent != protocol {
                return Err(NotUpdated::Unverified(
                    "another protocol than the one refused",
                ));
            }
            if !is_digest(&digest) {
                return Err(NotUpdated::Unverified("its digest"));
            }
            if size > MOST_UPDATE_BYTES {
                return Err(NotUpdated::Unverified("its size"));
            }
            (digest, size)
        }
        FromGateway::UpdateRefused { reason } => return Err(NotUpdated::Refused(reason)),
        _ => return Err(NotUpdated::Cut),
    };
    create_private(builds)?;
    let mut part = Part::new(builds)?;
    let mut hasher = Sha256::new();
    let mut written: u64 = 0;
    loop {
        match next(&mut stream).await? {
            FromGateway::UpdatePart { data } => {
                let bytes = part_bytes(&data).ok_or(NotUpdated::Cut)?;
                written += bytes.len() as u64;
                if written > size {
                    return Err(NotUpdated::Unverified("more bytes than its size"));
                }
                hasher.update(&bytes);
                part.write(&bytes)?;
            }
            FromGateway::UpdateEnd => break,
            _ => return Err(NotUpdated::Cut),
        }
    }
    if written != size {
        return Err(NotUpdated::Unverified("fewer bytes than its size"));
    }
    if format!("{:x}", hasher.finalize()) != digest {
        return Err(NotUpdated::Unverified("its SHA-256"));
    }
    part.finish()?;
    let says = protocol_of(part.path()).await?;
    if says != protocol {
        return Err(NotUpdated::Unverified("the protocol it says"));
    }
    let kept = part.place(&builds.join(&digest))?;
    record_current(builds, &digest, &replaced_by(builds, this_build))?;
    prune(builds, &digest);
    Ok(kept)
}

/// Remove every build kept in `builds` but `keep`, and whatever a fetch cut
/// off by the process ending left: only `current` and the copy it names
/// are ever run. A copy running now goes on running once removed.
fn prune(builds: &Path, keep: &str) {
    let Ok(entries) = fs::read_dir(builds) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let removed = if is_digest(name) && name != keep {
            fs::remove_dir_all(entry.path())
        } else if name.starts_with(".update-") || name.starts_with(".current-") {
            fs::remove_file(entry.path())
        } else {
            continue;
        };
        if let Err(error) = removed {
            tracing::debug!(%error, name, "an earlier build could not be removed");
        }
    }
}

/// The next frame of the exchange.
async fn next<R: AsyncRead + Unpin>(
    stream: &mut FrameStream<R>,
) -> Result<FromGateway, NotUpdated> {
    match stream.next().await {
        Ok(Some(body)) => from_gateway(&body).ok_or(NotUpdated::Cut),
        Ok(None) | Err(_) => Err(NotUpdated::Cut),
    }
}

fn is_digest(digest: &str) -> bool {
    digest.len() == 64
        && digest
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn create_private(directory: &Path) -> io::Result<()> {
    fs::create_dir_all(directory)?;
    fs::set_permissions(directory, fs::Permissions::from_mode(0o700))
}

/// A build arriving: a private file beside where it is kept, removed unless
/// it is placed.
struct Part {
    path: PathBuf,
    /// Open while bytes arrive, and closed before the copy is run: a file
    /// still open for writing cannot be executed.
    file: Option<File>,
    placed: bool,
}

impl Part {
    fn new(builds: &Path) -> io::Result<Self> {
        let path = builds.join(format!(".update-{}", Uuid::new_v4()));
        let file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)?;
        Ok(Self {
            path,
            file: Some(file),
            placed: false,
        })
    }

    fn path(&self) -> &Path {
        &self.path
    }

    fn write(&mut self, bytes: &[u8]) -> io::Result<()> {
        match &mut self.file {
            Some(file) => file.write_all(bytes),
            None => Err(io::Error::other("the build was already finished")),
        }
    }

    /// Durable, closed, and runnable by this account alone.
    fn finish(&mut self) -> io::Result<()> {
        if let Some(file) = self.file.take() {
            file.sync_all()?;
        }
        fs::set_permissions(&self.path, fs::Permissions::from_mode(0o700))
    }

    /// Kept as `directory/nessa`. One already there was checked when it was
    /// put there, by this or by a gateway's install, and is kept as it is.
    fn place(mut self, directory: &Path) -> io::Result<PathBuf> {
        create_private(directory)?;
        let kept = directory.join("nessa");
        if !kept.exists() {
            fs::rename(&self.path, &kept)?;
            self.placed = true;
            File::open(directory)?.sync_all()?;
        }
        Ok(kept)
    }
}

impl Drop for Part {
    fn drop(&mut self) {
        if !self.placed {
            let _ = fs::remove_file(&self.path);
        }
    }
}

/// What `nessa env protocol` prints when the copy at `path` runs it.
async fn protocol_of(path: &Path) -> Result<String, NotUpdated> {
    let mut command = tokio::process::Command::new(path);
    command
        .args(["env", "protocol"])
        .env_clear()
        .stdin(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .kill_on_drop(true);
    // A process this one forks meanwhile holds a copy of the descriptor the
    // file was written through until it execs itself, and while it does the
    // file cannot be run ("text file busy"): that passes in a moment.
    let mut busy = 0;
    let output = loop {
        let ran = tokio::time::timeout(PROTOCOL_WAIT, command.output())
            .await
            .map_err(|_| NotUpdated::Unverified("it did not say its protocol in time"))?;
        match ran {
            Err(error) if error.raw_os_error() == Some(libc::ETXTBSY) && busy < BUSY_TRIES => {
                busy += 1;
                tokio::time::sleep(Duration::from_millis(50 * busy)).await;
            }
            ran => break ran.map_err(|_| NotUpdated::Unverified("it does not run here"))?,
        }
    };
    if !output.status.success() {
        return Err(NotUpdated::Unverified("it does not run here"));
    }
    Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
}

/// The file naming the copy run in place of a build.
fn current_file(builds: &Path) -> PathBuf {
    builds.join("current")
}

fn record_current(builds: &Path, digest: &str, replaced: &str) -> io::Result<()> {
    let temporary = builds.join(format!(".current-{}", Uuid::new_v4()));
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(&temporary)?;
    let written = file
        .write_all(format!("{digest} {replaced}\n").as_bytes())
        .and_then(|()| file.sync_all())
        .and_then(|()| fs::rename(&temporary, current_file(builds)));
    if written.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    written?;
    File::open(builds)?.sync_all()
}

/// The build a copy fetched now is run in place of: the one `current`
/// already names when `this_build` is the copy run for it, so a machine
/// started from its image again (A, which fetched B, which fetched C) runs
/// the newest copy (C) rather than fetching it once more; else this build.
fn replaced_by(builds: &Path, this_build: &str) -> String {
    match recorded(builds) {
        Some((digest, replaced)) if digest == this_build && is_digest(&replaced) => replaced,
        _ => this_build.to_owned(),
    }
}

/// What `current` says: the copy kept, and the build it is run in place of.
fn recorded(builds: &Path) -> Option<(String, String)> {
    let mut recorded = String::new();
    File::open(current_file(builds))
        .ok()?
        .take(256)
        .read_to_string(&mut recorded)
        .ok()?;
    let mut words = recorded.split_whitespace();
    Some((words.next()?.to_owned(), words.next()?.to_owned()))
}

/// The copy kept in `builds` to run in place of the build whose SHA-256 is
/// `this_build`, when one was fetched for it and is still there.
pub(crate) fn kept_for(builds: &Path, this_build: &str) -> Option<PathBuf> {
    let (digest, replaced) = recorded(builds)?;
    if replaced != this_build || digest == this_build || !is_digest(&digest) {
        return None;
    }
    let kept = builds.join(digest).join("nessa");
    kept.is_file().then_some(kept)
}

/// The SHA-256 of the build running now.
///
/// # Errors
/// Its executable could not be read.
pub(crate) fn this_build() -> io::Result<String> {
    let path = if cfg!(target_os = "linux") {
        PathBuf::from("/proc/self/exe")
    } else {
        std::env::current_exe()?
    };
    let mut file = File::open(path)?;
    let mut hasher = Sha256::new();
    io::copy(&mut file, &mut hasher)?;
    Ok(format!("{:x}", hasher.finalize()))
}

/// Run `copy` in place of this process, with the arguments this one was
/// started with. Answers only when it could not be started.
pub(crate) fn run_in_place(copy: &Path) -> io::Error {
    use std::os::unix::process::CommandExt;
    let arguments: Vec<OsString> = std::env::args_os().skip(1).collect();
    std::process::Command::new(copy).args(arguments).exec()
}

#[cfg(test)]
#[path = "../../../tests/env_serve/self_update.rs"]
mod tests;
