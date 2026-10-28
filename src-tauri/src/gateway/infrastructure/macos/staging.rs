//! Immutable runtime installations. Copy and verify in a private sibling directory,
//! then publish once; old and already-published generations are never rewritten.
//!
//! The shipping copy clones the whole source tree with one `clonefile`, checks
//! that clone against the survey, then one thread seals every directory and
//! every file. A directory is opened without following a link, set to `0700`,
//! and stripped of extended attributes by the same normalization a file gets,
//! before any file is sealed. A path that is not the surveyed type — a
//! directory or file replaced by a symlink, or a symlink the survey did not
//! record — refuses the attempt before any permission, attribute, or sync
//! call. The digest follows the seal: it reads execute bits, and those bits
//! have to be the sealed mode.
//! `ENOTSUP` and `EXDEV` fall back to per-file work on one thread, and that
//! path's digest overlaps the syncs after every seal has returned. Progress
//! counts a file only after that `sync_all` returns, and the zero report is
//! sent once the file count is known, before `clonefile`. Every file sync
//! and every directory sync finishes before the exclusive rename. A failed
//! attempt drops only its own temporary directory, after any digest thread
//! has joined. The attempt owns that directory; a worker does not.
use super::{generation::random_generation, runtime_fingerprint};
use crate::gateway::application::{RuntimeCopyPhase, RuntimeCopyProgress};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
#[cfg(test)]
use std::{
    cell::RefCell,
    sync::Arc,
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
    sync::atomic::{AtomicBool, AtomicUsize, Ordering},
    sync::{mpsc, Condvar, Mutex},
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
    /// The shipping path's one directory `clonefile` returned success.
    /// Per-file fallback and the sequential comparison leave this false.
    tree_cloned: bool,
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

/// A sealed file waiting for `sync_all`. `bytes` is what progress counts after
/// that sync returns.
struct SealedFile {
    file: fs::File,
    bytes: u64,
}

/// Files already sealed, waiting for `sync_all`. The bound keeps a large tree
/// from holding one open file per entry.
struct FileQueue {
    bound: usize,
    inner: Mutex<FileQueueState>,
    ready: Condvar,
}

struct FileQueueState {
    files: VecDeque<SealedFile>,
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

    fn push(&self, file: SealedFile) -> Result<(), String> {
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

    fn pop(&self) -> Option<SealedFile> {
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

fn mutex_lock<T: ?Sized>(mutex: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

#[cfg(test)]
type CopyHookFn = dyn FnMut(&Path) -> Result<(), String> + Send;
#[cfg(test)]
type SyncHookFn = dyn FnMut() + Send;

// The attempt that is staging captures these on its own thread. Workers use
// that capture. They do not read the slot, so a test on another thread cannot
// see or clear them.
#[cfg(test)]
thread_local! {
    static COPY_HOOK: RefCell<Option<Arc<Mutex<CopyHookFn>>>> = const { RefCell::new(None) };
    static SYNC_HOOK: RefCell<Option<Arc<Mutex<SyncHookFn>>>> = const { RefCell::new(None) };
}

#[cfg(test)]
struct AttemptHooks {
    copy: Option<Arc<Mutex<CopyHookFn>>>,
    before_sync: Option<Arc<Mutex<SyncHookFn>>>,
}

#[cfg(not(test))]
struct AttemptHooks;

impl AttemptHooks {
    fn capture() -> Self {
        #[cfg(test)]
        {
            Self {
                copy: COPY_HOOK.with(|slot| slot.borrow().clone()),
                before_sync: SYNC_HOOK.with(|slot| slot.borrow().clone()),
            }
        }
        #[cfg(not(test))]
        {
            Self
        }
    }

    fn note_file(&self, path: &Path) -> Result<(), String> {
        #[cfg(test)]
        if let Some(hook) = &self.copy {
            return mutex_lock(hook)(path);
        }
        let _ = (self, path);
        Ok(())
    }

    fn note_before_sync(&self) {
        #[cfg(test)]
        if let Some(hook) = &self.before_sync {
            mutex_lock(hook)();
        }
        #[cfg(not(test))]
        let _ = self;
    }
}

#[cfg(test)]
struct CopyHookGuard;

#[cfg(test)]
impl CopyHookGuard {
    fn set(hook: impl FnMut(&Path) -> Result<(), String> + Send + 'static) -> Self {
        COPY_HOOK.with(|slot| *slot.borrow_mut() = Some(Arc::new(Mutex::new(hook))));
        Self
    }
}

#[cfg(test)]
impl Drop for CopyHookGuard {
    fn drop(&mut self) {
        COPY_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

#[cfg(test)]
struct SyncHookGuard;

#[cfg(test)]
impl SyncHookGuard {
    fn set(hook: impl FnMut() + Send + 'static) -> Self {
        SYNC_HOOK.with(|slot| *slot.borrow_mut() = Some(Arc::new(Mutex::new(hook))));
        Self
    }
}

#[cfg(test)]
impl Drop for SyncHookGuard {
    fn drop(&mut self) {
        SYNC_HOOK.with(|slot| *slot.borrow_mut() = None);
    }
}

pub(super) fn stage_runtime(
    source: &Path,
    installations: &Path,
    expected: &str,
) -> Result<PathBuf, String> {
    stage_runtime_tree(source, installations, expected, None, &clone_directory)
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

/// One file the copy will clone or byte-copy. `bytes` is the size known
/// before any copy starts, and the total the progress report is measured
/// against. `mode` is the source mode at survey time; sealing keeps its
/// execute bits.
struct PlannedFile {
    source: PathBuf,
    destination: PathBuf,
    bytes: u64,
    mode: u32,
}

struct PlannedLink {
    destination: PathBuf,
    target: PathBuf,
}

struct PlannedCopy {
    files: Vec<PlannedFile>,
    /// Post-order: a directory is listed after the directories inside it, so
    /// directory `sync` runs after the entries it names are durable. Reversed,
    /// it is a parent-before-child create order.
    directories: Vec<PathBuf>,
    links: Vec<PlannedLink>,
}

/// What the survey recorded at one path. The clone is accepted only when every
/// path still has this type, read with `symlink_metadata`.
enum SurveyedEntry {
    Directory,
    File,
    Link(PathBuf),
}

/// The clone must still be the surveyed tree. Nothing is followed.
///
/// A directory that became a symlink after the survey would be chmod'd through
/// that link, and a file open only passes `O_NOFOLLOW` on its last component.
/// This is the check before any of those calls.
fn cloned_tree_matches(staged: &Path, plan: &PlannedCopy) -> Result<(), String> {
    let mut surveyed = HashMap::new();
    for directory in &plan.directories {
        surveyed.insert(directory.clone(), SurveyedEntry::Directory);
    }
    for file in &plan.files {
        surveyed.insert(file.destination.clone(), SurveyedEntry::File);
    }
    for link in &plan.links {
        surveyed.insert(
            link.destination.clone(),
            SurveyedEntry::Link(link.target.clone()),
        );
    }
    let mut seen = HashSet::new();
    match_cloned_entry(staged, &surveyed, &mut seen)?;
    if seen.len() != surveyed.len() {
        return Err("Cloned runtime does not match the surveyed tree".into());
    }
    Ok(())
}

fn match_cloned_entry(
    path: &Path,
    surveyed: &HashMap<PathBuf, SurveyedEntry>,
    seen: &mut HashSet<PathBuf>,
) -> Result<(), String> {
    let metadata = fs::symlink_metadata(path).map_err(|error| error.to_string())?;
    let kind = metadata.file_type();
    let Some(expected) = surveyed.get(path) else {
        if kind.is_symlink() {
            return Err(substituted_symlink(path));
        }
        return Err("Cloned runtime does not match the surveyed tree".into());
    };
    match expected {
        SurveyedEntry::Directory => {
            if kind.is_symlink() {
                return Err(substituted_symlink(path));
            }
            if !metadata.is_dir() {
                return Err("Cloned runtime does not match the surveyed tree".into());
            }
            seen.insert(path.to_path_buf());
            for (_, child) in children(path)? {
                match_cloned_entry(&child, surveyed, seen)?;
            }
        }
        SurveyedEntry::File => {
            if kind.is_symlink() {
                return Err(substituted_symlink(path));
            }
            if !metadata.is_file() {
                return Err("Cloned runtime does not match the surveyed tree".into());
            }
            seen.insert(path.to_path_buf());
        }
        SurveyedEntry::Link(target) => {
            if !kind.is_symlink() {
                return Err("Cloned runtime does not match the surveyed tree".into());
            }
            let actual = fs::read_link(path).map_err(|error| error.to_string())?;
            if &actual != target {
                return Err(substituted_symlink(path));
            }
            seen.insert(path.to_path_buf());
        }
    }
    Ok(())
}

fn substituted_symlink(path: &Path) -> String {
    format!("Cloned runtime substituted a symlink at {}", path.display())
}

/// How far `sync_all` has got. A file is counted after its own sync returns.
/// The count and the callback share one lock, so a slower report cannot move
/// the percent backwards and cannot land after the final file. The callback
/// must not call back into this progress. A non-final report is sent only when
/// its percent is higher than the last one sent. The final file is always sent.
/// `sync_progress_counts_a_file_only_after_its_sync_returns` observes the count.
/// `a_slower_progress_report_does_not_move_the_percent_backwards` holds the callback.
struct CopyProgress<'a> {
    state: Mutex<CopyCount>,
    report: Option<&'a (dyn Fn(RuntimeCopyProgress) + Sync)>,
}

struct CopyCount {
    files_completed: u64,
    bytes_completed: u64,
    files_total: u64,
    bytes_total: u64,
    last_percent: u64,
    final_sent: bool,
}

impl<'a> CopyProgress<'a> {
    fn start(
        files_total: u64,
        bytes_total: u64,
        report: Option<&'a (dyn Fn(RuntimeCopyProgress) + Sync)>,
    ) -> Self {
        let progress = Self {
            state: Mutex::new(CopyCount {
                files_completed: 0,
                bytes_completed: 0,
                files_total,
                bytes_total,
                last_percent: u64::MAX,
                final_sent: false,
            }),
            report,
        };
        progress.synced(0, true);
        progress
    }

    /// `opening` is the zero report from [`Self::start`]. A finished file
    /// passes `opening` as false and adds its planned size.
    fn synced(&self, bytes: u64, opening: bool) {
        let Some(report) = self.report else {
            return;
        };
        let mut state = mutex_lock(&self.state);
        if state.final_sent || state.files_total == 0 {
            return;
        }
        if !opening {
            state.files_completed += 1;
            state.bytes_completed += bytes;
        }
        let files = state.files_completed;
        let done = state.bytes_completed;
        let percent = copy_percent(files, done, state.files_total, state.bytes_total);
        let is_final = files == state.files_total;
        if !is_final && state.last_percent != u64::MAX && percent <= state.last_percent {
            return;
        }
        state.last_percent = percent;
        state.final_sent = is_final;
        let progress = RuntimeCopyProgress {
            state: RuntimeCopyPhase::Copying,
            files_completed: files,
            files_total: state.files_total,
            bytes_completed: done,
            bytes_total: state.bytes_total,
        };
        report(progress);
    }
}

fn copy_percent(files: u64, bytes: u64, files_total: u64, bytes_total: u64) -> u64 {
    let (done, total) = if bytes_total > 0 {
        (bytes, bytes_total)
    } else {
        (files, files_total)
    };
    done.saturating_mul(100).checked_div(total).unwrap_or(100)
}

pub(super) fn stage_runtime_reporting(
    source: &Path,
    installations: &Path,
    expected: &str,
    report: &(dyn Fn(RuntimeCopyProgress) + Sync),
) -> Result<PathBuf, String> {
    stage_runtime_tree(
        source,
        installations,
        expected,
        Some(report),
        &clone_directory,
    )
}

#[cfg(test)]
fn stage_runtime_using(
    source: &Path,
    installations: &Path,
    expected: &str,
    clone: impl Fn(&Path, &Path) -> Result<bool, String> + Sync,
    report: Option<&(dyn Fn(RuntimeCopyProgress) + Sync)>,
) -> Result<PathBuf, String> {
    stage_files(
        source,
        installations,
        expected,
        report,
        parallelism(),
        &clone,
    )
}

/// Per-file clone on one thread. The comparison test times this against the
/// shipping tree clone. It is test-only: production uses [`stage_runtime`].
#[cfg(test)]
fn stage_runtime_sequential(
    source: &Path,
    installations: &Path,
    expected: &str,
    report: Option<&(dyn Fn(RuntimeCopyProgress) + Sync)>,
) -> Result<PathBuf, String> {
    stage_files(source, installations, expected, report, 1, &clone_file)
}

/// Shipping copy, with the directory clone injected so a test can refuse it.
fn stage_runtime_tree(
    source: &Path,
    installations: &Path,
    expected: &str,
    report: Option<&(dyn Fn(RuntimeCopyProgress) + Sync)>,
    clone_tree: &(impl Fn(&Path, &Path) -> Result<bool, String> + Sync),
) -> Result<PathBuf, String> {
    stage_attempt(
        source,
        installations,
        expected,
        false,
        |staged, source, hooks| copy_tree(staged, source, expected, clone_tree, report, hooks),
    )
}

#[cfg(test)]
fn stage_files(
    source: &Path,
    installations: &Path,
    expected: &str,
    report: Option<&(dyn Fn(RuntimeCopyProgress) + Sync)>,
    workers: usize,
    clone: &(impl Fn(&Path, &Path) -> Result<bool, String> + Sync),
) -> Result<PathBuf, String> {
    stage_attempt(
        source,
        installations,
        expected,
        true,
        |staged, source, hooks| {
            copy_sync_and_match(staged, source, expected, clone, workers, report, hooks)
        },
    )
}

fn stage_attempt(
    source: &Path,
    installations: &Path,
    expected: &str,
    create_directory: bool,
    copy: impl FnOnce(&Path, &Path, &AttemptHooks) -> Result<(), String>,
) -> Result<PathBuf, String> {
    let hooks = AttemptHooks::capture();
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
    if create_directory {
        DirBuilder::new()
            .mode(0o700)
            .create(&temporary_path)
            .map_err(|error| error.to_string())?;
        note_dir!();
    }
    let temporary = TemporaryRuntime(temporary_path);
    copy(&temporary.0, &source, &hooks)?;
    phase!(Phase::Publish, publish(&temporary.0, &published))?;
    // After publication the old temporary path no longer exists; its guard cannot
    // remove the published directory even if directory synchronization fails.
    phase!(
        Phase::ParentSync,
        nessa_local_storage::sync_directory(installations).map_err(|error| error.to_string())
    )?;
    Ok(published)
}

/// Copy, flush, and hash `source` into `staged` one file at a time.
///
/// Directories and symlinks are created before file workers start. `workers`
/// is 1 for the comparison's old path. The injected overlap tests pass the
/// machine's parallelism. The copy pool and the sync pool both join before
/// this returns, including on failure, so the caller's temporary-directory
/// guard removes that directory only after every worker has closed its files.
/// The attempt owns the directory. Production does not call this: it
/// `clonefile`s the tree, and its per-file fallback calls `run_planned`
/// with one worker.
#[cfg(test)]
fn copy_sync_and_match(
    staged: &Path,
    source: &Path,
    expected: &str,
    clone: &(impl Fn(&Path, &Path) -> Result<bool, String> + Sync),
    workers: usize,
    report: Option<&(dyn Fn(RuntimeCopyProgress) + Sync)>,
    hooks: &AttemptHooks,
) -> Result<(), String> {
    let mut plan = PlannedCopy {
        files: Vec::new(),
        directories: Vec::new(),
        links: Vec::new(),
    };
    walk_tree(source, source, staged, &mut plan, true)?;
    let bytes_total = plan.files.iter().map(|file| file.bytes).sum();
    let progress = CopyProgress::start(plan.files.len() as u64, bytes_total, report);
    run_planned(staged, expected, &plan, workers, &progress, hooks, clone)
}

/// One `clonefile` of the source tree, then one thread seals and syncs.
///
/// The zero report goes out once the survey knows the file count, before
/// `clonefile`, so the bar is not stuck behind that call. `Ok(false)` is
/// `ENOTSUP` or `EXDEV`: the per-file path then runs on one thread.
fn copy_tree(
    staged: &Path,
    source: &Path,
    expected: &str,
    clone_tree: &(impl Fn(&Path, &Path) -> Result<bool, String> + Sync),
    report: Option<&(dyn Fn(RuntimeCopyProgress) + Sync)>,
    hooks: &AttemptHooks,
) -> Result<(), String> {
    let mut plan = PlannedCopy {
        files: Vec::new(),
        directories: Vec::new(),
        links: Vec::new(),
    };
    walk_tree(source, source, staged, &mut plan, false)?;
    let bytes_total = plan.files.iter().map(|file| file.bytes).sum();
    let progress = CopyProgress::start(plan.files.len() as u64, bytes_total, report);
    let cloned = phase!(Phase::Clone, clone_tree(source, staged))?;
    if cloned {
        #[cfg(test)]
        with_phases(|phases| phases.tree_cloned = true);
        return seal_cloned_tree(staged, expected, &plan, &progress, hooks);
    }
    discard_tree(staged)?;
    materialize(&plan)?;
    run_planned(staged, expected, &plan, 1, &progress, hooks, &clone_file)
}

fn run_planned(
    staged: &Path,
    expected: &str,
    plan: &PlannedCopy,
    workers: usize,
    progress: &CopyProgress<'_>,
    hooks: &AttemptHooks,
    clone: &(impl Fn(&Path, &Path) -> Result<bool, String> + Sync),
) -> Result<(), String> {
    let workers = if plan.files.is_empty() {
        0
    } else {
        workers.max(1)
    };
    let queue = FileQueue::new(workers.saturating_mul(2));
    let syncs = SyncWait::default();
    let copies = SyncWait::default();
    for _ in &plan.files {
        copies.start();
    }
    let next = AtomicUsize::new(0);
    let cancel = AtomicBool::new(false);
    let copy = CopyAttempt {
        plan,
        next: &next,
        cancel: &cancel,
        queue: &queue,
        copies: &copies,
        syncs: &syncs,
        hooks,
    };
    let attempt = &copy;
    #[cfg(test)]
    let ledger = current_ledger();
    thread::scope(|scope| {
        for _ in 0..workers {
            let queue = &queue;
            let syncs = &syncs;
            let progress = &progress;
            #[cfg(test)]
            let ledger = ledger.clone();
            scope.spawn(move || {
                #[cfg(test)]
                install_measurement(ledger);
                sync_queued_files(queue, syncs, progress, hooks);
            });
        }
        for _ in 0..workers {
            #[cfg(test)]
            let ledger = ledger.clone();
            scope.spawn(move || {
                #[cfg(test)]
                install_measurement(ledger);
                copy_planned_files(attempt, clone);
            });
        }
        let copied = copies.wait();
        if let Err(error) = copied {
            // Sync workers leave only after the queue is closed. Joining them
            // here keeps the temporary directory alive until their files close.
            queue.close();
            let _ = syncs.wait();
            return Err(error);
        }
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
        matched_tree(staged, expected, &plan.directories, recv_digest(&digest_rx))
    })
}

fn seal_cloned_tree(
    staged: &Path,
    expected: &str,
    plan: &PlannedCopy,
    progress: &CopyProgress<'_>,
    hooks: &AttemptHooks,
) -> Result<(), String> {
    cloned_tree_matches(staged, plan)?;
    for directory in &plan.directories {
        seal_directory(directory)?;
    }
    // One thread, so the digest cannot overlap this loop without hashing a
    // file that is not sealed yet. The fingerprint includes execute bits.
    // Seal keeps the bits the source had; hashing afterwards reads those bits
    // and not a mode `clonefile` was still wearing. `sync_all` is in this
    // loop so a crash cannot observe a renamed tree whose file metadata never
    // reached disk.
    for file in &plan.files {
        hooks.note_file(&file.source)?;
        let sealed = phase!(
            Phase::Open,
            OpenOptions::new()
                .write(true)
                .custom_flags(libc::O_NOFOLLOW | libc::O_NONBLOCK)
                .open(&file.destination)
                .map_err(|error| error.to_string())
        )?;
        seal_file(&sealed, file.mode)?;
        phase!(
            Phase::FileSync,
            sealed.sync_all().map_err(|error| error.to_string())
        )?;
        progress.synced(file.bytes, false);
        #[cfg(test)]
        note_file(true, file.bytes);
    }
    let digest = phase!(Phase::Digest, tree_fingerprint(staged));
    matched_tree(staged, expected, &plan.directories, digest)
}

fn recv_digest(digest_rx: &mpsc::Receiver<Result<String, String>>) -> Result<String, String> {
    digest_rx
        .recv()
        .unwrap_or_else(|_| Err("Runtime digest stopped before publication".into()))
}

fn matched_tree(
    staged: &Path,
    expected: &str,
    directories: &[PathBuf],
    digest: Result<String, String>,
) -> Result<(), String> {
    for directory in directories {
        phase!(
            Phase::DirSync,
            nessa_local_storage::sync_directory(directory).map_err(|error| error.to_string())
        )?;
    }
    publication_checks(staged, expected)?;
    if digest? != expected {
        return Err(content_mismatch());
    }
    Ok(())
}

fn materialize(plan: &PlannedCopy) -> Result<(), String> {
    for directory in plan.directories.iter().rev() {
        DirBuilder::new()
            .mode(0o700)
            .create(directory)
            .map_err(|error| error.to_string())?;
    }
    for link in &plan.links {
        symlink(&link.target, &link.destination).map_err(|error| error.to_string())?;
    }
    Ok(())
}

fn discard_tree(path: &Path) -> Result<(), String> {
    match fs::symlink_metadata(path) {
        Err(error) if error.kind() == ErrorKind::NotFound => Ok(()),
        Err(error) => Err(error.to_string()),
        Ok(_) => fs::remove_dir_all(path).map_err(|error| error.to_string()),
    }
}

fn sync_queued_files(
    queue: &FileQueue,
    syncs: &SyncWait,
    progress: &CopyProgress<'_>,
    hooks: &AttemptHooks,
) {
    while let Some(sealed) = queue.pop() {
        let mut finish = SyncFinish {
            wait: syncs,
            result: None,
        };
        hooks.note_before_sync();
        let result = phase!(
            Phase::FileSync,
            sealed.file.sync_all().map_err(|error| error.to_string())
        );
        if result.is_ok() {
            progress.synced(sealed.bytes, false);
        }
        finish.result = Some(result);
    }
}

struct CopyAttempt<'a> {
    plan: &'a PlannedCopy,
    next: &'a AtomicUsize,
    cancel: &'a AtomicBool,
    queue: &'a FileQueue,
    copies: &'a SyncWait,
    syncs: &'a SyncWait,
    hooks: &'a AttemptHooks,
}

fn copy_planned_files(
    attempt: &CopyAttempt<'_>,
    clone: &(impl Fn(&Path, &Path) -> Result<bool, String> + Sync),
) {
    loop {
        let index = attempt.next.fetch_add(1, Ordering::Relaxed);
        if index >= attempt.plan.files.len() {
            return;
        }
        let mut finish = SyncFinish {
            wait: attempt.copies,
            result: None,
        };
        if attempt.cancel.load(Ordering::Relaxed) {
            // The first error is already recorded. This file is not copied,
            // and finishing Ok leaves that error as the one `wait` returns.
            finish.result = Some(Ok(()));
            continue;
        }
        let file = &attempt.plan.files[index];
        match copy_file(
            &file.source,
            &file.destination,
            file.bytes,
            clone,
            attempt.queue,
            attempt.syncs,
            attempt.hooks,
        ) {
            Ok(()) => finish.result = Some(Ok(())),
            Err(error) => {
                attempt.cancel.store(true, Ordering::Relaxed);
                finish.result = Some(Err(error));
            }
        }
    }
}

/// Survey `source` into `plan`.
///
/// `create` is the per-file path: it makes child directories and symlinks as
/// it goes, and the caller has already created and counted the root. The tree
/// path passes `create` as false, counts every directory including the root,
/// and records symlinks for [`materialize`] if `clonefile` cannot run. Either
/// way a directory is pushed after its children.
fn walk_tree(
    root: &Path,
    source: &Path,
    destination: &Path,
    plan: &mut PlannedCopy,
    create: bool,
) -> Result<(), String> {
    if !create {
        note_dir!();
    }
    for (_, path) in children(source)? {
        let name = path.file_name().ok_or("Missing runtime entry name")?;
        let output = destination.join(name);
        let metadata = fs::symlink_metadata(&path).map_err(|error| error.to_string())?;
        if metadata.file_type().is_symlink() {
            let target = checked_link(root, &path)?;
            if create {
                symlink(&target, &output).map_err(|error| error.to_string())?;
            } else {
                plan.links.push(PlannedLink {
                    destination: output,
                    target,
                });
            }
            note_link!();
        } else if metadata.is_dir() {
            if create {
                DirBuilder::new()
                    .mode(0o700)
                    .create(&output)
                    .map_err(|error| error.to_string())?;
                note_dir!();
            }
            walk_tree(root, &path, &output, plan, create)?;
        } else if metadata.is_file() {
            plan.files.push(PlannedFile {
                source: path,
                destination: output,
                bytes: metadata.len(),
                mode: metadata.permissions().mode(),
            });
        } else {
            return Err("Unsupported runtime entry type".into());
        }
    }
    plan.directories.push(destination.to_path_buf());
    Ok(())
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
fn copy_file(
    path: &Path,
    output: &Path,
    planned_bytes: u64,
    clone: &(impl Fn(&Path, &Path) -> Result<bool, String> + Sync),
    queue: &FileQueue,
    syncs: &SyncWait,
    hooks: &AttemptHooks,
) -> Result<(), String> {
    // Before the open, so a test can move or replace the bundle and fail this
    // attempt even when another worker has already copied a different file.
    hooks.note_file(path)?;
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
    seal_file(&file, opened.permissions().mode())?;
    drop(input);
    let queued = SyncQueued::arm(syncs);
    queue.push(SealedFile {
        file,
        bytes: planned_bytes,
    })?;
    queued.queued();
    #[cfg(test)]
    note_file(cloned, opened.len());
    Ok(())
}

/// Mode and extended attributes for one surveyed directory.
///
/// The descriptor is the directory itself: `O_NOFOLLOW` refuses a symlink
/// substituted after the survey match, and `fchmod` does not follow. Attribute
/// removal is [`normalize_extended_attributes`], the same call a sealed file
/// uses, so a cloned directory cannot publish a bundle quarantine or any
/// other removable attribute the file path would have dropped.
fn seal_directory(path: &Path) -> Result<(), String> {
    let directory = phase!(
        Phase::Open,
        OpenOptions::new()
            .read(true)
            .custom_flags(libc::O_DIRECTORY | libc::O_NOFOLLOW)
            .open(path)
            .map_err(|error| error.to_string())
    )?;
    phase!(
        Phase::Permissions,
        directory
            .set_permissions(Permissions::from_mode(0o700))
            .map_err(|error| error.to_string())
    )?;
    phase!(Phase::Xattr, normalize_extended_attributes(&directory))
}

fn seal_file(file: &fs::File, source_mode: u32) -> Result<(), String> {
    phase!(
        Phase::Permissions,
        file.set_permissions(Permissions::from_mode(0o600 | (source_mode & 0o111)))
            .map_err(|error| error.to_string())
    )?;
    phase!(Phase::Xattr, normalize_extended_attributes(file))
}

/// Permissions, extended attributes, and `sync_all`, as one step.
///
/// A copy seals the file on the copying thread and runs `sync_all` on the
/// pool. This is those same two steps for the clone test, which does not use
/// the pool. It is test-only: the shipping build has no other caller, and a
/// function only tests call is dead code in the build `-D warnings` checks.
#[cfg(test)]
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
            // and directories created by both copy paths. It cannot carry
            // bundle-supplied policy.
            continue;
        }
        if error.raw_os_error() != Some(libc::ENOATTR) {
            return Err(error.to_string());
        }
    }
    let remaining = extended_attribute_names(file.as_raw_fd())?;
    if remaining.iter().any(|name| name != PROVENANCE) {
        return Err("Staged runtime entry retains extended attributes".into());
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

/// `CLONE_NOFOLLOW` from `<sys/clonefile.h>`. The libc crate does not export it.
const CLONE_NOFOLLOW: u32 = 0x0001;

/// Clone the source directory onto `destination`, which must not exist.
///
/// `copyfile(COPYFILE_RECURSIVE)` still clonefiles each file and creates each
/// directory, and it rejects `COPYFILE_CLONE_FORCE`. One `clonefile` of the
/// directory is the call that is not a per-file metadata transaction. The
/// kernel clones each item; this process then rewrites modes and extended
/// attributes, which a faithful recursive copy would have kept. The caller
/// matches that clone against the survey before any of those writes.
/// `ENOTSUP` and `EXDEV` return `Ok(false)` so the caller can copy one file at
/// a time. The syscall is all-or-nothing: a failure leaves no destination
/// tree.
fn clone_directory(source: &Path, destination: &Path) -> Result<bool, String> {
    let source = CString::new(utf8(source)?).map_err(|error| error.to_string())?;
    let destination = CString::new(utf8(destination)?).map_err(|error| error.to_string())?;
    if unsafe { libc::clonefile(source.as_ptr(), destination.as_ptr(), CLONE_NOFOLLOW) } == 0 {
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
