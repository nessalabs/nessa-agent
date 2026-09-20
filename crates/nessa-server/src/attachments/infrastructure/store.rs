//! Private local files: bytes stored once per digest, and one small record per hold.
//!
//! ```text
//! <root>/incoming/.nessa-*.tmp                        transfers in progress
//! <root>/blobs/<stored digest hex>                    bytes, once per digest
//! <root>/holds/<sha256(organization)>/<conversation>/<stored digest hex>.json
//! ```
//!
//! Every name under the root is derived, never copied, from outside input: a
//! digest's 64 hexadecimal digits, a canonical lowercase UUID, and the hash of
//! an organization identifier, which may itself hold any character and differ
//! only by case. Files are 0600 in 0700 directories; an existing file or
//! directory that is not private is refused, never repaired.
//!
//! One lock orders every change, so "publish these bytes and hold them" and
//! "release these holds and remove bytes nothing holds" cannot interleave.
//! Transfers are written outside it. File work runs on the blocking pool.
use super::hold_record::{decode, encode};
use crate::{
    attachments::{
        application::{
            AttachmentStore, BlobOutcome, HoldChange, PortFuture, ReceivedBytes, ReleaseReport,
            ReleasedHold, StagedUpload, StoreUnavailable,
        },
        domain::{Attachment, Hold},
    },
    conversation::domain::ConversationId,
};
use nessa_auth::domain::OrganizationId;
use nessa_local_storage::{
    create_directory, create_directory_beneath, open_beneath, sync_directory,
    sync_directory_beneath, verify_directory, OpenMode, PrivateTempFile,
};
use nessa_sdk::domain::common::value_objects::Sha256Digest;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs,
    io::{self, Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

const BLOBS: &str = "blobs";
const INCOMING: &str = "incoming";
const HOLDS: &str = "holds";
const HOLD_SUFFIX: &str = ".json";
/// A hold record is a few hundred bytes; anything larger was not written here.
const MAX_HOLD_BYTES: u64 = 8192;

struct Files {
    root: PathBuf,
    changes: Mutex<()>,
}

/// Content-addressed attachment bytes with per-conversation holds, on local disk.
pub struct LocalAttachmentStore {
    files: Arc<Files>,
}

impl LocalAttachmentStore {
    /// Take a private directory, creating what is missing. Temporary files an
    /// interrupted transfer or publish left behind are removed here, once,
    /// before anything new is written; a running transfer's file is never
    /// swept, because nothing sweeps after this.
    ///
    /// # Errors
    /// The directory, or anything already in its place, is not private to this
    /// OS user, or cannot be created or read.
    pub fn open(root: PathBuf) -> io::Result<Self> {
        create_directory(&root)?;
        for name in [BLOBS, INCOMING, HOLDS] {
            create_directory(&root.join(name))?;
        }
        PrivateTempFile::clear_stale(&root.join(INCOMING))?;
        for organization in directories(&root.join(HOLDS))? {
            for conversation in directories(&organization)? {
                PrivateTempFile::clear_stale(&conversation)?;
            }
        }
        Ok(Self {
            files: Arc::new(Files {
                root,
                changes: Mutex::new(()),
            }),
        })
    }

    async fn blocking<T: Send + 'static>(
        &self,
        work: impl FnOnce(&Files) -> io::Result<T> + Send + 'static,
    ) -> Result<T, StoreUnavailable> {
        let files = self.files.clone();
        match tokio::task::spawn_blocking(move || work(&files)).await {
            Ok(Ok(value)) => Ok(value),
            Ok(Err(error)) => {
                tracing::error!(%error, "attachment storage failed");
                Err(StoreUnavailable)
            }
            Err(_) => Err(StoreUnavailable),
        }
    }
}

/// Real subdirectories only. A symbolic link is not followed and not listed.
fn directories(parent: &Path) -> io::Result<Vec<PathBuf>> {
    let mut found = Vec::new();
    for entry in fs::read_dir(parent)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            found.push(entry.path());
        }
    }
    Ok(found)
}

fn organization_directory(organization_id: &OrganizationId) -> String {
    Sha256Digest::from_bytes(Sha256::digest(organization_id.as_str().as_bytes()).into()).to_hex()
}

fn hold_directory(organization_id: &OrganizationId, conversation_id: &ConversationId) -> PathBuf {
    Path::new(HOLDS)
        .join(organization_directory(organization_id))
        .join(conversation_id.to_string())
}

fn hold_path(hold: &Hold) -> PathBuf {
    hold_directory(hold.organization_id(), hold.conversation_id())
        .join(format!("{}{HOLD_SUFFIX}", hold.stored().digest().to_hex()))
}

fn blob_path(digest: Sha256Digest) -> PathBuf {
    Path::new(BLOBS).join(digest.to_hex())
}

