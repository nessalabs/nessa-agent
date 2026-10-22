//! Windows filesystem adapter for the shared restore lifecycle.
//!
//! Native mechanics and ACL rules belong to nessa-local-storage. This adapter
//! maps their outcomes and retains existing-session permissions for restoration.
//! The shared lifecycle owns final validation, publication and audit ordering.

use super::{duplicate, EntryKind, RestoreFault};
use nessa_local_storage::{self as storage, DirectoryAcl, OpenMode};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io,
};

pub(super) fn create_directory(parent: &File, name: &OsStr, root: bool) -> io::Result<File> {
    if root {
        storage::create_protected_directory_at(parent, name)
    } else {
        storage::create_protected_subdirectory_at(parent, name)
    }
}

pub(super) fn open_directory(parent: &File, name: &OsStr) -> Result<File, RestoreFault> {
    storage::open_relative_directory(parent, name).map_err(|_| RestoreFault::Unreadable)
}

pub(super) fn open_file(parent: &File, name: &OsStr, mode: OpenMode) -> Result<File, RestoreFault> {
    storage::open_relative_file(parent, name, mode).map_err(|_| RestoreFault::Unreadable)
}

pub(super) fn entry_kind(parent: &File, name: &OsStr) -> Result<EntryKind, RestoreFault> {
    use storage::RelativeEntryKind;
    storage::relative_entry_kind(parent, name)
        .map(|entry| match entry {
            None => EntryKind::Missing,
            Some(RelativeEntryKind::File) => EntryKind::File,
            Some(RelativeEntryKind::Directory) => EntryKind::Directory,
            Some(RelativeEntryKind::Reparse) => EntryKind::Symlink,
            Some(RelativeEntryKind::Other) => EntryKind::Other,
        })
        .map_err(|_| RestoreFault::Unreadable)
}

pub(super) fn names(directory: &File) -> Result<Vec<OsString>, RestoreFault> {
    storage::relative_names(directory).map_err(|_| RestoreFault::Unreadable)
}

pub(super) fn protected_file(
    parent: &File,
    name: &OsStr,
    occupied: RestoreFault,
) -> Result<File, RestoreFault> {
    storage::create_protected_file_at(parent, name).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            occupied
        } else {
            RestoreFault::Unreadable
        }
    })
}

pub(super) fn rename_owned(
    source: &File,
    parent: &File,
    name: &OsStr,
    occupied: RestoreFault,
) -> Result<(), RestoreFault> {
    storage::rename_handle_at(source, parent, name).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            occupied
        } else {
            RestoreFault::Rename
        }
    })
}

#[cfg(test)]
pub(super) fn rename_entry(
    from_parent: &File,
    from: &OsStr,
    to_parent: &File,
    to: &OsStr,
    occupied: RestoreFault,
) -> Result<(), RestoreFault> {
    storage::rename_relative_entry_at(from_parent, from, to_parent, to).map_err(|error| {
        if error.kind() == io::ErrorKind::AlreadyExists {
            occupied
        } else {
            RestoreFault::Rename
        }
    })
}

pub(super) fn seal_copy_directory(directory: &File) -> Result<File, RestoreFault> {
    let retained = duplicate(directory)?;
    storage::seal_directory_writes(&retained).map_err(|_| RestoreFault::Unreadable)?;
    Ok(retained)
}

pub(super) fn release_copy_directory(directory: &File) -> Result<(), RestoreFault> {
    storage::release_directory_writes(directory).map_err(|_| RestoreFault::Retained)
}

enum SessionPermissions {
    Existing(DirectoryAcl),
    NewPrivate,
}

/// Session directory retained without delete sharing before any leaf is created.
/// A first fetch creates it with that handle policy from birth; an existing
/// directory is snapshotted before temporary append permissions are installed.
pub(super) struct SessionDirectory {
    directory: File,
    permissions: SessionPermissions,
    armed: bool,
}

