//! Where a lease's published files are staged, and where its harnesses ask
//! to publish them (issue #701).
//!
//! ```text
//! <data>/environment/outbox/            swept when serving starts
//!   <lease's digest>/<artifact>         one staged copy, 0600, synced
//! <private temporary directory>/        0700, one per serving process
//!   <16 hex digits of the lease>        the lease's publish point (a Unix socket)
//!
//! stage(path) ──▶ outbox.lock exclusive ──▶ absolute? ──▶ every link followed
//!   ──▶ inside the workspace?
//!   ──▶ open each name relative to a pinned directory, without following links
//!   ──▶ descriptor metadata says regular file?
//!   ──▶ copy to <lease>/<artifact>, hashed and counted as it is copied
//!   ──▶ StagedArtifact { name, media type, size, digest, path }
//!   ──▶ outbox.lock released
//! discard and close hold that same lock across the removal. The export pin
//! holds it shared for its whole read (`an_environment_outbox_cut_waits_for_the_writer_lock`).
//! publish point: one request line ──▶ PublishCall ──▶ one answer line
//! ```
//!
//! Arrows are steps, in order. What the gateway reads is the staged copy,
//! made once and hashed as it was made, so a file the harness changes or
//! replaces afterwards changes nothing the gateway receives. The publish
//! point is a socket in a directory only this user may enter; any process
//! of this user can reach it, which is no more than the harness itself, who
//! runs as this user, already can. Its address is short whatever the data
//! directory is called, since a Unix socket's path has a small bound
//! (104 bytes on macOS).
//!
//! # Source identity and ordering (#767)
//!
//! The workspace descriptor is captured once at startup, which assumes a
//! trusted workspace selection. Canonicalization keeps existing in-workspace
//! aliases usable; it supplies names, not authority to open an absolute path.
//! On Unix, openat + O_DIRECTORY + O_NOFOLLOW pin each directory identity and
//! O_NOFOLLOW refuses a final link. O_NONBLOCK prevents a substituted FIFO from
//! parking the source open; descriptor metadata then refuses non-regular files.
//!
//! | Ordering | Result | Regression |
//! | --- | --- | --- |
//! | Stable file, or stable alias to a file inside workspace | Stage descriptor bytes under requested name | a_file_in_the_workspace_is_staged_as_copied_and_hashed; a_file_published_through_a_link_keeps_the_name_it_was_asked_by |
//! | Ancestor becomes a link after canonicalization, before traversal | Unreadable; no staged file | a_directory_swapped_after_resolution_is_refused_without_staging |
//! | Final name becomes a link after canonicalization | Unreadable; no staged file | a_final_link_substituted_after_resolution_is_refused |
//! | Workspace path replaced after startup capture | Original pinned workspace remains the source | the_workspace_handle_survives_path_replacement |
//! | Source becomes a FIFO before open | Nonblocking open; non-regular refusal | a_fifo_substituted_after_resolution_is_refused |
//!
//! A held directory remains the same object if renamed; this is an identity
//! guarantee, not a check of current pathname ancestry after every rename.
//! Mounts, hard links, changes to bytes during copy, and hostile workspace
//! capture at startup are outside this boundary. This is not a filesystem
//! sandbox. This module is Unix-only; it provides no Windows artifact guarantee.
use crate::env_serve::application::{
    ArtifactOutbox, PublishAnswer, PublishCall, PublishPoint, PublishRequest,
};
use nessa_protocol::lease::{
    PublishRefusal, StagedArtifact, MAX_ARTIFACT_BYTES, MAX_ARTIFACT_NAME_BYTES,
    MAX_ARTIFACT_PATH_BYTES, MAX_OPEN_PUBLISHERS,
};
use nessa_sdk::domain::common::value_objects::MediaType;
use sha2::{Digest, Sha256};
use std::{
    collections::HashMap,
    ffi::CString,
    fs::{self, File, OpenOptions},
    io::{self, Read, Write},
    os::{
        fd::{AsRawFd, FromRawFd},
        unix::{
            ffi::OsStrExt,
            fs::{DirBuilderExt, FileExt, OpenOptionsExt},
        },
    },
    path::{Component, Path, PathBuf},
    sync::{Arc, Mutex, PoisonError},
    time::Duration,
};
use tokio::{
    io::{AsyncBufReadExt, AsyncWriteExt, BufReader},
    net::{UnixListener, UnixStream},
    sync::{mpsc, oneshot, Semaphore},
    task::JoinHandle,
};