fn not_found<T>(result: io::Result<T>) -> io::Result<Option<T>> {
    match result {
        Ok(value) => Ok(Some(value)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(error),
    }
}

fn corrupt(what: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, what)
}

impl Files {
    fn lock(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.changes
            .lock()
            .map_err(|_| io::Error::other("attachment store lock poisoned"))
    }

    /// The hold saved at `relative`, which must describe the owner,
    /// conversation, and stored digest its own path says it does.
    fn read_hold(&self, relative: &Path) -> io::Result<Option<Hold>> {
        let Some(file) = not_found(open_beneath(&self.root, relative, OpenMode::Read))? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        file.take(MAX_HOLD_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_HOLD_BYTES {
            return Err(corrupt("hold record is too large"));
        }
        let hold = decode(&bytes).ok_or_else(|| corrupt("hold record is not valid"))?;
        if hold_path(&hold) != relative {
            return Err(corrupt("hold record describes another hold"));
        }
        Ok(Some(hold))
    }

    fn write_hold(&self, hold: &Hold) -> io::Result<()> {
        let directory = hold_directory(hold.organization_id(), hold.conversation_id());
        create_directory_beneath(&self.root, &directory)?;
        let mut file = PrivateTempFile::new_beneath(&self.root, &directory)?;
        file.as_file_mut().write_all(&encode(hold))?;
        file.as_file().sync_all()?;
        file.persist_beneath(&hold_path(hold))?;
        // The record's own directory, then each new name above it.
        sync_directory_beneath(&self.root, &directory)?;
        if let Some(organization) = directory.parent() {
            sync_directory_beneath(&self.root, organization)?;
        }
        sync_directory_beneath(&self.root, Path::new(HOLDS))
    }

    /// Remove a file beneath the root after proving its directory is a real,
    /// private directory reached without following a link.
    fn remove(&self, relative: &Path) -> io::Result<()> {
        let directory = relative
            .parent()
            .ok_or_else(|| corrupt("nothing to remove"))?;
        sync_directory_beneath(&self.root, directory)?;
        fs::remove_file(self.root.join(relative))?;
        sync_directory_beneath(&self.root, directory)
    }

    fn blob_size(&self, digest: Sha256Digest) -> io::Result<Option<u64>> {
        not_found(open_beneath(&self.root, &blob_path(digest), OpenMode::Read))?
            .map(|file| file.metadata().map(|metadata| metadata.len()))
            .transpose()
    }

    /// Every stored digest some hold still names, read from names alone so an
    /// unreadable record still protects its bytes.
    fn referenced(&self) -> io::Result<HashSet<String>> {
        let mut names = HashSet::new();
        for organization in directories(&self.root.join(HOLDS))? {
            for conversation in directories(&organization)? {
                for entry in fs::read_dir(conversation)? {
                    if let Some(digest) = entry?
                        .file_name()
                        .to_str()
                        .and_then(|name| name.strip_suffix(HOLD_SUFFIX))
                    {
                        names.insert(digest.to_owned());
                    }
                }
            }
        }
        Ok(names)
    }

    /// Remove bytes no hold names any more.
    fn remove_unheld(&self, digest: Sha256Digest) -> BlobOutcome {
        match self.referenced() {
            Ok(names) if names.contains(&digest.to_hex()) => BlobOutcome::StillHeld,
            Ok(_) => match not_found(self.remove(&blob_path(digest))) {
                Ok(Some(())) => BlobOutcome::Removed,
                Ok(None) => BlobOutcome::Missing,
                Err(error) => {
                    tracing::error!(%digest, %error, "unheld attachment bytes were not removed");
                    BlobOutcome::RemovalFailed
                }
            },
            // Without the list of holds nothing proves these bytes are unheld.
            Err(error) => {
                tracing::error!(%digest, %error, "attachment holds could not be listed");
                BlobOutcome::RemovalFailed
            }
        }
    }

    /// The saved holds of one conversation: each record's path, and the hold
    /// when it could be read.
    fn conversation_holds(
        &self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
    ) -> io::Result<Vec<(PathBuf, io::Result<Hold>)>> {
        let directory = hold_directory(organization_id, conversation_id);
        let absolute = self.root.join(&directory);
        if not_found(verify_directory(&absolute))?.is_none() {
            return Ok(Vec::new());
        }
        let mut holds = Vec::new();
        for entry in fs::read_dir(absolute)? {
            let name = entry?.file_name();
            let Some(name) = name.to_str().filter(|name| name.ends_with(HOLD_SUFFIX)) else {
                continue;
            };
            let relative = directory.join(name);
            let hold = self
                .read_hold(&relative)
                .and_then(|hold| hold.ok_or_else(|| corrupt("hold record vanished")));
            holds.push((relative, hold));
        }
        Ok(holds)
    }

    fn keeps(&self, hold: &Hold) -> io::Result<bool> {
        Ok(self.blob_size(hold.stored().digest())? == Some(hold.stored().size()))
    }

    fn publish(&self, file: PrivateTempFile, stored: &Attachment) -> io::Result<()> {
        let blobs = self.root.join(BLOBS);
        match file.publish(&blobs.join(stored.digest().to_hex())) {
            Ok(()) => sync_directory(&blobs),
            // The same bytes are already stored. They are trusted by name, and
            // refused if they cannot be the same bytes.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if self.blob_size(stored.digest())? == Some(stored.size()) {
                    Ok(())
                } else {
                    Err(corrupt("stored bytes disagree with their digest's size"))
                }
            }
            Err(error) => Err(error),
        }
    }
}

