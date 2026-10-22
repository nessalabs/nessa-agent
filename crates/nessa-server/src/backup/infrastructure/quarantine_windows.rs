//! Windows filesystem adapter for the shared restore lifecycle.
//!
//! Native mechanics and ACL rules belong to nessa-local-storage. This adapter
//! maps their outcomes and retains session directories without changing their ACLs.
//! The shared lifecycle owns final validation, publication and audit ordering.

use super::{duplicate, EntryKind, RestoreFault};
use nessa_local_storage::{self as storage, OpenMode};
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

/// A fetch leaf stays pinned against write and delete opens through its final audit.
pub(super) fn pinned_file(
    parent: &File,
    name: &OsStr,
    occupied: RestoreFault,
) -> Result<File, RestoreFault> {
    storage::create_pinned_file_at(parent, name).map_err(|error| {
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

/// Session directory retained without delete sharing before any leaf is created.
/// Its ACL is final from acquisition onward. Only this attempt's pinned leaves
/// receive permission changes; inherited permissions on unrelated children stay intact.
pub(super) struct SessionDirectory {
    directory: File,
}

impl SessionDirectory {
    pub(super) fn begin(parent: &File, name: &OsStr) -> Result<Self, RestoreFault> {
        let directory = match storage::open_session_directory_at(parent, name) {
            Ok(directory) => directory,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                storage::create_session_directory_at(parent, name)
                    .map_err(|_| RestoreFault::Retained)?
            }
            Err(_) => return Err(RestoreFault::Retained),
        };
        Ok(Self { directory })
    }

    pub(super) fn directory(&self) -> &File {
        &self.directory
    }

    /// Setup has no parent permission effects to undo on failure.
    pub(super) fn prepare<T>(
        &mut self,
        setup: impl FnOnce(&File) -> Result<T, RestoreFault>,
    ) -> Result<T, RestoreFault> {
        setup(self.directory())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use storage::DirectoryAcl;

    #[test]
    fn failed_session_setup_preserves_the_existing_parent_acl_and_error() {
        let temp = tempfile::tempdir().unwrap();
        let parent = super::super::open_ancestor(temp.path()).unwrap();
        let directory =
            storage::create_session_directory_at(&parent, OsStr::new("sessions")).unwrap();
        let original = DirectoryAcl::capture(&directory).unwrap();
        drop(directory);
        let mut guard = SessionDirectory::begin(&parent, OsStr::new("sessions")).unwrap();
        assert_eq!(DirectoryAcl::capture(guard.directory()).unwrap(), original);
        let result: Result<(), RestoreFault> = guard.prepare(|_| Err(RestoreFault::Unreadable));
        assert_eq!(result, Err(RestoreFault::Unreadable));
        assert_eq!(DirectoryAcl::capture(guard.directory()).unwrap(), original);
        drop(guard);
        let directory =
            storage::open_session_directory_at(&parent, OsStr::new("sessions")).unwrap();
        assert_eq!(DirectoryAcl::capture(&directory).unwrap(), original);
    }
}
