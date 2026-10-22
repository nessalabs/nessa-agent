//! Identity tokens for a sealed Windows tree while descendant handles close.
//!
//! The common lifecycle owns seal/validate/rename/audit ordering. This module
//! walks saved component identities through the retained root; a name alone
//! never grants ACL or truncation authority.

use super::{
    duplicate, relative_leaf, relative_path_is_safe, OwnedDirectory, OwnedStageFile, RestoreFault,
    Unpublished,
};
use nessa_local_storage::{self as storage, NativeObjectId};
use std::{
    fs::File,
    path::{Component, Path},
};

struct DirectoryToken {
    relative: String,
    identity: NativeObjectId,
}
struct FileToken {
    relative: String,
    identity: NativeObjectId,
    fingerprint: Option<super::Fingerprint>,
}

pub(super) struct ClosedTree {
    root: NativeObjectId,
    directories: Vec<DirectoryToken>,
    files: Vec<FileToken>,
}

impl ClosedTree {
    pub(super) fn capture_stage(
        root: &File,
        files: &[OwnedStageFile],
        directories: &[OwnedDirectory],
    ) -> Result<Self, RestoreFault> {
        let mut tree = Self::capture(
            root,
            files
                .iter()
                .map(|entry| (entry.relative.as_str(), &entry.file)),
            directories,
        )?;
        for (token, file) in tree.files.iter_mut().zip(files) {
            token.fingerprint = Some(file.fingerprint);
        }
        Ok(tree)
    }

    pub(super) fn capture_publication(
        root: &File,
        files: &[Unpublished],
        journal: &Unpublished,
        directories: &[OwnedDirectory],
    ) -> Result<Self, RestoreFault> {
        Self::capture(
            root,
            files
                .iter()
                .chain(std::iter::once(journal))
                .map(|entry| {
                    Ok((
                        entry.relative.as_str(),
                        entry.file.as_ref().ok_or(RestoreFault::Retained)?,
                    ))
                })
                .collect::<Result<Vec<_>, RestoreFault>>()?,
            directories,
        )
    }

    fn capture<'a>(
        root: &File,
        files: impl IntoIterator<Item = (&'a str, &'a File)>,
        directories: &[OwnedDirectory],
    ) -> Result<Self, RestoreFault> {
        Ok(Self {
            root: NativeObjectId::capture(root).map_err(|_| RestoreFault::Retained)?,
            directories: directories
                .iter()
                .map(|entry| {
                    Ok(DirectoryToken {
                        relative: entry.relative.clone(),
                        identity: NativeObjectId::capture(&entry.directory)
                            .map_err(|_| RestoreFault::Retained)?,
                    })
                })
                .collect::<Result<_, RestoreFault>>()?,
            files: files
                .into_iter()
                .map(|(relative, file)| {
                    Ok(FileToken {
                        relative: relative.to_owned(),
                        identity: NativeObjectId::capture(file)
                            .map_err(|_| RestoreFault::Retained)?,
                        fingerprint: None,
                    })
                })
                .collect::<Result<_, RestoreFault>>()?,
        })
    }

    fn require_root(&self, root: &File) -> Result<(), RestoreFault> {
        if NativeObjectId::capture(root).map_err(|_| RestoreFault::Retained)? != self.root {
            return Err(RestoreFault::Retained);
        }
        Ok(())
    }

    fn directory(&self, root: &File, relative: &str) -> Result<File, RestoreFault> {
        self.require_root(root)?;
        let mut directory = duplicate(root)?;
        let mut prefix = String::new();
        for component in Path::new(relative).components() {
            let Component::Normal(name) = component else {
                return Err(RestoreFault::Retained);
            };
            if !prefix.is_empty() {
                prefix.push('/');
            }
            prefix.push_str(name.to_str().ok_or(RestoreFault::Retained)?);
            let token = self
                .directories
                .iter()
                .find(|entry| entry.relative == prefix)
                .ok_or(RestoreFault::Retained)?;
            directory = storage::open_verified_directory_at(&directory, name, token.identity)
                .map_err(|_| RestoreFault::Retained)?;
        }
        Ok(directory)
    }

    fn file(&self, root: &File, token: &FileToken) -> Result<File, RestoreFault> {
        if !relative_path_is_safe(&token.relative) {
            return Err(RestoreFault::Retained);
        }
        let parent = Path::new(&token.relative)
            .parent()
            .ok_or(RestoreFault::Retained)?;
        let directory = self.directory(root, parent.to_str().ok_or(RestoreFault::Retained)?)?;
        storage::open_verified_file_at(&directory, &relative_leaf(&token.relative)?, token.identity)
            .map_err(|_| RestoreFault::Retained)
    }

    /// Reacquire every staged exact inode before making the namespace writable.
    /// Retire successfully acquired exact files if a later file refuses, and
    /// report the still-sealed remainder as retained.
    pub(super) fn reacquire_stage(&self, root: &File) -> Result<Vec<OwnedStageFile>, RestoreFault> {
        let mut acquired = Vec::new();
        let result: Result<(), RestoreFault> = (|| {
            for token in &self.files {
                let metadata = self.file(root, token)?;
                let file = storage::reacquire_exclusive_file(&metadata, token.identity)
                    .map_err(|_| RestoreFault::Retained)?;
                acquired.push(OwnedStageFile {
                    relative: token.relative.clone(),
                    file,
                    fingerprint: token.fingerprint.ok_or(RestoreFault::Retained)?,
                });
            }
            Ok(())
        })();
        if result.is_err() {
            for entry in &acquired {
                let _ = super::empty_owned_file(&entry.file);
            }
            return Err(RestoreFault::Retained);
        }
        Ok(acquired)
    }

    pub(super) fn release(&self, root: &File) -> bool {
        let mut complete = true;
        for token in &self.files {
            if self
                .file(root, token)
                .and_then(|file| {
                    storage::release_file_writes(&file).map_err(|_| RestoreFault::Retained)
                })
                .is_err()
            {
                complete = false;
            }
        }
        // Walk parents while they remain sealed; release children before root.
        for token in self.directories.iter().rev() {
            if self
                .directory(root, &token.relative)
                .and_then(|file| {
                    storage::release_directory_writes(&file).map_err(|_| RestoreFault::Retained)
                })
                .is_err()
            {
                complete = false;
            }
        }
        if self
            .require_root(root)
            .and_then(|()| {
                storage::release_directory_writes(root).map_err(|_| RestoreFault::Retained)
            })
            .is_err()
        {
            complete = false;
        }
        complete
    }

    pub(super) fn retire(&self, root: &File) -> Result<(), RestoreFault> {
        for token in &self.files {
            let _ = self
                .file(root, token)
                .and_then(|metadata| {
                    storage::reacquire_exclusive_file(&metadata, token.identity)
                        .map_err(|_| RestoreFault::Retained)
                })
                .and_then(|file| super::empty_owned_file(&file));
        }
        // Names and unconfirmed bytes remain restricted. Never return absent.
        Err(RestoreFault::Retained)
    }
}