impl AttachmentStore for LocalAttachmentStore {
    fn holds<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        stored: &'a Attachment,
    ) -> PortFuture<'a, bool, StoreUnavailable> {
        let (organization_id, conversation_id, stored) = (
            organization_id.clone(),
            conversation_id.clone(),
            stored.clone(),
        );
        Box::pin(self.blocking(move |files| {
            let _changes = files.lock()?;
            let relative = hold_directory(&organization_id, &conversation_id)
                .join(format!("{}{HOLD_SUFFIX}", stored.digest().to_hex()));
            match files.read_hold(&relative)? {
                Some(hold) if hold.keeps(&organization_id, &conversation_id, &stored) => {
                    files.keeps(&hold)
                }
                _ => Ok(false),
            }
        }))
    }

    fn find_upload<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
        uploaded: &'a Attachment,
    ) -> PortFuture<'a, Option<Hold>, StoreUnavailable> {
        let (organization_id, conversation_id, uploaded) = (
            organization_id.clone(),
            conversation_id.clone(),
            uploaded.clone(),
        );
        Box::pin(self.blocking(move |files| {
            let _changes = files.lock()?;
            for (path, hold) in files.conversation_holds(&organization_id, &conversation_id)? {
                match hold {
                    Ok(hold) if hold.came_from(&organization_id, &conversation_id, &uploaded) => {
                        if files.keeps(&hold)? {
                            return Ok(Some(hold));
                        }
                    }
                    Ok(_) => {}
                    // One unreadable record does not stop a new upload. It is
                    // left exactly as it is for whoever investigates.
                    Err(error) => {
                        tracing::warn!(path = %path.display(), %error, "hold record unreadable")
                    }
                }
            }
            Ok(None)
        }))
    }

    fn stage(&self) -> PortFuture<'_, Box<dyn StagedUpload>, StoreUnavailable> {
        let owner = self.files.clone();
        Box::pin(self.blocking(move |files| {
            let file = PrivateTempFile::new_in(&files.root.join(INCOMING))?;
            Ok(Box::new(LocalStagedUpload {
                files: owner,
                writing: Some((file, Sha256::new())),
                size: 0,
                finished: None,
            }) as Box<dyn StagedUpload>)
        }))
    }

    fn revert(&self, hold: Hold, change: HoldChange) -> PortFuture<'_, (), StoreUnavailable> {
        Box::pin(self.blocking(move |files| {
            let _changes = files.lock()?;
            match change.previous {
                Some(previous) => files.write_hold(&previous),
                None => {
                    not_found(files.remove(&hold_path(&hold)))?;
                    match files.remove_unheld(hold.stored().digest()) {
                        BlobOutcome::RemovalFailed => {
                            Err(io::Error::other("unheld bytes were not removed"))
                        }
                        _ => Ok(()),
                    }
                }
            }
        }))
    }

    fn release<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
    ) -> PortFuture<'a, ReleaseReport, StoreUnavailable> {
        let (organization_id, conversation_id) = (organization_id.clone(), conversation_id.clone());
        Box::pin(self.blocking(move |files| {
            let _changes = files.lock()?;
            let mut report = ReleaseReport::default();
            let mut removed = Vec::new();
            for (path, hold) in files.conversation_holds(&organization_id, &conversation_id)? {
                // A record that cannot be read is not erased: its bytes stay
                // protected by its name, and the failure is reported.
                match hold.and_then(|hold| files.remove(&path).map(|()| hold)) {
                    Ok(hold) => removed.push(hold),
                    Err(error) => {
                        tracing::error!(path = %path.display(), %error, "hold was not released");
                        report.failures += 1;
                    }
                }
            }
            // Bytes go only after every hold of this conversation is gone, so
            // one reference check per digest sees the final state.
            for hold in removed {
                let blob = files.remove_unheld(hold.stored().digest());
                report.released.push(ReleasedHold { hold, blob });
            }
            let directory = files
                .root
                .join(hold_directory(&organization_id, &conversation_id));
            // Left in place when something remains in it.
            let _ = fs::remove_dir(directory);
            Ok(report)
        }))
    }

    fn read(
        &self,
        digest: Sha256Digest,
        limit: u64,
    ) -> PortFuture<'_, Option<Vec<u8>>, StoreUnavailable> {
        Box::pin(self.blocking(move |files| {
            // Opened under the lock so a publish in progress is never seen
            // half-linked; read outside it, since an open file outlives its name.
            let file = {
                let _changes = files.lock()?;
                not_found(open_beneath(
                    &files.root,
                    &blob_path(digest),
                    OpenMode::Read,
                ))?
            };
            let Some(file) = file else { return Ok(None) };
            let mut bytes = Vec::new();
            file.take(limit).read_to_end(&mut bytes)?;
            Ok(Some(bytes))
        }))
    }
}

