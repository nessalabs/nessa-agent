//! Retained private-directory authority and origin-bound publication.
//!
//! A [`PrivateDirectory`] keeps the directory object acquired beneath a trusted
//! root. Operations never reacquire authority from a replacement absolute path.
//! Binding checks are acknowledgement checkpoints: callers that can mutate the
//! same parent must still hold their own stable lock across each operation.

use crate::{platform, temporary_name, unsafe_file, OpenMode};
use std::{
    error::Error,
    ffi::{OsStr, OsString},
    fmt,
    fs::File,
    io,
    path::Path,
};

/// A retained handle to one private directory beneath a caller-trusted root.
pub struct PrivateDirectory {
    inner: platform::RetainedDirectory,
}

impl PrivateDirectory {
    /// Retain an absolute private directory through trusted locator ancestry.
    ///
    /// Locator ancestors before `private_root` may be owned by root or the
    /// current user and may have ordinary read/execute permissions, but may not
    /// be group/other writable. `private_root`, the final directory, and every
    /// component between them must remain current-user-owned and mode `0700`.
    #[cfg(unix)]
    pub fn open_path(private_root: &Path, directory: &Path) -> io::Result<Self> {
        Ok(Self {
            inner: platform::RetainedDirectory::open_path(private_root, directory)?,
        })
    }

    /// Acquire `directory` once beneath `root` and retain that exact object.
    ///
    /// `root` must be absolute. `directory` must be non-empty, relative, and
    /// contain only normal path components. Every component is private,
    /// non-reparse storage owned by the current OS user.
    pub fn open_beneath(root: &Path, directory: &Path) -> io::Result<Self> {
        Ok(Self {
            inner: platform::RetainedDirectory::open_beneath(root, directory)?,
        })
    }

    /// Start an independent, non-atomic enumeration of this retained directory.
    pub fn entries(&self) -> io::Result<PrivateDirectoryEntries> {
        Ok(PrivateDirectoryEntries {
            inner: self.inner.entries()?,
        })
    }

    /// Open one native single-component name through the retained directory.
    pub fn open_file(&self, name: &OsStr, mode: OpenMode) -> io::Result<File> {
        validate_name(name)?;
        self.inner.open_file(name, mode)
    }

    /// Report whether `name` currently names the exact open file.
    ///
    /// The name is inspected without following a symbolic link or reparse
    /// point. `false` means absence or a different object; malformed names and
    /// operating-system failures are errors.
    pub fn named_file_is(&self, name: &OsStr, file: &File) -> io::Result<bool> {
        validate_name(name)?;
        self.verify_binding()?;
        let matches = self.inner.named_file_is(name, file)?;
        self.verify_binding()?;
        Ok(matches)
    }

    /// Verify that every retained directory still has its original name in its
    /// original parent, including the trusted root path.
    pub fn verify_binding(&self) -> io::Result<()> {
        self.inner.verify_binding()
    }

    /// Verify the binding, sync this retained directory where the OS supports
    /// directory fsync, and verify the binding again.
    ///
    /// Windows performs the two binding validations but has no directory-fsync
    /// equivalent. This is an acknowledgement checkpoint, not a continuous
    /// attachment guarantee and not a claim about arbitrary power loss.
    pub fn sync(&self) -> io::Result<()> {
        self.verify_binding()?;
        self.inner.sync()?;
        self.verify_binding()
    }

    /// Reserve a private temporary file in this exact directory.
    pub fn reserve_temp(&self) -> io::Result<PrivateDirectoryTempFile<'_>> {
        for _ in 0..10 {
            let name = temporary_name()?;
            match self.inner.reserve_temp(&name) {
                Ok(file) => {
                    return Ok(PrivateDirectoryTempFile {
                        directory: self,
                        file: Some(file),
                        name,
                        cleanup_required: true,
                    });
                }
                Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
                Err(error) => return Err(error),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "could not reserve private temporary file",
        ))
    }
}

/// The kind observed for a directory entry without following that entry.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivateFileType {
    /// A regular file.
    RegularFile,
    /// A directory.
    Directory,
    /// A symbolic link or Windows reparse point.
    Symlink,
    /// Any other native file kind.
    Other,
}

/// One native directory name and the type observed during enumeration.
#[derive(Debug)]
pub struct PrivateDirectoryEntry {
    name: OsString,
    file_type: PrivateFileType,
}

