//! The one repair this crate allows (ADR 221). An object that only its owner
//! can write, but others can read or search, still holds its owner's contents,
//! so tightening it to owner-only changes who can see it and nothing about who
//! is trusted. Everything else unsafe stays refused.
use super::*;

/// The kind of object the caller expects at the path.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SharedReadKind {
    File,
    Directory,
}

/// A repairable object, held open and not yet changed.
///
/// The caller records what will change, then calls [`tighten`](Self::tighten),
/// which changes this same open object: nothing can be swapped in at the path
/// between the check, the record, and the change.
#[derive(Debug)]
pub struct SharedReadCandidate {
    object: File,
    mode_before: u32,
}

impl SharedReadCandidate {
    /// Permission bits found, before the repair.
    pub fn mode_before(&self) -> u32 {
        self.mode_before
    }

    /// Permission bits the repair leaves: the owner's, and nothing else.
    pub fn mode_after(&self) -> u32 {
        self.mode_before & 0o700
    }

    /// Remove every group and other permission from the held object.
    pub fn tighten(self) -> io::Result<()> {
        let mode = self.mode_after();
        platform::tighten(&self.object, mode)
    }
}

/// Find whether the object at `path` is shared only for reading.
///
/// `Ok(None)` means it is already private. `Ok(Some(_))` means it is owned by
/// the current user, is the expected kind (a regular file with one link, or a
/// directory), and no one else can write to it, but others can read or search
/// it. Every other case, including a symlink at `path`, is the unsafe-file
/// error. Windows repairs nothing and always answers with that error.
pub fn find_shared_read(
    path: &Path,
    kind: SharedReadKind,
) -> io::Result<Option<SharedReadCandidate>> {
    Ok(
        platform::open_shared_read(path, kind)?.map(|(object, mode_before)| SharedReadCandidate {
            object,
            mode_before,
        }),
    )
}
