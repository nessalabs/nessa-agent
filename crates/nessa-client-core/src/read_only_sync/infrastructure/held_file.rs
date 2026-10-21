//! The output a fetched held file is kept as, and the part it is assembled
//! in: `OUT.<key>.part` beside `OUT`, keyed on the whole file it holds
//! (conversation, digest, media type and size), so a part another file left,
//! or the same bytes fetched for another conversation, is never resumed.
//!
//! A part is held with an exclusive lock from when it is opened or created
//! until it is kept or discarded, and only its holder links or removes it.
//! On Unix the holder checks, once it has the lock, that the part's name
//! still names the file it locked, and holds it once more from the name if
//! not; on Windows the part is opened without delete sharing, so its name
//! cannot move while anyone has it open. Either way two fetches never write
//! one part.
//!
//! `OUT` is only ever made by hard-linking a synced, checked part at its
//! name. The link fails when anything is there, a link that leads nowhere
//! included, so what is at `OUT` is never replaced, and there is no gap
//! between a check and the publish for something to appear in. A file
//! system without hard links refuses every fetch at the `link` step. An
//! `OUT` already there is looked at without following links, and opened
//! only when it is a regular file of the named size.
//!
//! The part's name is the output's file name and [`PART_SUFFIX_BYTES`] more,
//! so an output whose file name is longer than 217 bytes is refused before
//! anything is dialled (on Windows counted in WTF-8 bytes, which refuses
//! some names its 255 UTF-16 units would allow).
use crate::read_only_sync::application::held_file::{
    FetchFiles, FileStep, KeepRefusal, OutputFound,
};
use crate::read_only_sync::domain::HeldFile;
use sha2::{Digest, Sha256};
use std::ffi::OsString;
use std::fs::{self, File, Metadata, OpenOptions, TryLockError};
use std::io::{ErrorKind, Read, Result as IoResult, Seek, SeekFrom, Write};
use std::path::{is_separator, Path, PathBuf};

/// How much of a file is read at once while it is hashed.
const READ_BYTES: usize = 64 * 1024;
/// The most bytes one file name may have on the file systems a fetch writes
/// to.
const MAX_NAME_BYTES: usize = 255;
/// How many hex digits of [`part_key`]'s SHA-256 name a part: 128 bits.
const PART_KEY_HEX: usize = 32;
/// What [`part_path`] adds to the output's file name: `.`, the key, `.part`.
const PART_SUFFIX_BYTES: usize = 1 + PART_KEY_HEX + 5;

pub(crate) struct DiskFiles {
    out: PathBuf,
    part: PathBuf,
    /// The part, held, once it was opened or created.
    held: Option<File>,
}

impl DiskFiles {
    /// Sync the held part. A failure here means the output was not published.
    pub(crate) fn sync_held(&mut self) -> Result<(), KeepRefusal> {
        let file = self
            .held
            .as_ref()
            .ok_or(KeepRefusal::File(FileStep::Link))?;
        file.sync_all()
            .map_err(|_| KeepRefusal::File(FileStep::Sync))
    }

    /// Hard-link the part at the output's name. `OutputExists` and `File(Link)`
    /// both mean this call did not publish. On Unix the lock stays held for
    /// the retirement that drops the part's name. On Windows the part's
    /// sharing refuses a link while it is open, so it is closed first; a
    /// fetch that opens it in between finds the same checked bytes, and only
    /// one link can make the output.
    pub(crate) fn link_held(&mut self) -> Result<(), KeepRefusal> {
        #[cfg(not(unix))]
        {
            self.held.take();
        }
        if let Err(error) = fs::hard_link(&self.part, &self.out) {
            // The part stays, for a later run.
            return Err(if error.kind() == ErrorKind::AlreadyExists {
                KeepRefusal::OutputExists
            } else {
                KeepRefusal::File(FileStep::Link)
            });
        }
        Ok(())
    }

    /// Drop the part's name and sync the output's directory. The hard link
    /// already exists, so either failure is [`KeepRefusal::Published`]. A
    /// part name that cannot be removed names the output's bytes; the next
    /// run finds the output is the file and removes it then, never writing
    /// through it.
    pub(crate) fn retire_held(&mut self) -> Result<(), KeepRefusal> {
        let removed = fs::remove_file(&self.part);
        self.held.take();
        removed.map_err(|_| published(FileStep::Remove))?;
        sync_directory(&self.out).map_err(|_| published(FileStep::SyncDirectory))
    }