/// Longest request line a publish point reads.
const MAX_REQUEST_BYTES: u64 = 8 * 1024;
/// How long a publisher may take to send its request.
const REQUEST_DEADLINE: Duration = Duration::from_secs(10);
/// Requests one publish point queues; more wait to be accepted.
const CALL_QUEUE: usize = 8;
/// How much is copied at a time.
const COPY_CHUNK: usize = 64 * 1024;

/// The outbox on this host's file system.
pub(crate) struct FileOutbox {
    root: PathBuf,
    sockets: PathBuf,
    workspace: PathBuf,
    workspace_directory: File,
    points: Mutex<HashMap<String, JoinHandle<()>>>,
}

impl FileOutbox {
    /// The outbox at `root` for harnesses working in `workspace`, with
    /// publish points under `sockets`. Whatever an earlier serving process
    /// left in `root` is let go first: every lease it staged for has ended,
    /// as one process serves this data directory at a time.
    ///
    /// # Errors
    /// The outbox, the workspace or the directory for publish points could
    /// not be made ready.
    pub(crate) fn open(root: PathBuf, sockets: PathBuf, workspace: &Path) -> io::Result<Self> {
        let workspace = fs::canonicalize(workspace)?;
        let workspace_directory = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW | libc::O_CLOEXEC)
            .open(&workspace)?;
        if let Some(parent) = root
            .parent()
            .filter(|parent| !parent.as_os_str().is_empty())
        {
            fs::DirBuilder::new()
                .recursive(true)
                .mode(0o700)
                .create(parent)?;
        }
        let outbox = Self {
            root,
            sockets,
            workspace,
            workspace_directory,
            points: Mutex::new(HashMap::new()),
        };
        // The sweep is the same exclusive section as a stage: an export pin
        // waiting on this lock does not read a half-deleted tree.
        let _lock = outbox.lock_outbox()?;
        match fs::remove_dir_all(&outbox.root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error),
        }
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&outbox.root)?;
        fs::DirBuilder::new().mode(0o700).create(&outbox.sockets)?;
        Ok(outbox)
    }

    /// `outbox.lock` beside the outbox directory, exclusive until dropped.
    /// The export pin holds the same file shared.
    /// `a_stage_and_a_close_wait_for_the_outbox_lock`.
    fn lock_outbox(&self) -> io::Result<File> {
        let file = nessa_local_storage::open(
            &self.root.with_extension("lock"),
            nessa_local_storage::OpenMode::OpenOrCreate,
        )?;
        file.lock()?;
        Ok(file)
    }

    /// Named by the lease's digest, never by what the gateway sent: a
    /// lease is any text, and never a path.
    fn lease_directory(&self, lease: &str) -> PathBuf {
        let digest = Sha256::digest(lease.as_bytes());
        let name: String = digest.iter().map(|byte| format!("{byte:02x}")).collect();
        self.root.join(name)
    }

    fn socket(&self, lease: &str) -> PathBuf {
        let digest = Sha256::digest(lease.as_bytes());
        let name: String = digest[..8]
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        self.sockets.join(name)
    }

    fn copy(
        &self,
        lease: &str,
        artifact: u32,
        request: &PublishRequest,
    ) -> Result<StagedArtifact, PublishRefusal> {
        let _lock = self
            .lock_outbox()
            .map_err(|_| PublishRefusal::StagingFailed)?;
        let requested = Path::new(&request.path);
        if !requested.is_absolute() {
            return Err(PublishRefusal::OutsideWorkspace);
        }
        let resolved = fs::canonicalize(requested).map_err(|_| PublishRefusal::Unreadable)?;
        self.copy_resolved(lease, artifact, request, &resolved)
    }

    /// Continue under the caller's outbox lock after path resolution. Opening
    /// re-establishes containment through directory handles, not a pathname stat.
    fn copy_resolved(
        &self,
        lease: &str,
        artifact: u32,
        request: &PublishRequest,
        resolved: &Path,
    ) -> Result<StagedArtifact, PublishRefusal> {
        let requested = Path::new(&request.path);
        if !resolved.starts_with(&self.workspace) {
            return Err(PublishRefusal::OutsideWorkspace);
        }
        // The agent's own name for the file, not its link target's: the
        // resolved path is only for containment and opening.
        let name = requested
            .file_name()
            .and_then(|name| name.to_str())
            .filter(|name| shown(name))
            .ok_or(PublishRefusal::Invalid)?
            .to_owned();
        let media_type = match &request.media_type {
            Some(media_type) if well_formed(media_type) => media_type.clone(),
            Some(_) => return Err(PublishRefusal::Invalid),
            None => media_type_of(&name).into(),
        };
        let mut source = self.open_source(resolved)?;
        let opened = source.metadata().map_err(|_| PublishRefusal::Unreadable)?;
        if !opened.is_file() {
            return Err(PublishRefusal::OutsideWorkspace);
        }
        if opened.len() == 0 || opened.len() > MAX_ARTIFACT_BYTES {
            return Err(PublishRefusal::Size);
        }
        let directory = self.lease_directory(lease);
        fs::DirBuilder::new()
            .recursive(true)
            .mode(0o700)
            .create(&directory)
            .map_err(|_| PublishRefusal::StagingFailed)?;
        let staged_path = directory.join(artifact.to_string());
        let path = staged_path
            .to_str()
            .filter(|path| path.len() <= MAX_ARTIFACT_PATH_BYTES)
            .ok_or(PublishRefusal::StagingFailed)?
            .to_owned();
        let mut staged = OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .custom_flags(libc::O_NOFOLLOW)
            .open(&staged_path)
            .map_err(|_| PublishRefusal::StagingFailed)?;
        let copied = copy_hashed(&mut source, &mut staged);
        let (size, digest) = match copied {
            Ok(copied) => copied,
            Err(refusal) => {
                drop(staged);
                let _ = fs::remove_file(&staged_path);
                return Err(refusal);
            }
        };
        Ok(StagedArtifact {
            name,
            media_type,
            size,
            digest,
            path,
        })
    }

    /// Canonical paths contain names only below the pinned workspace. Each
    /// openat refuses links; a renamed ancestor cannot redirect a held handle.
    fn open_source(&self, resolved: &Path) -> Result<File, PublishRefusal> {
        let relative = resolved
            .strip_prefix(&self.workspace)
            .map_err(|_| PublishRefusal::OutsideWorkspace)?;
        let mut parts = relative.components().peekable();
        let mut directory = self
            .workspace_directory
            .try_clone()
            .map_err(|_| PublishRefusal::Unreadable)?;
        while let Some(part) = parts.next() {
            let Component::Normal(name) = part else {
                return Err(PublishRefusal::OutsideWorkspace);
            };
            let name = CString::new(name.as_bytes()).map_err(|_| PublishRefusal::Invalid)?;
            let flags = libc::O_RDONLY
                | libc::O_NOFOLLOW
                | libc::O_CLOEXEC
                | if parts.peek().is_some() {
                    libc::O_DIRECTORY
                } else {
                    libc::O_NONBLOCK
                };
            // SAFETY: directory is live and name is NUL terminated for the call.
            let descriptor = unsafe { libc::openat(directory.as_raw_fd(), name.as_ptr(), flags) };
            if descriptor < 0 {
                return Err(PublishRefusal::Unreadable);
            }
            // SAFETY: openat returned a new descriptor owned only by this File.
            directory = unsafe { File::from_raw_fd(descriptor) };
        }
        Ok(directory)
    }
}

