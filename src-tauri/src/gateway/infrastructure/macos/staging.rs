//! Immutable runtime installations. Copy and verify in a private sibling directory,
//! then publish once; old and already-published generations are never rewritten.
//!
//! File durability runs on a bounded pool sized to the machine. The full-tree
//! digest runs at the same time: hashing reads bytes that are already written,
//! while `sync_all` only flushes them. The digest still matches before the
//! exclusive rename, and every file and directory sync finishes before that
//! rename. A failed attempt drops only its own temporary directory, after the
//! pool has joined.
use super::{generation::random_generation, runtime_fingerprint};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::{
    cell::RefCell,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};
use std::{
    collections::{HashMap, HashSet, VecDeque},
    ffi::{CString, OsString},
    fs::{self, DirBuilder, OpenOptions, Permissions},
    io::{Error, ErrorKind, Read},
    os::fd::AsRawFd,
    os::unix::fs::{symlink, DirBuilderExt, MetadataExt, OpenOptionsExt, PermissionsExt},
    path::{Path, PathBuf},
    sync::{mpsc, Arc, Condvar, Mutex},
    thread,
};

#[derive(Debug, Hash, PartialEq, Eq)]
struct ValidatedRuntime {
    path: PathBuf,
    fingerprint: String,
    device: u64,
    inode: u64,
}

/// Successful full-tree validations retained by one injected launchd adapter.
///
/// Published generations are immutable to the host. Reuse still checks that
/// the private root directory is the same filesystem object with the expected
/// owner and mode before trusting the earlier full validation. This deliberately
/// trusts the publication contract: code in this process never mutates a child
/// of a published root. A privileged or out-of-process in-place child mutation
/// that preserves the root inode is outside this cache's threat model.
#[derive(Default)]
pub(super) struct ValidatedRuntimes(Mutex<HashSet<ValidatedRuntime>>);

impl ValidatedRuntimes {
    fn identity(path: &Path, fingerprint: &str) -> Result<Option<ValidatedRuntime>, String> {
        let metadata = match fs::symlink_metadata(path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error.to_string()),
        };
        if !metadata.is_dir()
            || metadata.file_type().is_symlink()
            || metadata.uid() != unsafe { libc::geteuid() }
            || metadata.permissions().mode() & 0o7777 != 0o700
        {
            return Ok(None);
        }
        Ok(Some(ValidatedRuntime {
            path: path.to_owned(),
            fingerprint: fingerprint.to_owned(),
            device: metadata.dev(),
            inode: metadata.ino(),
        }))
    }

    fn contains(&self, path: &Path, fingerprint: &str) -> Result<bool, String> {
        let Some(identity) = Self::identity(path, fingerprint)? else {
            return Ok(false);
        };
        self.0
            .lock()
            .map(|validated| validated.contains(&identity))
            .map_err(|_| "Runtime validation cache is unavailable".into())
    }

    fn remember(&self, path: &Path, fingerprint: &str) -> Result<(), String> {
        let identity = Self::identity(path, fingerprint)?
            .ok_or("Validated runtime lost its private directory identity")?;
        self.0
            .lock()
            .map_err(|_| "Runtime validation cache is unavailable".to_owned())?
            .insert(identity);
        Ok(())
    }
}

struct TemporaryRuntime(PathBuf);
impl Drop for TemporaryRuntime {
    fn drop(&mut self) {
        // This guard owns only the random directory created by this attempt.
        let _ = fs::remove_dir_all(&self.0);
    }
}

type CloneFile = fn(&Path, &Path) -> Result<bool, String>;

/// Off unless a staging measurement test on this thread turns it on.
///
/// `phase!` compiles the operation alone outside tests, and these helpers are
/// test-only with it. A test that arms the flag records how long each
/// operation took; every other test leaves it off and runs the same operations.
#[cfg(test)]
static STAGE_TIMING: AtomicBool = AtomicBool::new(false);

#[cfg(test)]
#[derive(Clone, Copy)]
enum Phase {
    Open,
    Clone,
    ByteCopy,
    Permissions,
    Xattr,
    FileSync,
    DirSync,
    PrivateWalk,
    Manifest,
    Digest,
    Publish,
    ParentSync,
}

#[cfg(test)]
#[derive(Clone, Copy, Default)]
struct StagePhases {
    open: Duration,
    clone: Duration,
    byte_copy: Duration,
    permissions: Duration,
    xattr: Duration,
    /// Wall span from the first `sync_all` start to the last finish.
    ///
    /// Comparable with the single-threaded measurement. `file_sync_work` is
    /// the sum of those calls, which is longer than the span once they overlap.
    file_sync: Duration,
    file_sync_work: Duration,
    sync_started: Option<Instant>,
    sync_finished: Option<Instant>,
    dir_sync: Duration,
    private_walk: Duration,
    manifest: Duration,
    digest: Duration,
    publish: Duration,
    parent_sync: Duration,
    files: u64,
    directories: u64,
    symlinks: u64,
    bytes: u64,
    cloned_files: u64,
    copied_files: u64,
}