impl SessionDirectory {
    pub(super) fn begin(parent: &File, name: &OsStr) -> Result<Self, RestoreFault> {
        let directory = match storage::retain_directory_at(parent, name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                // Do not create, close and reopen: retain the initial root
                // authority. A concurrent occupant is refused, not adopted.
                let directory = storage::create_protected_directory_at(parent, name)
                    .map_err(|_| RestoreFault::Retained)?;
                return Ok(Self {
                    directory,
                    permissions: SessionPermissions::NewPrivate,
                    armed: true,
                });
            }
            Err(_) => return Err(RestoreFault::Retained),
        };
        let original = DirectoryAcl::capture(&directory).map_err(|_| RestoreFault::Unreadable)?;
        let mut guard = Self {
            directory,
            permissions: SessionPermissions::Existing(original),
            armed: true,
        };
        if storage::append_directory_writes(&guard.directory).is_err() {
            guard.release()?;
            return Err(RestoreFault::Unreadable);
        }
        Ok(guard)
    }

    pub(super) fn directory(&self) -> &File {
        &self.directory
    }

    /// Complete setup before transferring this guard into a fetch attempt.
    /// A failed setup must observe restoration failure rather than rely on Drop.
    pub(super) fn prepare<T>(
        &mut self,
        setup: impl FnOnce(&File) -> Result<T, RestoreFault>,
    ) -> Result<T, RestoreFault> {
        let prepared = setup(self.directory());
        finish_setup(prepared, || self.release())
    }

    pub(super) fn seal(&self) -> Result<(), RestoreFault> {
        storage::seal_directory_writes(&self.directory).map_err(|_| RestoreFault::Unreadable)
    }

    pub(super) fn release(&mut self) -> Result<(), RestoreFault> {
        if self.armed {
            match &self.permissions {
                SessionPermissions::Existing(original) => original.restore(&self.directory),
                SessionPermissions::NewPrivate => {
                    storage::release_directory_writes(&self.directory)
                }
            }
            .map_err(|_| RestoreFault::Retained)?;
            self.armed = false;
        }
        Ok(())
    }
}

impl Drop for SessionDirectory {
    fn drop(&mut self) {
        let _ = self.release();
    }
}

fn finish_setup<T>(
    prepared: Result<T, RestoreFault>,
    restore: impl FnOnce() -> Result<(), RestoreFault>,
) -> Result<T, RestoreFault> {
    match prepared {
        Ok(value) => Ok(value),
        Err(error) => match restore() {
            Ok(()) => Err(error),
            Err(_) => Err(RestoreFault::Retained),
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn existing_sessions(parent: &File) -> DirectoryAcl {
        let directory =
            storage::create_protected_directory_at(parent, OsStr::new("sessions")).unwrap();
        storage::release_directory_writes(&directory).unwrap();
        DirectoryAcl::capture(&directory).unwrap()
    }

    #[test]
    fn failed_setup_restores_the_exact_existing_acl_and_preserves_its_error() {
        let temp = tempfile::tempdir().unwrap();
        let parent = super::super::open_ancestor(temp.path()).unwrap();
        let original = existing_sessions(&parent);
        let mut guard = SessionDirectory::begin(&parent, OsStr::new("sessions")).unwrap();
        assert_ne!(DirectoryAcl::capture(guard.directory()).unwrap(), original);
        let result: Result<(), RestoreFault> = guard.prepare(|_| Err(RestoreFault::Unreadable));
        assert_eq!(result, Err(RestoreFault::Unreadable));
        assert_eq!(DirectoryAcl::capture(guard.directory()).unwrap(), original);
    }

    #[test]
    fn failed_setup_and_failed_acl_restoration_report_retained_permissions() {
        let temp = tempfile::tempdir().unwrap();
        let parent = super::super::open_ancestor(temp.path()).unwrap();
        let original = existing_sessions(&parent);
        let mut guard = SessionDirectory::begin(&parent, OsStr::new("sessions")).unwrap();
        let temporary = DirectoryAcl::capture(guard.directory()).unwrap();
        assert_ne!(temporary, original);
        // This genuine directory handle has no WRITE_DAC authority. Native
        // restoration must fail; the production setup decision keeps that
        // retained metadata distinct from the original setup error.
        let readonly = storage::open_relative_directory(&parent, OsStr::new("sessions")).unwrap();
        let mut restoration_attempted = false;
        let result: Result<(), RestoreFault> = finish_setup(Err(RestoreFault::Unreadable), || {
            restoration_attempted = true;
            original
                .restore(&readonly)
                .map_err(|_| RestoreFault::Retained)
        });
        assert!(restoration_attempted);
        assert_eq!(result, Err(RestoreFault::Retained));
        assert_eq!(DirectoryAcl::capture(guard.directory()).unwrap(), temporary);
        guard.release().unwrap();
        assert_eq!(DirectoryAcl::capture(guard.directory()).unwrap(), original);
    }
}