    /// The files for fetching `file` into `out`. Opens nothing. `Name` when
    /// `out` names no file (it ends in a separator or `..`), or names one
    /// too long to have a part beside it.
    pub(crate) fn new(out: &Path, file: &HeldFile) -> Result<Self, FileStep> {
        let ends_in_separator = out
            .as_os_str()
            .as_encoded_bytes()
            .last()
            .is_some_and(|byte| is_separator(char::from(*byte)));
        let name = out.file_name().ok_or(FileStep::Name)?;
        if ends_in_separator || name.as_encoded_bytes().len() + PART_SUFFIX_BYTES > MAX_NAME_BYTES {
            return Err(FileStep::Name);
        }
        Ok(Self {
            out: out.to_owned(),
            part: part_path(out, file),
            held: None,
        })
    }
    /// Open the part at its name and hold it, `create`ing it when asked; a
    /// part another fetch holds, or one created by another in between, is
    /// `Locked`. `None` when there is no part and none is created. When the
    /// name moved to another file while it was being held, it is held once
    /// more from the name; moved again, it is `Changed`.
    fn hold(&mut self, create: bool) -> Result<Option<&mut File>, FileStep> {
        if self.held.is_none() {
            self.held = match self.hold_once(create) {
                Err(FileStep::Changed) => self.hold_once(create)?,
                held => held?,
            };
        }
        Ok(self.held.as_mut())
    }
    fn hold_once(&self, create: bool) -> Result<Option<File>, FileStep> {
        let mut options = part_options();
        options.create_new(create);
        let file = match options.open(&self.part) {
            Ok(file) => file,
            Err(error) if !create && error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) if create && error.kind() == ErrorKind::AlreadyExists => {
                return Err(FileStep::Locked)
            }
            Err(_) => return Err(FileStep::Open),
        };
        match file.try_lock() {
            Ok(()) => {}
            Err(TryLockError::WouldBlock) => return Err(FileStep::Locked),
            Err(TryLockError::Error(_)) => return Err(FileStep::Open),
        }
        if !still_named(&self.part, &file) {
            return Err(FileStep::Changed);
        }
        Ok(Some(file))
    }
}

/// `OUT` with `.<key>.part` added to its file name, so `a.png` is assembled
/// in `a.png.<key>.part` beside it: never a name another output, or another
/// file, or the same file held by another conversation, would choose.
fn part_path(out: &Path, file: &HeldFile) -> PathBuf {
    let mut name = OsString::from(out.file_name().unwrap_or_default());
    name.push(".");
    name.push(part_key(file));
    name.push(".part");
    out.with_file_name(name)
}

/// The first [`PART_KEY_HEX`] hex digits of a SHA-256 over every field that
/// names the file, each preceded by its length so no two files run together
/// into one key.
fn part_key(file: &HeldFile) -> String {
    let conversation = file.conversation().to_string();
    let size = file.size().to_be_bytes();
    let mut hash = Sha256::new();
    hash.update(b"nessa held-file part v1");
    for field in [
        conversation.as_bytes(),
        file.digest().as_bytes(),
        file.media_type().as_str().as_bytes(),
        &size,
    ] {
        hash.update((field.len() as u64).to_be_bytes());
        hash.update(field);
    }
    let digest: [u8; 32] = hash.finalize().into();
    let mut key = String::with_capacity(PART_KEY_HEX);
    for byte in &digest[..PART_KEY_HEX / 2] {
        key.push_str(&format!("{byte:02x}"));
    }
    key
}

fn part_options() -> OpenOptions {
    let mut options = OpenOptions::new();
    options.read(true).write(true);
    #[cfg(windows)]
    {
        use std::os::windows::fs::OpenOptionsExt;
        // FILE_SHARE_READ | FILE_SHARE_WRITE, without FILE_SHARE_DELETE: no
        // one renames or removes the part while any fetch has it open.
        options.share_mode(0x1 | 0x2);
    }
    options
}

/// Whether `named`, found at a name without following links, is the file
/// `file` has open. On Unix a name can move while a file is open, so the two
/// are compared; on Windows a part's sharing keeps its name, and the output
/// is opened right after it is looked at, so they are taken to be the same.
#[cfg(unix)]
fn is_same(named: &Metadata, file: &File) -> bool {
    use std::os::unix::fs::MetadataExt;
    file.metadata()
        .is_ok_and(|held| named.dev() == held.dev() && named.ino() == held.ino())
}
#[cfg(not(unix))]
fn is_same(_: &Metadata, _: &File) -> bool {
    true
}