impl PrivateDirectoryEntry {
    pub(crate) fn new(name: OsString, file_type: PrivateFileType) -> Self {
        Self { name, file_type }
    }

    /// Return the native, unmodified entry name.
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Return the entry type observed by this non-atomic enumeration.
    pub fn file_type(&self) -> PrivateFileType {
        self.file_type
    }
}

/// An independent cursor over a retained private directory.
pub struct PrivateDirectoryEntries {
    inner: platform::RetainedDirectoryEntries,
}

impl Iterator for PrivateDirectoryEntries {
    type Item = io::Result<PrivateDirectoryEntry>;

    fn next(&mut self) -> Option<Self::Item> {
        self.inner.next()
    }
}

/// Opaque identity of an opened native file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct PrivateFileIdentity {
    volume: u64,
    file: [u8; 16],
}

impl PrivateFileIdentity {
    #[cfg(any(unix, test))]
    pub(crate) fn from_u64s(volume: u64, file: u64) -> Self {
        let mut bytes = [0; 16];
        bytes[..8].copy_from_slice(&file.to_ne_bytes());
        Self {
            volume,
            file: bytes,
        }
    }

    #[cfg(windows)]
    pub(crate) fn from_windows(volume: u64, file: [u8; 16]) -> Self {
        Self { volume, file }
    }
}

/// A file whose destination rename occurred, with the still-open file handle.
#[derive(Debug)]
pub struct PublishedPrivateFile {
    name: OsString,
    identity: PrivateFileIdentity,
    file: File,
}

impl PublishedPrivateFile {
    /// Return the exact destination name supplied to publication.
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Return the identity read from the open file handle.
    pub fn identity(&self) -> PrivateFileIdentity {
        self.identity
    }

    /// Return the file handle that crossed the rename.
    pub fn as_file(&self) -> &File {
        &self.file
    }
}

/// The exact publication checkpoint that failed.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum PrivatePublicationStage {
    /// The requested destination was not a safe native single-component name.
    ValidateDestination,
    /// The retained directory was no longer bound to its original names.
    VerifyOriginBinding,
    /// The reservation name no longer referred to its open file handle.
    ValidateReservation,
    /// Flushing the reservation before rename failed.
    FlushBeforeRename,
    /// The exclusive destination rename failed, so no publication is known.
    Rename,
    /// Flushing the already-renamed file handle failed.
    FlushAfterRename,
    /// The destination name did not resolve to the renamed open file.
    ValidatePublishedDestination,
    /// A binding acknowledgement after rename failed.
    VerifyPublishedBinding,
    /// Syncing the retained directory failed after rename.
    SyncDirectory,
}

/// A publication failure with the rename fact and cleanup failure kept separate.
#[derive(Debug)]
pub struct PrivatePublicationFailure {
    stage: PrivatePublicationStage,
    source: io::Error,
    published: Option<PublishedPrivateFile>,
    cleanup: Option<io::Error>,
}

impl PrivatePublicationFailure {
    /// Return the checkpoint whose primary operation failed.
    pub fn stage(&self) -> PrivatePublicationStage {
        self.stage
    }

    /// Return the published file when the rename already occurred.
    pub fn published(&self) -> Option<&PublishedPrivateFile> {
        self.published.as_ref()
    }

    /// Return the primary failure.
    pub fn source_error(&self) -> &io::Error {
        &self.source
    }

    /// Return an independent origin-reservation cleanup failure.
    pub fn cleanup_error(&self) -> Option<&io::Error> {
        self.cleanup.as_ref()
    }

    /// Consume the failure without losing published-file ownership.
    pub fn into_parts(
        self,
    ) -> (
        PrivatePublicationStage,
        io::Error,
        Option<PublishedPrivateFile>,
        Option<io::Error>,
    ) {
        (self.stage, self.source, self.published, self.cleanup)
    }
}

impl fmt::Display for PrivatePublicationFailure {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(
            output,
            "private publication failed at {:?}: {}",
            self.stage, self.source
        )?;
        if let Some(cleanup) = &self.cleanup {
            write!(output, "; reservation cleanup also failed: {cleanup}")?;
        }
        Ok(())
    }
}

impl Error for PrivatePublicationFailure {
    fn source(&self) -> Option<&(dyn Error + 'static)> {
        Some(&self.source)
    }
}