#[cfg(test)]
thread_local! {
    static STAGE_PHASES: RefCell<Option<Arc<Mutex<StagePhases>>>> = const { RefCell::new(None) };
}

#[cfg(test)]
fn begin_stage_measurement() {
    STAGE_TIMING.store(true, Ordering::SeqCst);
    let ledger = Arc::new(Mutex::new(StagePhases::default()));
    STAGE_PHASES.with(|slot| *slot.borrow_mut() = Some(ledger));
}

#[cfg(test)]
fn end_stage_measurement() -> StagePhases {
    STAGE_TIMING.store(false, Ordering::SeqCst);
    STAGE_PHASES.with(|slot| {
        slot.borrow_mut()
            .take()
            .map(|ledger| *mutex_lock(&ledger))
            .unwrap_or_default()
    })
}

#[cfg(test)]
fn current_ledger() -> Option<Arc<Mutex<StagePhases>>> {
    STAGE_PHASES.with(|slot| slot.borrow().clone())
}

#[cfg(test)]
fn install_measurement(ledger: Option<Arc<Mutex<StagePhases>>>) {
    STAGE_PHASES.with(|slot| *slot.borrow_mut() = ledger);
}

#[cfg(test)]
fn with_phases(update: impl FnOnce(&mut StagePhases)) {
    if !STAGE_TIMING.load(Ordering::Relaxed) {
        return;
    }
    STAGE_PHASES.with(|slot| {
        let Some(ledger) = slot.borrow().clone() else {
            return;
        };
        update(&mut mutex_lock(&ledger));
    });
}

#[cfg(test)]
fn timed<T>(kind: Phase, op: impl FnOnce() -> T) -> T {
    if !STAGE_TIMING.load(Ordering::Relaxed) {
        return op();
    }
    let started = Instant::now();
    let value = op();
    let elapsed = started.elapsed();
    with_phases(|phases| match kind {
        Phase::FileSync => {
            let finished = started + elapsed;
            phases.sync_started = Some(match phases.sync_started {
                Some(existing) if existing <= started => existing,
                _ => started,
            });
            phases.sync_finished = Some(match phases.sync_finished {
                Some(existing) if existing >= finished => existing,
                _ => finished,
            });
            phases.file_sync_work += elapsed;
            if let (Some(first), Some(last)) = (phases.sync_started, phases.sync_finished) {
                phases.file_sync = last.saturating_duration_since(first);
            }
        }
        Phase::Open => phases.open += elapsed,
        Phase::Clone => phases.clone += elapsed,
        Phase::ByteCopy => phases.byte_copy += elapsed,
        Phase::Permissions => phases.permissions += elapsed,
        Phase::Xattr => phases.xattr += elapsed,
        Phase::DirSync => phases.dir_sync += elapsed,
        Phase::PrivateWalk => phases.private_walk += elapsed,
        Phase::Manifest => phases.manifest += elapsed,
        Phase::Digest => phases.digest += elapsed,
        Phase::Publish => phases.publish += elapsed,
        Phase::ParentSync => phases.parent_sync += elapsed,
    });
    value
}

#[cfg(test)]
fn note_directory() {
    with_phases(|phases| phases.directories += 1);
}

#[cfg(test)]
fn note_symlink() {
    with_phases(|phases| phases.symlinks += 1);
}

#[cfg(test)]
fn note_file(cloned: bool, bytes: u64) {
    with_phases(|phases| {
        phases.files += 1;
        phases.bytes += bytes;
        if cloned {
            phases.cloned_files += 1;
        } else {
            phases.copied_files += 1;
        }
    });
}

/// Times `op` when a measurement is armed, and runs `op` directly otherwise.
///
/// Both arms evaluate the same expression. The direct arm is the only one the
/// shipping build compiles.
macro_rules! phase {
    ($kind:expr, $op:expr) => {{
        #[cfg(test)]
        {
            timed($kind, || $op)
        }
        #[cfg(not(test))]
        {
            $op
        }
    }};
}

macro_rules! note_dir {
    () => {{
        #[cfg(test)]
        note_directory();
    }};
}

macro_rules! note_link {
    () => {{
        #[cfg(test)]
        note_symlink();
    }};
}