/// Whether `path` still names `file`.
fn still_named(path: &Path, file: &File) -> bool {
    fs::symlink_metadata(path).is_ok_and(|named| is_same(&named, file))
}

fn published(step: FileStep) -> KeepRefusal {
    KeepRefusal::Published { step }
}

/// Sync the directory a link or removal happened in, so it survives a
/// crash. Windows has no directory sync; its changes are durable as the file
/// system makes them.
#[cfg(unix)]
fn sync_directory(out: &Path) -> IoResult<()> {
    let parent = out
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or(Path::new("."));
    File::open(parent)?.sync_all()
}
#[cfg(not(unix))]
fn sync_directory(_: &Path) -> IoResult<()> {
    Ok(())
}

fn read_all(file: &mut File, visit: &mut dyn FnMut(&[u8])) -> IoResult<u64> {
    file.seek(SeekFrom::Start(0))?;
    let mut buffer = vec![0; READ_BYTES];
    let mut total = 0_u64;
    loop {
        match file.read(&mut buffer) {
            Ok(0) => return Ok(total),
            Ok(count) => {
                visit(&buffer[..count]);
                total = total.saturating_add(count as u64);
            }
            Err(error) if error.kind() == ErrorKind::Interrupted => {}
            Err(error) => return Err(error),
        }
    }
}

impl FetchFiles for DiskFiles {
    fn read_output(
        &mut self,
        size: u64,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<OutputFound, FileStep> {
        let named = match fs::symlink_metadata(&self.out) {
            Ok(named) => named,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(OutputFound::Absent),
            Err(_) => return Err(FileStep::Open),
        };
        if !named.file_type().is_file() || named.len() != size {
            return Ok(OutputFound::Other);
        }
        let mut output = File::open(&self.out).map_err(|_| FileStep::Open)?;
        // Opening follows a link put there since it was looked at; what was
        // opened is then not what was looked at.
        if !is_same(&named, &output) {
            return Ok(OutputFound::Other);
        }
        read_all(&mut output, visit)
            .map(OutputFound::Read)
            .map_err(|_| FileStep::Read)
    }
    fn remove_unheld_part(&mut self) {
        match self.hold(false) {
            Ok(None) | Err(FileStep::Locked) => return,
            Ok(Some(_)) => {}
            Err(step) => {
                tracing::warn!(?step, part = ?self.part, "a part beside the output stays");
                return;
            }
        }
        if let Err(step) = self.discard() {
            tracing::warn!(?step, part = ?self.part, "a part beside the output stays");
        }
    }
    fn open_part(&mut self) -> Result<u64, FileStep> {
        match self.hold(false)? {
            None => Ok(0),
            Some(file) => file
                .metadata()
                .map(|metadata| metadata.len())
                .map_err(|_| FileStep::Read),
        }
    }
    fn read_part(&mut self, visit: &mut dyn FnMut(&[u8])) -> Result<u64, FileStep> {
        match self.hold(false)? {
            None => Ok(0),
            Some(file) => read_all(file, visit).map_err(|_| FileStep::Read),
        }
    }
    fn restart(&mut self) -> Result<(), FileStep> {
        match self.hold(false).map_err(|_| FileStep::Restart)? {
            None => Ok(()),
            Some(file) => file.set_len(0).map_err(|_| FileStep::Restart),
        }
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), FileStep> {
        if self.held.is_none() {
            self.hold(true)?;
        }
        let file = self.held.as_mut().ok_or(FileStep::Open)?;
        file.seek(SeekFrom::End(0)).map_err(|_| FileStep::Write)?;
        file.write_all(bytes).map_err(|_| FileStep::Write)
    }
    fn keep(&mut self) -> Result<(), KeepRefusal> {
        self.sync_held()?;
        self.link_held()?;
        self.retire_held()
    }
    fn sync_held(&mut self) -> Result<(), KeepRefusal> {
        DiskFiles::sync_held(self)
    }
    fn link_held(&mut self) -> Result<(), KeepRefusal> {
        DiskFiles::link_held(self)
    }
    fn retire_held(&mut self) -> Result<(), KeepRefusal> {
        DiskFiles::retire_held(self)
    }
    fn discard(&mut self) -> Result<(), FileStep> {
        if self.held.is_none() {
            return Ok(());
        }
        // As for `keep`: Windows removes no name a handle is open on.
        #[cfg(not(unix))]
        {
            self.held.take();
        }
        fs::remove_file(&self.part).map_err(|_| FileStep::Remove)?;
        self.held.take();
        Ok(())
    }
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/infrastructure/held_file.rs"]
mod tests;