/// One transfer in private temporary storage, hashed as it is written.
struct LocalStagedUpload {
    files: Arc<Files>,
    /// Taken by each blocking write and returned after it. A write abandoned
    /// midway never returns it, and the file is removed when it drops.
    writing: Option<(PrivateTempFile, Sha256)>,
    size: u64,
    finished: Option<ReceivedBytes>,
}

impl LocalStagedUpload {
    async fn with_file<T: Send + 'static>(
        &mut self,
        work: impl FnOnce(&mut PrivateTempFile, &mut Sha256) -> io::Result<T> + Send + 'static,
    ) -> Result<T, StoreUnavailable> {
        let (mut file, mut hasher) = self.writing.take().ok_or(StoreUnavailable)?;
        let (file, hasher, result) = tokio::task::spawn_blocking(move || {
            let result = work(&mut file, &mut hasher);
            (file, hasher, result)
        })
        .await
        .map_err(|_| StoreUnavailable)?;
        self.writing = Some((file, hasher));
        result.map_err(|error| {
            tracing::error!(%error, "attachment transfer storage failed");
            StoreUnavailable
        })
    }
}

impl StagedUpload for LocalStagedUpload {
    fn write(&mut self, chunk: Vec<u8>) -> PortFuture<'_, (), StoreUnavailable> {
        Box::pin(async move {
            if self.finished.is_some() {
                return Err(StoreUnavailable);
            }
            let length = chunk.len() as u64;
            self.with_file(move |file, hasher| {
                file.as_file_mut().write_all(&chunk)?;
                hasher.update(&chunk);
                Ok(())
            })
            .await?;
            self.size = self.size.saturating_add(length);
            Ok(())
        })
    }

    fn finish(&mut self) -> PortFuture<'_, ReceivedBytes, StoreUnavailable> {
        Box::pin(async move {
            let digest = self
                .with_file(|file, hasher| {
                    file.as_file().sync_all()?;
                    Ok(Sha256Digest::from_bytes(hasher.clone().finalize().into()))
                })
                .await?;
            let received = ReceivedBytes {
                size: self.size,
                digest,
            };
            self.finished = Some(received);
            Ok(received)
        })
    }

    fn read(&mut self) -> PortFuture<'_, Vec<u8>, StoreUnavailable> {
        Box::pin(async move {
            let size = self.finished.ok_or(StoreUnavailable)?.size;
            self.with_file(move |file, _| {
                let file = file.as_file_mut();
                file.seek(SeekFrom::Start(0))?;
                let mut bytes = Vec::with_capacity(size as usize);
                file.take(size).read_to_end(&mut bytes)?;
                Ok(bytes)
            })
            .await
        })
    }

    fn keep(mut self: Box<Self>, hold: Hold) -> PortFuture<'static, HoldChange, StoreUnavailable> {
        Box::pin(async move {
            // These bytes are published under the hold's stored digest, so they
            // must be the bytes that digest and size describe.
            let received = self.finished.ok_or(StoreUnavailable)?;
            if received.digest != hold.stored().digest() || received.size != hold.stored().size() {
                return Err(StoreUnavailable);
            }
            let (file, _) = self.writing.take().ok_or(StoreUnavailable)?;
            let files = self.files.clone();
            let kept = tokio::task::spawn_blocking(move || {
                let _changes = files.lock()?;
                let previous = files.read_hold(&hold_path(&hold))?;
                files.publish(file, hold.stored())?;
                if let Err(error) = files.write_hold(&hold) {
                    // Do not leave bytes behind that nothing came to hold.
                    if previous.is_none() {
                        let _ = files.remove_unheld(hold.stored().digest());
                    }
                    return Err(error);
                }
                Ok(HoldChange { previous })
            })
            .await
            .map_err(|_| StoreUnavailable)?;
            kept.map_err(|error: io::Error| {
                tracing::error!(%error, "attachment could not be kept");
                StoreUnavailable
            })
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/attachments/store.rs"]
mod tests;
