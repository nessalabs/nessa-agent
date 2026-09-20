//! Private local files: bytes stored once per digest, and one small record per hold.
//!
//! ```text
//! <root>/incoming/.nessa-*.tmp                        transfers in progress
//! <root>/blobs/<stored digest hex>                    bytes, once per digest
//! <root>/holds/<sha256(organization)>/<conversation>/<stored digest hex>-<media type tag>.json
//! ```
//!
//! Every name under the root is derived, never copied, from outside input: a
//! digest's 64 hexadecimal digits, a canonical lowercase UUID, the hash of an
//! organization identifier (which may itself hold any character and differ
//! only by case), and sixteen digits of the hash of a media type. Files are
//! 0600 in 0700 directories; an existing file or directory that is not private
//! is refused, never repaired.
//!
//! A hold is one conversation keeping one stored file, digest and media type
//! together, so the same bytes kept as two types are two records and neither
//! replaces the other. A usable record is never overwritten.
//!
//! A record is written `pending` and carries the generation of the write that
//! made it. Nothing that asks what a conversation holds sees a pending record.
//! Only the bearer of that generation's claim makes it `kept` or takes it
//! back, so an upload undoes exactly its own write: never a later upload of
//! the same file, never a record a release already removed. A release removes
//! pending records too. A pending record a crash leaves behind stays invisible;
//! it still protects its bytes, a later upload of the same file replaces it,
//! and its conversation's next close removes it.
//!
//! One lock orders every change, so "publish these bytes and hold them" and
//! "release these holds and remove bytes nothing holds" cannot interleave.
//! Transfers are written outside it. File work runs on the blocking pool.
use super::hold_record::{decode, encode, HoldRecord, RecordState};
use crate::{
    attachments::{
        application::{
            AttachmentStore, Confirmation, Discard, HoldClaim, Kept, PortFuture, ReceivedBytes,
            ReleaseReport, ReleasedHold, StagedUpload, StoreUnavailable,
        },
        domain::{Attachment, Hold, HoldState},
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
use uuid::Uuid;

const BLOBS: &str = "blobs";
const INCOMING: &str = "incoming";
const HOLDS: &str = "holds";
const HOLD_SUFFIX: &str = ".json";
const DIGEST_HEX: usize = 64;
/// A hold record is well under a kilobyte of plain text; identifiers full of
/// characters JSON must escape can reach a few. Larger was not written here.
const MAX_HOLD_BYTES: u64 = 16 * 1024;

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

fn hex_of(bytes: &[u8]) -> String {
    Sha256Digest::from_bytes(Sha256::digest(bytes).into()).to_hex()
}

fn organization_directory(organization_id: &OrganizationId) -> String {
    hex_of(organization_id.as_str().as_bytes())
}

fn hold_directory(organization_id: &OrganizationId, conversation_id: &ConversationId) -> PathBuf {
    Path::new(HOLDS)
        .join(organization_directory(organization_id))
        .join(conversation_id.to_string())
}

/// The record's name says which bytes it protects, so bytes stay protected
/// even by a record that cannot be read; the tag tells apart the media types
/// those bytes are kept as. A media type can hold `/`, so it is never a name.
fn hold_name(stored: &Attachment) -> String {
    format!(
        "{}-{}{HOLD_SUFFIX}",
        stored.digest().to_hex(),
        &hex_of(stored.media_type().as_str().as_bytes())[..16]
    )
}

fn hold_path(
    organization_id: &OrganizationId,
    conversation_id: &ConversationId,
    stored: &Attachment,
) -> PathBuf {
    hold_directory(organization_id, conversation_id).join(hold_name(stored))
}

fn path_of(hold: &Hold) -> PathBuf {
    hold_path(
        hold.organization_id(),
        hold.conversation_id(),
        hold.stored(),
    )
}

/// The digest a record's name protects.
fn protected_digest(name: &str) -> Option<&str> {
    let digest = name.strip_suffix(HOLD_SUFFIX)?.get(..DIGEST_HEX)?;
    digest
        .bytes()
        .all(|byte| byte.is_ascii_hexdigit())
        .then_some(digest)
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

fn hold_state(state: RecordState) -> HoldState {
    match state {
        RecordState::Pending => HoldState::Pending,
        RecordState::Kept => HoldState::Held,
    }
}

impl Files {
    fn lock(&self) -> io::Result<MutexGuard<'_, ()>> {
        self.changes
            .lock()
            .map_err(|_| io::Error::other("attachment store lock poisoned"))
    }

    /// The record saved at `relative`, which must describe the owner,
    /// conversation, and stored file its own path says it does.
    fn read_record(&self, relative: &Path) -> io::Result<Option<HoldRecord>> {
        let Some(file) = not_found(open_beneath(&self.root, relative, OpenMode::Read))? else {
            return Ok(None);
        };
        let mut bytes = Vec::new();
        file.take(MAX_HOLD_BYTES + 1).read_to_end(&mut bytes)?;
        if bytes.len() as u64 > MAX_HOLD_BYTES {
            return Err(corrupt("hold record is too large"));
        }
        let record = decode(&bytes).ok_or_else(|| corrupt("hold record is not valid"))?;
        if path_of(&record.hold) != relative {
            return Err(corrupt("hold record describes another hold"));
        }
        Ok(Some(record))
    }

    fn write_record(&self, hold: &Hold, state: RecordState, generation: &str) -> io::Result<()> {
        let directory = hold_directory(hold.organization_id(), hold.conversation_id());
        create_directory_beneath(&self.root, &directory)?;
        let mut file = PrivateTempFile::new_beneath(&self.root, &directory)?;
        file.as_file_mut()
            .write_all(&encode(hold, state, generation))?;
        file.as_file().sync_all()?;
        file.persist_beneath(&path_of(hold))?;
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

    /// Every stored digest some record still names, pending or kept, read from
    /// names alone so an unreadable record still protects its bytes.
    fn referenced(&self) -> io::Result<HashSet<String>> {
        let mut digests = HashSet::new();
        for organization in directories(&self.root.join(HOLDS))? {
            for conversation in directories(&organization)? {
                for entry in fs::read_dir(conversation)? {
                    if let Some(digest) = entry?.file_name().to_str().and_then(protected_digest) {
                        digests.insert(digest.to_owned());
                    }
                }
            }
        }
        Ok(digests)
    }

    /// Remove bytes that no record in `referenced` names. `Ok(true)` when
    /// bytes were removed, `Ok(false)` when they are still held or were
    /// already gone.
    fn remove_unheld(
        &self,
        digest: Sha256Digest,
        referenced: &HashSet<String>,
    ) -> io::Result<bool> {
        if referenced.contains(&digest.to_hex()) {
            return Ok(false);
        }
        Ok(not_found(self.remove(&blob_path(digest)))?.is_some())
    }

    /// The saved records of one conversation: each one's path, and the record
    /// when it could be read.
    fn conversation_records(
        &self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
    ) -> io::Result<Vec<(PathBuf, io::Result<HoldRecord>)>> {
        let directory = hold_directory(organization_id, conversation_id);
        let absolute = self.root.join(&directory);
        if not_found(verify_directory(&absolute))?.is_none() {
            return Ok(Vec::new());
        }
        let mut records = Vec::new();
        for entry in fs::read_dir(absolute)? {
            let name = entry?.file_name();
            let Some(name) = name.to_str().filter(|name| name.ends_with(HOLD_SUFFIX)) else {
                continue;
            };
            let relative = directory.join(name);
            let record = self
                .read_record(&relative)
                .and_then(|record| record.ok_or_else(|| corrupt("hold record vanished")));
            records.push((relative, record));
        }
        Ok(records)
    }

    fn has_bytes(&self, stored: &Attachment) -> io::Result<bool> {
        Ok(self.blob_size(stored.digest())? == Some(stored.size()))
    }

    fn publish(&self, file: PrivateTempFile, stored: &Attachment) -> io::Result<()> {
        let blobs = self.root.join(BLOBS);
        match file.publish(&blobs.join(stored.digest().to_hex())) {
            Ok(()) => sync_directory(&blobs),
            // The same bytes are already stored. They are trusted by name, and
            // refused if they cannot be the same bytes.
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => {
                if self.has_bytes(stored)? {
                    Ok(())
                } else {
                    Err(corrupt("stored bytes disagree with their digest's size"))
                }
            }
            Err(error) => Err(error),
        }
    }

    /// Publish and write a pending record, or find the file already kept.
    fn keep(&self, file: PrivateTempFile, hold: &Hold) -> io::Result<Kept> {
        let _changes = self.lock()?;
        let existing = self.read_record(&path_of(hold))?;
        self.publish(file, hold.stored())?;
        if let Some(HoldRecord {
            hold: kept,
            state: RecordState::Kept,
            ..
        }) = existing
        {
            // A usable hold is never replaced. If its bytes had gone missing
            // they are back now, which is all that changed.
            return Ok(Kept::Existing(kept));
        }
        // Nothing, or a pending record: another upload's, whose claim stops
        // matching here, or one a crash left behind.
        let generation = Uuid::new_v4().to_string();
        if let Err(error) = self.write_record(hold, RecordState::Pending, &generation) {
            // A record that did get written stays pending, which nothing can
            // use. Take back what can be: this write's record, and bytes
            // nothing came to hold.
            self.discard_generation(hold, &generation);
            return Err(error);
        }
        Ok(Kept::Pending(HoldClaim::new(&generation)))
    }

    /// Best effort, for a write that failed partway. Errors are logged by the
    /// caller's own failure.
    fn discard_generation(&self, hold: &Hold, generation: &str) {
        let mine = matches!(
            self.read_record(&path_of(hold)),
            Ok(Some(record)) if record.generation == generation
        );
        if mine {
            let _ = self.remove(&path_of(hold));
        }
        if let Ok(referenced) = self.referenced() {
            let _ = self.remove_unheld(hold.stored().digest(), &referenced);
        }
    }

    fn confirm(&self, hold: &Hold, claim: &HoldClaim) -> io::Result<Confirmation> {
        let _changes = self.lock()?;
        match self.read_record(&path_of(hold))? {
            None => Ok(Confirmation::Gone),
            Some(record) if record.state == RecordState::Kept => {
                Ok(if record.generation == claim.as_str() {
                    Confirmation::Confirmed
                } else {
                    Confirmation::AlreadyKept
                })
            }
            // Pending under this claim, or under another upload's of the same
            // file. The caller has committed evidence, so its hold is the one
            // kept; the other claim finds the file already kept.
            Some(_) => {
                self.write_record(hold, RecordState::Kept, claim.as_str())?;
                Ok(Confirmation::Confirmed)
            }
        }
    }

    fn discard(&self, hold: &Hold, claim: &HoldClaim) -> io::Result<Discard> {
        let _changes = self.lock()?;
        match self.read_record(&path_of(hold))? {
            Some(record) if record.generation == claim.as_str() => {
                self.remove(&path_of(hold))?;
                self.remove_unheld(hold.stored().digest(), &self.referenced()?)?;
                Ok(Discard::Discarded)
            }
            _ => Ok(Discard::NotMine),
        }
    }

    fn release(
        &self,
        organization_id: &OrganizationId,
        conversation_id: &ConversationId,
    ) -> io::Result<ReleaseReport> {
        let _changes = self.lock()?;
        let mut report = ReleaseReport::default();
        for (path, record) in self.conversation_records(organization_id, conversation_id)? {
            // A record that cannot be read is not erased: its bytes stay
            // protected by its name, and the failure is reported.
            match record.and_then(|record| self.remove(&path).map(|()| record)) {
                Ok(record) => report.released.push(ReleasedHold {
                    hold: record.hold,
                    was: hold_state(record.state),
                }),
                Err(error) => {
                    tracing::error!(path = %path.display(), %error, "hold was not released");
                    report.failures += 1;
                }
            }
        }
        // Bytes go only after every hold of this conversation is gone, and
        // what is still held anywhere is read once for all of them.
        let mut considered = HashSet::new();
        match self.referenced() {
            Ok(referenced) => {
                for released in &report.released {
                    let digest = released.hold.stored().digest();
                    if !considered.insert(digest) {
                        continue;
                    }
                    match self.remove_unheld(digest, &referenced) {
                        Ok(true) => report.removed.push(released.hold.clone()),
                        Ok(false) => {}
                        Err(error) => {
                            tracing::error!(%digest, %error, "unheld attachment bytes were not removed");
                            report.failures += 1;
                        }
                    }
                }
            }
            // Without the list of holds nothing proves any bytes are unheld.
            Err(error) => {
                tracing::error!(%error, "attachment holds could not be listed");
                report.failures += 1;
            }
        }
        // Left in place when something remains in it.
        let _ = fs::remove_dir(
            self.root
                .join(hold_directory(organization_id, conversation_id)),
        );
        Ok(report)
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
            match files.read_record(&hold_path(&organization_id, &conversation_id, &stored))? {
                Some(record)
                    if record.state == RecordState::Kept
                        && record
                            .hold
                            .keeps(&organization_id, &conversation_id, &stored) =>
                {
                    files.has_bytes(&stored)
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
            for (path, record) in files.conversation_records(&organization_id, &conversation_id)? {
                match record {
                    Ok(HoldRecord {
                        hold,
                        state: RecordState::Kept,
                        ..
                    }) if hold.came_from(&organization_id, &conversation_id, &uploaded) => {
                        if files.has_bytes(hold.stored())? {
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

    fn confirm<'a>(
        &'a self,
        hold: &'a Hold,
        claim: &'a HoldClaim,
    ) -> PortFuture<'a, Confirmation, StoreUnavailable> {
        let (hold, claim) = (hold.clone(), claim.clone());
        Box::pin(self.blocking(move |files| files.confirm(&hold, &claim)))
    }

    fn discard<'a>(
        &'a self,
        hold: &'a Hold,
        claim: &'a HoldClaim,
    ) -> PortFuture<'a, Discard, StoreUnavailable> {
        let (hold, claim) = (hold.clone(), claim.clone());
        Box::pin(self.blocking(move |files| files.discard(&hold, &claim)))
    }

    fn release<'a>(
        &'a self,
        organization_id: &'a OrganizationId,
        conversation_id: &'a ConversationId,
    ) -> PortFuture<'a, ReleaseReport, StoreUnavailable> {
        let (organization_id, conversation_id) = (organization_id.clone(), conversation_id.clone());
        Box::pin(self.blocking(move |files| files.release(&organization_id, &conversation_id)))
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

    fn keep(mut self: Box<Self>, hold: Hold) -> PortFuture<'static, Kept, StoreUnavailable> {
        Box::pin(async move {
            // These bytes are published under the hold's stored digest, so they
            // must be the bytes that digest and size describe.
            let received = self.finished.ok_or(StoreUnavailable)?;
            if received.digest != hold.stored().digest() || received.size != hold.stored().size() {
                return Err(StoreUnavailable);
            }
            let (file, _) = self.writing.take().ok_or(StoreUnavailable)?;
            let files = self.files.clone();
            tokio::task::spawn_blocking(move || files.keep(file, &hold))
                .await
                .map_err(|_| StoreUnavailable)?
                .map_err(|error| {
                    tracing::error!(%error, "attachment could not be kept");
                    StoreUnavailable
                })
        })
    }
}

#[cfg(test)]
#[path = "../../../tests/attachments/store.rs"]
mod tests;