/// Copy all of `source` into `staged`, at most [`MAX_ARTIFACT_BYTES`], and
/// sync it: its length and SHA-256 as copied.
fn copy_hashed(source: &mut File, staged: &mut File) -> Result<(u64, String), PublishRefusal> {
    let mut hash = Sha256::new();
    let mut size = 0u64;
    let mut buffer = vec![0; COPY_CHUNK];
    loop {
        let read = match source.read(&mut buffer) {
            Ok(0) => break,
            Ok(read) => read,
            Err(error) if error.kind() == io::ErrorKind::Interrupted => continue,
            Err(_) => return Err(PublishRefusal::Unreadable),
        };
        size += read as u64;
        // A file that grew past the bound while it was copied.
        if size > MAX_ARTIFACT_BYTES {
            return Err(PublishRefusal::Size);
        }
        hash.update(&buffer[..read]);
        staged
            .write_all(&buffer[..read])
            .map_err(|_| PublishRefusal::StagingFailed)?;
    }
    if size == 0 {
        return Err(PublishRefusal::Size);
    }
    staged
        .sync_all()
        .map_err(|_| PublishRefusal::StagingFailed)?;
    let digest = hash
        .finalize()
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect();
    Ok((size, digest))
}

/// Whether `name` can be shown as a file's name: not too long, and no
/// control character.
fn shown(name: &str) -> bool {
    !name.is_empty() && name.len() <= MAX_ARTIFACT_NAME_BYTES && !name.chars().any(char::is_control)
}