macro_rules! note_copied {
    ($cloned:expr, $bytes:expr) => {{
        #[cfg(test)]
        note_file($cloned, $bytes);
    }};
}

/// Files already sealed, waiting for `sync_all`. The bound keeps a large tree
/// from holding one open file per entry.
struct FileQueue {
    bound: usize,
    inner: Mutex<FileQueueState>,
    ready: Condvar,
}

struct FileQueueState {
    files: VecDeque<fs::File>,
    closed: bool,
}

impl FileQueue {
    fn new(bound: usize) -> Self {
        Self {
            bound: bound.max(1),
            inner: Mutex::new(FileQueueState {
                files: VecDeque::new(),
                closed: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn push(&self, file: fs::File) -> Result<(), String> {
        let mut state = mutex_lock(&self.inner);
        while state.files.len() >= self.bound && !state.closed {
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        if state.closed {
            return Err("File sync queue closed before the file was queued".into());
        }
        state.files.push_back(file);
        drop(state);
        self.ready.notify_one();
        Ok(())
    }

    fn pop(&self) -> Option<fs::File> {
        let mut state = mutex_lock(&self.inner);
        loop {
            if let Some(file) = state.files.pop_front() {
                drop(state);
                self.ready.notify_one();
                return Some(file);
            }
            if state.closed {
                return None;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn close(&self) {
        mutex_lock(&self.inner).closed = true;
        self.ready.notify_all();
    }
}

/// In-flight `sync_all` calls. The pending count and the wait share one mutex.
#[derive(Default)]
struct SyncWait {
    state: Mutex<SyncState>,
    done: Condvar,
}

#[derive(Default)]
struct SyncState {
    pending: usize,
    error: Option<String>,
}

impl SyncWait {
    fn start(&self) {
        mutex_lock(&self.state).pending += 1;
    }

    fn finish(&self, result: Result<(), String>) {
        let mut state = mutex_lock(&self.state);
        if let Err(error) = result {
            if state.error.is_none() {
                state.error = Some(error);
            }
        }
        state.pending = state.pending.saturating_sub(1);
        let finished = state.pending == 0;
        drop(state);
        if finished {
            self.done.notify_all();
        }
    }

    fn wait(&self) -> Result<(), String> {
        let mut state = mutex_lock(&self.state);
        while state.pending > 0 {
            state = self
                .done
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
        match state.error.clone() {
            Some(error) => Err(error),
            None => Ok(()),
        }
    }
}

/// Counts a file as pending until it is queued, including when the push panics.
struct SyncQueued<'a> {
    wait: &'a SyncWait,
    queued: bool,
}

impl<'a> SyncQueued<'a> {
    fn arm(wait: &'a SyncWait) -> Self {
        wait.start();
        Self {
            wait,
            queued: false,
        }
    }

    fn queued(mut self) {
        self.queued = true;
    }
}

impl Drop for SyncQueued<'_> {
    fn drop(&mut self) {
        if !self.queued {
            self.wait.finish(Err("File sync was not queued".into()));
        }
    }
}

/// Completes the matching `start` if a worker drops the file unfinished.
struct SyncFinish<'a> {
    wait: &'a SyncWait,
    result: Option<Result<(), String>>,
}

impl Drop for SyncFinish<'_> {
    fn drop(&mut self) {
        let result = self
            .result
            .take()
            .unwrap_or_else(|| Err("File sync worker stopped before the file was durable".into()));
        self.wait.finish(result);
    }
}

fn parallelism() -> usize {
    thread::available_parallelism()
        .map(|count| count.get())
        .unwrap_or(1)
        .clamp(1, 32)
}

fn mutex_lock<T>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
thread_local! {
    static COPY_HOOK: RefCell<Option<Box<dyn FnMut(&Path)>>> = const { RefCell::new(None) };
}

#[cfg(test)]
struct CopyHookGuard;

#[cfg(test)]
impl CopyHookGuard {
    fn set(hook: impl FnMut(&Path) + 'static) -> Self {
        COPY_HOOK.with(|slot| *slot.borrow_mut() = Some(Box::new(hook)));
        Self
    }
}

#[cfg(test)]
impl Drop for CopyHookGuard {
    fn drop(&mut self) {
        COPY_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

fn note_file_queued(path: &Path) {
    #[cfg(test)]
    COPY_HOOK.with(|slot| {
        if let Some(hook) = slot.borrow_mut().as_mut() {
            hook(path);
        }
    });
    #[cfg(not(test))]
    let _ = path;
}

pub(super) fn stage_runtime(
    source: &Path,
    installations: &Path,
    expected: &str,
) -> Result<PathBuf, String> {
    stage_runtime_using(source, installations, expected, clone_file)
}

pub(super) fn stage_runtime_cached(
    source: &Path,
    installations: &Path,
    expected: &str,
    validated: &ValidatedRuntimes,
) -> Result<PathBuf, String> {
    validate_fingerprint(expected)?;
    let published = installations.join(expected);
    if validated.contains(&published, expected)? {
        return Ok(published);
    }
    let published = stage_runtime(source, installations, expected)?;
    validated.remember(&published, expected)?;
    Ok(published)
}

fn stage_runtime_using(
    source: &Path,
    installations: &Path,
    expected: &str,
    clone: CloneFile,
) -> Result<PathBuf, String> {
    validate_fingerprint(expected)?;
    utf8(installations)?;
    let source = source.canonicalize().map_err(|error| error.to_string())?;
    utf8(&source)?;
    nessa_local_storage::create_directory(installations).map_err(|error| error.to_string())?;
    let parent = installations
        .parent()
        .ok_or("Missing runtime installation parent")?;
    phase!(
        Phase::ParentSync,
        nessa_local_storage::sync_directory(parent).map_err(|error| error.to_string())
    )?;
    let published = installations.join(expected);
    match fs::symlink_metadata(&published) {
        Ok(_) => {
            validate_runtime(&published, expected)?;
            return Ok(published);
        }
        Err(error) if error.kind() == ErrorKind::NotFound => {}
        Err(error) => return Err(error.to_string()),
    }
    let temporary_path = installations.join(format!(".staging-{}", random_generation()?));
    DirBuilder::new()
        .mode(0o700)
        .create(&temporary_path)
        .map_err(|error| error.to_string())?;
    note_dir!();
    let temporary = TemporaryRuntime(temporary_path);
    copy_sync_and_match(&temporary.0, &source, expected, clone)?;
    phase!(Phase::Publish, publish(&temporary.0, &published))?;
    // After publication the old temporary path no longer exists; its guard cannot
    // remove the published directory even if directory synchronization fails.
    phase!(
        Phase::ParentSync,
        nessa_local_storage::sync_directory(installations).map_err(|error| error.to_string())
    )?;
    Ok(published)
}

/// Copy, flush, and hash `source` into `staged`.
///
/// The pool joins before this returns, including on failure, so the caller's
/// temporary-directory guard removes that directory only after every worker
/// has closed its files.
fn copy_sync_and_match(
    staged: &Path,
    source: &Path,
    expected: &str,
    clone: CloneFile,
) -> Result<(), String> {
    let workers = parallelism();
    let queue = FileQueue::new(workers.saturating_mul(2));
    let syncs = SyncWait::default();
    #[cfg(test)]
    let ledger = current_ledger();
    thread::scope(|scope| {
        for _ in 0..workers {
            let queue = &queue;
            let syncs = &syncs;
            #[cfg(test)]
            let ledger = ledger.clone();
            scope.spawn(move || {
                #[cfg(test)]
                install_measurement(ledger);
                sync_queued_files(queue, syncs);
            });
        }
        let directories = match copy_directory(source, source, staged, clone, &queue, &syncs) {
            Ok(directories) => directories,
            Err(error) => {
                queue.close();
                let _ = syncs.wait();
                return Err(error);
            }
        };
        // Bytes are sealed. Hashing them overlaps the syncs still in flight;
        // sync_all does not change those bytes.
        let (digest_tx, digest_rx) = mpsc::channel();
        let digest_path = staged.to_path_buf();
        #[cfg(test)]
        let digest_ledger = ledger.clone();
        scope.spawn(move || {
            #[cfg(test)]
            install_measurement(digest_ledger);
            let result = phase!(Phase::Digest, tree_fingerprint(&digest_path));
            let _ = digest_tx.send(result);
        });
        queue.close();
        syncs.wait()?;
        for directory in &directories {
            phase!(
                Phase::DirSync,
                nessa_local_storage::sync_directory(directory).map_err(|error| error.to_string())
            )?;
        }
        let checks = publication_checks(staged, expected);
        let digest = digest_rx
            .recv()
            .unwrap_or_else(|_| Err("Runtime digest stopped before publication".into()))?;
        checks?;
        if digest != expected {
            return Err(content_mismatch());
        }
        Ok(())
    })
}

fn sync_queued_files(queue: &FileQueue, syncs: &SyncWait) {
    while let Some(file) = queue.pop() {
        let mut finish = SyncFinish {
            wait: syncs,
            result: None,
        };
        finish.result = Some(phase!(
            Phase::FileSync,
            file.sync_all().map_err(|error| error.to_string())
        ));
    }
}

fn validate_fingerprint(expected: &str) -> Result<(), String> {
    if expected.len() == 64
        && expected
            .bytes()
            .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f'))
    {
        Ok(())
    } else {
        Err("Invalid runtime staging fingerprint".into())
    }
}
fn publish(temporary: &Path, published: &Path) -> Result<(), String> {
    let temporary = CString::new(utf8(temporary)?).map_err(|error| error.to_string())?;
    let published = CString::new(utf8(published)?).map_err(|error| error.to_string())?;
    // Unlike rename(), exclusive publication cannot replace an existing empty dir.
    if unsafe { libc::renamex_np(temporary.as_ptr(), published.as_ptr(), libc::RENAME_EXCL) } != 0 {
        return Err(Error::last_os_error().to_string());
    }
    Ok(())
}
fn utf8(path: &Path) -> Result<&str, String> {
    path.to_str()
        .ok_or_else(|| "Runtime paths must be valid UTF-8".into())
}
fn entry_name(name: OsString) -> Result<String, String> {
    name.into_string()
        .map_err(|_| "Runtime names must be valid UTF-8".into())
}
fn children(directory: &Path) -> Result<Vec<(String, PathBuf)>, String> {
    let mut entries = Vec::new();
    for entry in fs::read_dir(directory).map_err(|error| error.to_string())? {
        let entry = entry.map_err(|error| error.to_string())?;
        let name = entry_name(entry.file_name())?;
        entries.push((name, entry.path()));
    }
    // JavaScript Array.sort compares UTF-16 code units, not UTF-8 bytes.
    entries.sort_by(|left, right| left.0.encode_utf16().cmp(right.0.encode_utf16()));
    Ok(entries)
}
fn checked_link(root: &Path, path: &Path) -> Result<PathBuf, String> {
    let target = fs::read_link(path).map_err(|error| error.to_string())?;
    utf8(&target)?;
    let resolved = path.canonicalize().map_err(|error| error.to_string())?;
    if target.is_absolute() || !resolved.starts_with(root) {
        return Err("Runtime symlink must remain inside its installation".into());
    }
    Ok(target)
}
fn copy_directory(
    root: &Path,
    source: &Path,
    destination: &Path,
    clone: CloneFile,
    queue: &FileQueue,
    syncs: &SyncWait,
) -> Result<Vec<PathBuf>, String> {
    let mut directories = Vec::new();
    for (_, path) in children(source)? {
        let name = path.file_name().ok_or("Missing runtime entry name")?;
        let output = destination.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            symlink(checked_link(root, &path)?, &output).map_err(|error| error.to_string())?;
            note_link!();
        } else if metadata.is_dir() {
            DirBuilder::new()
                .mode(0o700)
                .create(&output)
                .map_err(|error| error.to_string())?;
            note_dir!();
            directories.extend(copy_directory(root, &path, &output, clone, queue, syncs)?);
        } else if metadata.is_file() {
            copy_file(&path, &output, clone, queue, syncs)?;
        } else {
            return Err("Unsupported runtime entry type".into());
        }
    }
    // Directory entries become durable only after every file sync in the tree.
    directories.push(destination.to_path_buf());
    Ok(directories)
}

fn copy_file(
    path: &Path,
    output: &Path,
    clone: CloneFile,
    queue: &FileQueue,
    syncs: &SyncWait,
) -> Result<(), String> {
    let mut input = phase!(
        Phase::Open,
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .open(path)
            .map_err(|error| error.to_string())
    )?;
    let opened = input.metadata().map_err(|error| error.to_string())?;
    if !opened.is_file() {
        return Err("Runtime changed to a non-file during copy".into());
    }
    let cloned = phase!(Phase::Clone, clone(path, output))?;
    let mut file = phase!(
        Phase::Open,
        OpenOptions::new()
            .write(true)
            .create_new(!cloned)
            .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
            .mode(0o600)
            .open(output)
            .map_err(|error| error.to_string())
    )?;
    if !cloned {
        phase!(
            Phase::ByteCopy,
            std::io::copy(&mut input, &mut file)
                .map(|_| ())
                .map_err(|error| error.to_string())
        )?;
    }
    let mode = opened.permissions().mode();
    let bytes = opened.len();
    seal_file(&file, mode)?;
    // Drop the source descriptor before a test hook can move the bundle.
    drop(input);
    let queued = SyncQueued::arm(syncs);
    queue.push(file)?;
    queued.queued();
    note_copied!(cloned, bytes);
    note_file_queued(path);
    Ok(())
}

fn seal_file(file: &fs::File, source_mode: u32) -> Result<(), String> {
    phase!(
        Phase::Permissions,
        file.set_permissions(Permissions::from_mode(0o600 | (source_mode & 0o111)))
            .map_err(|error| error.to_string())
    )?;
    phase!(Phase::Xattr, normalize_extended_attributes(file))
}

fn finalize_file(file: &fs::File, source_mode: u32) -> Result<(), String> {
    seal_file(file, source_mode)?;
    // copyfile clones still create a new directory entry whose file state must
    // reach disk before the containing directory can be published durably.
    phase!(
        Phase::FileSync,
        file.sync_all().map_err(|error| error.to_string())
    )
}

fn normalize_extended_attributes(file: &fs::File) -> Result<(), String> {
    const PROVENANCE: &[u8] = b"com.apple.provenance";
    let names = extended_attribute_names(file.as_raw_fd())?;
    for name in names {
        let attribute = CString::new(name.as_slice()).map_err(|error| error.to_string())?;
        if unsafe { libc::fremovexattr(file.as_raw_fd(), attribute.as_ptr(), 0) } == 0 {
            continue;
        }
        let error = Error::last_os_error();
        if name == PROVENANCE && error.raw_os_error() == Some(libc::EPERM) {
            // macOS attaches this protected process-provenance marker to files
            // created by both copy paths. It cannot carry bundle-supplied policy.
            continue;
        }
        if error.raw_os_error() != Some(libc::ENOATTR) {
            return Err(error.to_string());
        }
    }
    let remaining = extended_attribute_names(file.as_raw_fd())?;
    if remaining.iter().any(|name| name != PROVENANCE) {
        return Err("Staged runtime file retains extended attributes".into());
    }
    Ok(())
}

fn extended_attribute_names(file: libc::c_int) -> Result<Vec<Vec<u8>>, String> {
    let length = unsafe { libc::flistxattr(file, std::ptr::null_mut(), 0, 0) };
    if length < 0 {
        return Err(Error::last_os_error().to_string());
    }
    if length == 0 {
        return Ok(Vec::new());
    }
    let mut bytes = vec![0u8; length as usize];
    let written = unsafe { libc::flistxattr(file, bytes.as_mut_ptr().cast(), bytes.len(), 0) };
    if written < 0 {
        return Err(Error::last_os_error().to_string());
    }
    bytes.truncate(written as usize);
    if bytes.last() != Some(&0) {
        return Err("Invalid extended attribute list".into());
    }
    Ok(bytes
        .split(|byte| *byte == 0)
        .filter(|name| !name.is_empty())
        .map(<[u8]>::to_vec)
        .collect())
}

fn clone_file(source: &Path, destination: &Path) -> Result<bool, String> {
    let source = CString::new(utf8(source)?).map_err(|error| error.to_string())?;
    let destination = CString::new(utf8(destination)?).map_err(|error| error.to_string())?;
    let flags = libc::COPYFILE_DATA | libc::COPYFILE_CLONE_FORCE | libc::COPYFILE_NOFOLLOW_SRC;
    if unsafe {
        libc::copyfile(
            source.as_ptr(),
            destination.as_ptr(),
            std::ptr::null_mut(),
            flags,
        )
    } == 0
    {
        return Ok(true);
    }
    let error = Error::last_os_error();
    if matches!(
        error.raw_os_error(),
        Some(libc::ENOTSUP) | Some(libc::EXDEV)
    ) {
        Ok(false)
    } else {
        Err(error.to_string())
    }
}
/// How launchd starts this staged runtime.
///
/// The runtime directory is named here and nowhere else in the service's
/// environment. Everything the gateway runs out of it — `nessa`, `node`, the
/// ACP entry, `nessa-mcp` — it addresses by absolute path, so the directory
/// does not belong on any `PATH`; on the agent's it would mean Nessa's bundled
/// Node shadowing whichever one the user's project pinned.
pub(super) fn launch_settings(runtime: &Path) -> Value {
    json!([
        runtime.join("nessa"),
        "server",
        "--desktop-runtime",
        runtime
    ])
}
fn validate_private_tree(path: &Path) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.uid() != unsafe { libc::geteuid() } {
        return Err("Published runtime has another owner".into());
    }
    if metadata.file_type().is_symlink() {
        return Ok(());
    }
    if metadata.is_dir() {
        if metadata.permissions().mode() & 0o7777 != 0o700 {
            return Err("Published runtime directories must remain private".into());
        }
        for (_, child) in children(path)? {
            validate_private_tree(&child)?;
        }
    } else if metadata.is_file() {
        if metadata.permissions().mode() & 0o7666 != 0o600 || metadata.nlink() != 1 {
            return Err("Published runtime files must remain private and unaliased".into());
        }
    } else {
        return Err("Unsupported published runtime entry".into());
    }
    Ok(())
}
fn content_mismatch() -> String {
    "Runtime content does not match its manifest; installation preserved".into()
}

pub(super) fn validate_runtime(directory: &Path, expected: &str) -> Result<(), String> {
    publication_checks(directory, expected)?;
    if phase!(Phase::Digest, tree_fingerprint(directory))? != expected {
        return Err(content_mismatch());
    }
    Ok(())
}

fn publication_checks(directory: &Path, expected: &str) -> Result<(), String> {
    let metadata = fs::symlink_metadata(directory).map_err(|error| error.to_string())?;
    if !metadata.is_dir() || metadata.file_type().is_symlink() {
        return Err("Published runtime must be a real directory".into());
    }
    let manifest =
        fs::symlink_metadata(directory.join("manifest.json")).map_err(|error| error.to_string())?;
    if !manifest.is_file() || manifest.file_type().is_symlink() {
        return Err("Runtime manifest must be a regular file".into());
    }
    phase!(Phase::PrivateWalk, validate_private_tree(directory))?;
    if phase!(Phase::Manifest, runtime_fingerprint(directory))? != expected {
        return Err(content_mismatch());
    }
    Ok(())
}

/// Full-tree SHA-256 in the packaging walk order.
///
/// The byte sequence is one hash of framed entries, not a tree of per-file
/// hashes, so it cannot be recombined from worker results. Small files are
/// read ahead on a bounded pool; one hasher still updates each frame and its
/// bytes in walk order. A large file is streamed on that hasher.
fn tree_fingerprint(directory: &Path) -> Result<String, String> {
    let root = directory
        .canonicalize()
        .map_err(|error| error.to_string())?;
    let mut plan = Vec::new();
    let mut small = Vec::new();
    plan_entry(&root, &root, "", &mut plan, &mut small)?;
    hash_plan(&plan, small)
}

fn framed(value: Value) -> Result<Vec<u8>, String> {
    let mut bytes = serde_json::to_vec(&value).map_err(|error| error.to_string())?;
    bytes.push(b'\n');
    Ok(bytes)
}

fn plan_entry(
    root: &Path,
    path: &Path,
    name: &str,
    plan: &mut Vec<PlanStep>,
    small: &mut Vec<SmallJob>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    if metadata.file_type().is_symlink() {
        let target = checked_link(root, path)?;
        plan.push(PlanStep::Bytes(framed(json!([
            name,
            "link",
            utf8(&target)?
        ]))?));
    } else if metadata.is_dir() {
        plan.push(PlanStep::Bytes(framed(json!([name, "directory"]))?));
        for (child, path) in children(path)? {
            if name.is_empty() && child == "manifest.json" {
                continue;
            }
            let relative = if name.is_empty() {
                child
            } else {
                format!("{name}/{child}")
            };
            plan_entry(root, &path, &relative, plan, small)?;
        }
    } else if metadata.is_file() {
        let index = plan.len();
        if metadata.len() > SMALL_FILE_LIMIT {
            plan.push(PlanStep::Large {
                path: path.to_path_buf(),
                name: name.to_owned(),
            });
        } else {
            plan.push(PlanStep::Small { index });
            small.push(SmallJob {
                index,
                path: path.to_path_buf(),
                name: name.to_owned(),
            });
        }
    } else {
        return Err("Unsupported runtime entry type".into());
    }
    Ok(())
}

const SMALL_FILE_LIMIT: u64 = 1024 * 1024;

enum PlanStep {
    Bytes(Vec<u8>),
    Small { index: usize },
    Large { path: PathBuf, name: String },
}

struct SmallJob {
    index: usize,
    path: PathBuf,
    name: String,
}

struct SmallRead {
    header: Vec<u8>,
    bytes: Vec<u8>,
}

struct Prefetch {
    state: Mutex<PrefetchState>,
    ready: Condvar,
}

struct PrefetchState {
    pending: VecDeque<SmallJob>,
    done: HashMap<usize, Result<SmallRead, String>>,
    issued: usize,
    consumed: usize,
    limit: usize,
    stop: bool,
}

impl Prefetch {
    fn new(jobs: Vec<SmallJob>, limit: usize) -> Self {
        Self {
            state: Mutex::new(PrefetchState {
                pending: VecDeque::from(jobs),
                done: HashMap::new(),
                issued: 0,
                consumed: 0,
                limit: limit.max(1),
                stop: false,
            }),
            ready: Condvar::new(),
        }
    }

    fn take_job(&self) -> Option<SmallJob> {
        let mut state = mutex_lock(&self.state);
        loop {
            if state.stop {
                return None;
            }
            if state.pending.is_empty() {
                return None;
            }
            if state.issued.saturating_sub(state.consumed) >= state.limit {
                state = self
                    .ready
                    .wait(state)
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                continue;
            }
            let job = state.pending.pop_front().unwrap();
            state.issued += 1;
            drop(state);
            self.ready.notify_all();
            return Some(job);
        }
    }

    fn finish(&self, index: usize, result: Result<SmallRead, String>) {
        mutex_lock(&self.state).done.insert(index, result);
        self.ready.notify_all();
    }

    fn take(&self, index: usize) -> Result<SmallRead, String> {
        let mut state = mutex_lock(&self.state);
        loop {
            if let Some(result) = state.done.remove(&index) {
                state.consumed += 1;
                drop(state);
                self.ready.notify_all();
                return result;
            }
            state = self
                .ready
                .wait(state)
                .unwrap_or_else(|poisoned| poisoned.into_inner());
        }
    }

    fn stop(&self) {
        mutex_lock(&self.state).stop = true;
        self.ready.notify_all();
    }

    fn run_worker(&self) {
        while let Some(job) = self.take_job() {
            let mut guard = ReadGuard {
                prefetch: self,
                index: job.index,
                done: false,
            };
            guard.complete(read_small(&job));
        }
    }
}

struct ReadGuard<'a> {
    prefetch: &'a Prefetch,
    index: usize,
    done: bool,
}

impl ReadGuard<'_> {
    fn complete(&mut self, result: Result<SmallRead, String>) {
        self.prefetch.finish(self.index, result);
        self.done = true;
    }
}

impl Drop for ReadGuard<'_> {
    fn drop(&mut self) {
        if !self.done {
            self.prefetch
                .finish(self.index, Err("Runtime file read stopped".into()));
        }
    }
}

fn hash_plan(plan: &[PlanStep], small: Vec<SmallJob>) -> Result<String, String> {
    if small.is_empty() {
        return hash_ordered(plan, None);
    }
    let readers = parallelism().min(small.len()).max(1);
    let prefetch = Prefetch::new(small, readers.saturating_mul(2));
    thread::scope(|scope| {
        for _ in 0..readers {
            scope.spawn(|| prefetch.run_worker());
        }
        let result = hash_ordered(plan, Some(&prefetch));
        prefetch.stop();
        result
    })
}

fn hash_ordered(plan: &[PlanStep], prefetch: Option<&Prefetch>) -> Result<String, String> {
    let mut hash = Sha256::new();
    for step in plan {
        match step {
            PlanStep::Bytes(bytes) => hash.update(bytes),
            PlanStep::Small { index } => {
                let read = prefetch
                    .ok_or("Runtime digest is missing a file read")?
                    .take(*index)?;
                hash.update(&read.header);
                hash.update(&read.bytes);
            }
            PlanStep::Large { path, name } => stream_file(&mut hash, path, name)?,
        }
    }
    Ok(format!("{:x}", hash.finalize()))
}

fn open_nofollow(path: &Path) -> Result<fs::File, String> {
    OpenOptions::new()
        .read(true)
        .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
        .open(path)
        .map_err(|error| error.to_string())
}

fn read_small(job: &SmallJob) -> Result<SmallRead, String> {
    let mut file = open_nofollow(&job.path)?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() || metadata.len() > SMALL_FILE_LIMIT {
        return Err("Runtime file changed while hashing".into());
    }
    let header = framed(json!([
        job.name,
        "file",
        metadata.permissions().mode() & 0o111,
        metadata.len()
    ]))?;
    let mut bytes = Vec::new();
    file.read_to_end(&mut bytes)
        .map_err(|error| error.to_string())?;
    if bytes.len() as u64 != metadata.len() {
        return Err("Runtime file changed while hashing".into());
    }
    Ok(SmallRead { header, bytes })
}

fn stream_file(hash: &mut Sha256, path: &Path, name: &str) -> Result<(), String> {
    let mut file = open_nofollow(path)?;
    let metadata = file.metadata().map_err(|error| error.to_string())?;
    if !metadata.is_file() {
        return Err("Runtime changed during fingerprinting".into());
    }
    hash.update(&framed(json!([
        name,
        "file",
        metadata.permissions().mode() & 0o111,
        metadata.len()
    ]))?);
    let expected = metadata.len();
    let mut bytes = [0u8; 65536];
    let mut count = 0u64;
    loop {
        let read = file.read(&mut bytes).map_err(|error| error.to_string())?;
        if read == 0 {
            break;
        }
        count += read as u64;
        hash.update(&bytes[..read]);
    }
    if count != expected {
        return Err("Runtime file changed while hashing".into());
    }
    Ok(())
}
#[cfg(test)]
#[path = "../../../../tests/gateway/infrastructure/staging.rs"]
mod tests;