/// An origin-bound private temporary file.
pub struct PrivateDirectoryTempFile<'directory> {
    directory: &'directory PrivateDirectory,
    file: Option<File>,
    name: OsString,
    cleanup_required: bool,
}

impl PrivateDirectoryTempFile<'_> {
    /// Return the reservation's native name.
    pub fn name(&self) -> &OsStr {
        &self.name
    }

    /// Borrow the reserved file.
    pub fn as_file(&self) -> &File {
        self.file.as_ref().expect("temporary file is open")
    }

    /// Mutably borrow the reserved file.
    pub fn as_file_mut(&mut self) -> &mut File {
        self.file.as_mut().expect("temporary file is open")
    }

    /// Remove this reservation through its originating directory authority.
    pub fn discard(mut self) -> io::Result<()> {
        let result = self.remove_reserved();
        if result.is_ok() {
            self.cleanup_required = false;
        }
        result
    }

    /// Publish beneath the originating directory without replacing a name.
    ///
    /// Rename success immediately disarms reservation cleanup. Any later error
    /// retains a [`PublishedPrivateFile`] so callers can reconcile the external
    /// effect without blindly reusing a sequence or filename.
    pub fn publish_new(
        mut self,
        destination: &OsStr,
    ) -> Result<PublishedPrivateFile, PrivatePublicationFailure> {
        if let Err(error) = validate_name(destination) {
            return Err(self.fail_before(PrivatePublicationStage::ValidateDestination, error));
        }
        if let Err(error) = self.directory.verify_binding() {
            return Err(self.fail_before(PrivatePublicationStage::VerifyOriginBinding, error));
        }
        let file = self.file.as_ref().expect("temporary file is open");
        match self.directory.named_file_is(&self.name, file) {
            Ok(true) => {}
            Ok(false) => {
                return Err(
                    self.fail_before(PrivatePublicationStage::ValidateReservation, unsafe_file())
                );
            }
            Err(error) => {
                return Err(self.fail_before(PrivatePublicationStage::ValidateReservation, error));
            }
        }
        if let Err(error) = file.sync_all() {
            return Err(self.fail_before(PrivatePublicationStage::FlushBeforeRename, error));
        }
        let identity = match self.directory.inner.file_identity(file) {
            Ok(identity) => identity,
            Err(error) => {
                return Err(self.fail_before(PrivatePublicationStage::ValidateReservation, error));
            }
        };
        if let Err(error) = self
            .directory
            .inner
            .publish_new(&self.name, destination, file)
        {
            return Err(self.fail_before(PrivatePublicationStage::Rename, error));
        }

        // The name now belongs to the destination. Cleanup must never delete a
        // published record, including when an acknowledgement check below fails.
        self.cleanup_required = false;
        let published = PublishedPrivateFile {
            name: destination.to_owned(),
            identity,
            file: self.file.take().expect("temporary file is open"),
        };

        acknowledge_publication(
            published,
            |published| published.file.sync_all(),
            |published| match self
                .directory
                .inner
                .named_file_is(&published.name, &published.file)?
            {
                true => Ok(()),
                false => Err(unsafe_file()),
            },
            || self.directory.verify_binding(),
            || self.directory.inner.sync(),
            || self.directory.verify_binding(),
        )
    }

    fn fail_before(
        &mut self,
        stage: PrivatePublicationStage,
        source: io::Error,
    ) -> PrivatePublicationFailure {
        let cleanup = match self.remove_reserved() {
            Ok(()) => {
                self.cleanup_required = false;
                None
            }
            Err(error) => Some(error),
        };
        PrivatePublicationFailure {
            stage,
            source,
            published: None,
            cleanup,
        }
    }

    fn remove_reserved(&self) -> io::Result<()> {
        self.directory
            .inner
            .remove_reserved(&self.name, self.file.as_ref().ok_or_else(unsafe_file)?)
    }
}

impl Drop for PrivateDirectoryTempFile<'_> {
    fn drop(&mut self) {
        if self.cleanup_required {
            // Drop is best-effort and makes no cleanup-success claim.
            let _ = self.remove_reserved();
        }
    }
}

fn fail_after(
    stage: PrivatePublicationStage,
    source: io::Error,
    published: PublishedPrivateFile,
) -> PrivatePublicationFailure {
    PrivatePublicationFailure {
        stage,
        source,
        published: Some(published),
        cleanup: None,
    }
}