/// Lowercase `type/subtype`: the one rule the gateway reads it by.
pub(crate) fn well_formed(media_type: &str) -> bool {
    MediaType::parse(media_type).is_ok()
}

/// The media type a file's extension names, for the files builds and
/// simulators make; anything else is bytes.
pub(crate) fn media_type_of(name: &str) -> &'static str {
    let extension = name
        .rsplit_once('.')
        .map(|(_, extension)| extension.to_ascii_lowercase())
        .unwrap_or_default();
    match extension.as_str() {
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "gif" => "image/gif",
        "webp" => "image/webp",
        "pdf" => "application/pdf",
        "dmg" => "application/x-apple-diskimage",
        "zip" => "application/zip",
        "gz" | "tgz" => "application/gzip",
        "json" => "application/json",
        "txt" | "log" => "text/plain",
        "html" => "text/html",
        "csv" => "text/csv",
        "mp4" => "video/mp4",
        "mov" => "video/quicktime",
        _ => "application/octet-stream",
    }
}

impl ArtifactOutbox for FileOutbox {
    fn open(&self, lease: &str) -> io::Result<PublishPoint> {
        let socket = self.socket(lease);
        // A point left by a lease of the same digits; there is none live.
        let _ = fs::remove_file(&socket);
        let address = socket
            .to_str()
            .ok_or_else(|| io::Error::other("the publish point's path is not UTF-8"))?
            .to_owned();
        let listener = UnixListener::bind(&socket)?;
        let (calls, received) = mpsc::channel(CALL_QUEUE);
        let task = tokio::spawn(accept(listener, calls));
        let previous = self
            .points
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(lease.to_owned(), task);
        if let Some(previous) = previous {
            previous.abort();
        }
        Ok(PublishPoint {
            address,
            calls: received,
        })
    }

    fn stage(
        &self,
        lease: &str,
        artifact: u32,
        request: &PublishRequest,
    ) -> Result<StagedArtifact, PublishRefusal> {
        self.copy(lease, artifact, request)
    }

    /// Opened without following a link: the copy is this outbox's own
    /// file, named by the lease's digest and the artifact's number.
    fn read(&self, lease: &str, artifact: u32, offset: u64, length: usize) -> io::Result<Vec<u8>> {
        let path = self.lease_directory(lease).join(artifact.to_string());
        let file = OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW)
            .open(path)?;
        let mut data = vec![0; length];
        let mut filled = 0;
        while filled < length {
            let at = offset.saturating_add(filled as u64);
            match file.read_at(&mut data[filled..], at) {
                Ok(0) => break,
                Ok(read) => filled += read,
                Err(error) if error.kind() == io::ErrorKind::Interrupted => {}
                Err(error) => return Err(error),
            }
        }
        data.truncate(filled);
        Ok(data)
    }

    fn discard(&self, lease: &str, artifact: u32) {
        let Ok(_lock) = self.lock_outbox() else {
            tracing::warn!(
                lease,
                artifact,
                "the outbox lock could not be taken to discard"
            );
            return;
        };
        let path = self.lease_directory(lease).join(artifact.to_string());
        if let Err(error) = fs::remove_file(&path) {
            if error.kind() != io::ErrorKind::NotFound {
                tracing::warn!(lease, artifact, %error, "a staged artifact could not be removed");
            }
        }
    }

    fn close(&self, lease: &str) {
        let task = self
            .points
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .remove(lease);
        if let Some(task) = task {
            task.abort();
        }
        let Ok(_lock) = self.lock_outbox() else {
            tracing::warn!(lease, "the outbox lock could not be taken to close");
            return;
        };
        let _ = fs::remove_file(self.socket(lease));
        match fs::remove_dir_all(self.lease_directory(lease)) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(lease, %error, "a lease's staged artifacts could not be removed");
            }
        }
    }
}

/// Take each publisher's one request and give it its one answer, holding at
/// most [`MAX_OPEN_PUBLISHERS`] open: a slot is taken before a connection is
/// accepted, so publishers that never send hold no more than that.
async fn accept(listener: UnixListener, calls: mpsc::Sender<PublishCall>) {
    let open = Arc::new(Semaphore::new(MAX_OPEN_PUBLISHERS));
    loop {
        let Ok(slot) = Arc::clone(&open).acquire_owned().await else {
            return;
        };
        let Ok((stream, _)) = listener.accept().await else {
            return;
        };
        let calls = calls.clone();
        tokio::spawn(async move {
            if let Err(error) = answer(stream, calls).await {
                tracing::debug!(%error, "a publisher went away before its answer");
            }
            drop(slot);
        });
    }
}

async fn answer(stream: UnixStream, calls: mpsc::Sender<PublishCall>) -> io::Result<()> {
    let (read, mut write) = stream.into_split();
    let mut line = String::new();
    let mut reader = BufReader::new(tokio::io::AsyncReadExt::take(read, MAX_REQUEST_BYTES));
    let request = match tokio::time::timeout(REQUEST_DEADLINE, reader.read_line(&mut line)).await {
        Ok(Ok(_)) => serde_json::from_str::<PublishRequest>(&line).ok(),
        _ => None,
    };
    let reply = match request {
        None => PublishAnswer::NotPublished {
            reason: PublishRefusal::Invalid,
        },
        Some(request) => {
            let (sender, answer) = oneshot::channel();
            let call = PublishCall {
                request,
                answer: sender,
            };
            match calls.send(call).await {
                Ok(()) => answer.await.unwrap_or(PublishAnswer::NotPublished {
                    reason: PublishRefusal::LeaseEnded,
                }),
                Err(_) => PublishAnswer::NotPublished {
                    reason: PublishRefusal::LeaseEnded,
                },
            }
        }
    };
    let mut bytes = serde_json::to_vec(&reply).map_err(io::Error::other)?;
    bytes.push(b'\n');
    write.write_all(&bytes).await?;
    write.shutdown().await
}

#[cfg(test)]
#[path = "../../../tests/env_serve/outbox.rs"]
mod tests;