fn acknowledge_publication(
    published: PublishedPrivateFile,
    flush: impl FnOnce(&PublishedPrivateFile) -> io::Result<()>,
    validate_destination: impl FnOnce(&PublishedPrivateFile) -> io::Result<()>,
    verify_binding: impl FnOnce() -> io::Result<()>,
    sync_directory: impl FnOnce() -> io::Result<()>,
    verify_synced_binding: impl FnOnce() -> io::Result<()>,
) -> Result<PublishedPrivateFile, PrivatePublicationFailure> {
    if let Err(error) = flush(&published) {
        return Err(fail_after(
            PrivatePublicationStage::FlushAfterRename,
            error,
            published,
        ));
    }
    if let Err(error) = validate_destination(&published) {
        return Err(fail_after(
            PrivatePublicationStage::ValidatePublishedDestination,
            error,
            published,
        ));
    }
    if let Err(error) = verify_binding() {
        return Err(fail_after(
            PrivatePublicationStage::VerifyPublishedBinding,
            error,
            published,
        ));
    }
    if let Err(error) = sync_directory() {
        return Err(fail_after(
            PrivatePublicationStage::SyncDirectory,
            error,
            published,
        ));
    }
    if let Err(error) = verify_synced_binding() {
        return Err(fail_after(
            PrivatePublicationStage::VerifyPublishedBinding,
            error,
            published,
        ));
    }
    Ok(published)
}

pub(crate) fn validate_name(name: &OsStr) -> io::Result<()> {
    let path = Path::new(name);
    let mut components = path.components();
    if matches!(components.next(), Some(std::path::Component::Normal(_)))
        && components.next().is_none()
    {
        platform::validate_native_name(name)
    } else {
        Err(unsafe_file())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[derive(Clone, Copy, Eq, PartialEq)]
    enum AcknowledgementCheckpoint {
        Flush,
        ValidateDestination,
        VerifyBinding,
        SyncDirectory,
        VerifySyncedBinding,
    }

    fn injected(
        checkpoint: AcknowledgementCheckpoint,
        failing: AcknowledgementCheckpoint,
    ) -> io::Result<()> {
        if checkpoint == failing {
            Err(io::Error::other("injected acknowledgement failure"))
        } else {
            Ok(())
        }
    }

    fn published_fixture() -> PublishedPrivateFile {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("published");
        std::fs::write(&path, b"published bytes").unwrap();
        PublishedPrivateFile {
            name: OsString::from("record"),
            identity: PrivateFileIdentity::from_u64s(1, 2),
            file: File::open(path).unwrap(),
        }
    }

    #[test]
    fn every_post_rename_failure_retains_name_identity_and_open_handle_without_cleanup() {
        for (checkpoint, expected_stage) in [
            (
                AcknowledgementCheckpoint::Flush,
                PrivatePublicationStage::FlushAfterRename,
            ),
            (
                AcknowledgementCheckpoint::ValidateDestination,
                PrivatePublicationStage::ValidatePublishedDestination,
            ),
            (
                AcknowledgementCheckpoint::VerifyBinding,
                PrivatePublicationStage::VerifyPublishedBinding,
            ),
            (
                AcknowledgementCheckpoint::SyncDirectory,
                PrivatePublicationStage::SyncDirectory,
            ),
            (
                AcknowledgementCheckpoint::VerifySyncedBinding,
                PrivatePublicationStage::VerifyPublishedBinding,
            ),
        ] {
            let failure = acknowledge_publication(
                published_fixture(),
                |_| injected(AcknowledgementCheckpoint::Flush, checkpoint),
                |_| injected(AcknowledgementCheckpoint::ValidateDestination, checkpoint),
                || injected(AcknowledgementCheckpoint::VerifyBinding, checkpoint),
                || injected(AcknowledgementCheckpoint::SyncDirectory, checkpoint),
                || injected(AcknowledgementCheckpoint::VerifySyncedBinding, checkpoint),
            )
            .unwrap_err();

            assert_eq!(failure.stage(), expected_stage);
            assert_eq!(
                failure.source_error().to_string(),
                "injected acknowledgement failure"
            );
            assert!(failure.cleanup_error().is_none());
            let published = failure.published().expect("rename fact is retained");
            assert_eq!(published.name(), OsStr::new("record"));
            assert_eq!(published.identity(), PrivateFileIdentity::from_u64s(1, 2));
            assert!(published.as_file().metadata().unwrap().is_file());
        }
    }
}
