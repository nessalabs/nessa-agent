//! Held-file fetch for the phone, over the existing range owner.
//!
//! A file already at the output is a local integrity match and is not asked
//! of the gateway. Gateway freshness is only the run whose ranges were
//! answered and whose part, read back from disk, matched the named digest.
//! Elapsed time is this fetch on the retained clock, not model generation.
//! Busy retries wait on the injected pause.

use super::{failure, ReadFailure, ReadStop, ReaderAccess, RetainedCache, Stopping};
use crate::read_only_sync::application::held_file::{
    fetch_held_file, kept_output, FetchCause, FetchFailure, FetchFiles, FileStep, HeldFileSource,
    KeepRefusal, OutputFound, PartCleanup, Pause,
};
use crate::read_only_sync::domain::{HeldFile, MAX_HELD_FILE_BYTES};
use crate::read_only_sync::infrastructure::gateway::GatewayConnection;
use crate::read_only_sync::infrastructure::held_file::DiskFiles;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use std::path::Path;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::Duration;

/// How a fetched file is fresh.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactFreshness {
    /// The output was already a regular file of this size and digest.
    /// No range was asked. This is not a new gateway hold.
    LocalMatch {
        /// Bytes in that file.
        bytes: u64,
    },
    /// This run's ranges were answered and the part on disk matched the digest.
    GatewayConfirmed {
        /// Ranges the gateway answered.
        ranges: u64,
        /// Bytes already in the part when this run began.
        resumed_from: u64,
    },
}

/// Which local file step failed. These are the existing fetch steps.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactFileStep {
    /// The output or part could not be opened.
    Open,
    /// Another fetch holds this part.
    Locked,
    /// The part could not be read back.
    Read,
    /// The part could not be emptied.
    Restart,
    /// A range could not be appended.
    Write,
    /// The part could not be synced.
    Sync,
    /// The part could not be linked at the output name.
    Link,
    /// The output's directory could not be synced.
    SyncDirectory,
    /// A part name could not be removed.
    Remove,
    /// The output name has no room for a part beside it.
    Name,
    /// The part's name moved to another file while it was held.
    Changed,
}

/// What became of a part that was not the named file.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactPartCleanup {
    /// The part was removed.
    Removed,
    /// Removal failed, and the part was emptied so the next run starts over.
    Emptied {
        /// The removal step that failed.
        remove: ArtifactFileStep,
    },
    /// Removal and emptying both failed. The part is still there.
    Left {
        /// The removal step that failed.
        remove: ArtifactFileStep,
        /// The emptying step that failed.
        restart: ArtifactFileStep,
    },
}

/// Why the file was not kept. `reached` is not freshness.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ArtifactFaultKind {
    /// The gateway refused a range, or the read stopped.
    Gateway(ReadFailure),
    /// An answer was not the range that was asked.
    WrongRange,
    /// The part on disk was not the named size and digest.
    /// `cleanup` is what then happened to that part.
    Integrity {
        /// Removal, emptying, or both having failed.
        cleanup: ArtifactPartCleanup,
    },
    /// Something other than this file is already at the output.
    OutputExists,
    /// A local file step failed.
    File(ArtifactFileStep),
}

/// A fetch that did not keep the file, with the part's progress.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactFault {
    kind: ArtifactFaultKind,
    reached: u64,
    resumed_from: u64,
}
impl ArtifactFault {
    /// A fault and the part's progress at that moment.
    /// `reached` is bytes of the named file still in the part.
    /// `resumed_from` is bytes already there when this run began.
    pub fn progress(kind: ArtifactFaultKind, reached: u64, resumed_from: u64) -> Self {
        Self {
            kind,
            reached,
            resumed_from,
        }
    }
    /// Why the file was not kept.
    pub fn kind(self) -> ArtifactFaultKind {
        self.kind
    }
    /// Bytes of the named file retained in the part. Zero after an integrity failure.
    pub fn reached(self) -> u64 {
        self.reached
    }
    /// Bytes already in the part when this run began.
    pub fn resumed_from(self) -> u64 {
        self.resumed_from
    }
}

/// Waits on the operating system's clock between retries of a busy range.
/// Session composition injects this, or a test double, into the fetch.
#[derive(Debug, Default)]
pub struct ThreadPause;
impl Pause for ThreadPause {
    fn pause(&self, delay: Duration) {
        std::thread::sleep(delay);
    }
}

/// One fetch's freshness or fault, and how long it took on the caller's clock.
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub struct ArtifactFetch {
    outcome: Result<ArtifactFreshness, ArtifactFault>,
    elapsed_ms: u64,
}
impl ArtifactFetch {
    /// A decision made before any gateway time was spent.
    pub fn decided(outcome: Result<ArtifactFreshness, ArtifactFault>, elapsed_ms: u64) -> Self {
        Self {
            outcome,
            elapsed_ms,
        }
    }
    /// Freshness when the file was kept. A fault is not fresh.
    pub fn outcome(self) -> Result<ArtifactFreshness, ArtifactFault> {
        self.outcome
    }
    /// Monotonic milliseconds spent in this fetch. Not model-generation latency.
    pub fn elapsed_ms(self) -> u64 {
        self.elapsed_ms
    }
}

struct StoppedSource<S> {
    source: S,
    stop: Arc<ReadStop>,
}
impl<S: HeldFileSource> HeldFileSource for StoppedSource<S> {
    fn read(
        &mut self,
        file: &HeldFile,
        range: crate::read_only_sync::domain::HeldRange,
    ) -> Result<
        crate::read_only_sync::application::held_file::RangeAnswer,
        crate::read_only_sync::application::GatewayError,
    > {
        if self.stop.is_stopped() {
            return Err(crate::read_only_sync::application::GatewayError::Cancelled);
        }
        self.source.read(file, range)
    }
}

fn named(
    conversation: &ConversationId,
    digest: &str,
    media_type: &str,
    size: u64,
) -> Result<HeldFile, ReadFailure> {
    let digest = Sha256Digest::parse(digest).map_err(|_| ReadFailure::Protocol)?;
    let media_type = MediaType::parse(media_type).map_err(|_| ReadFailure::Protocol)?;
    HeldFile::new(conversation.clone(), digest, media_type, size).map_err(|_| ReadFailure::Protocol)
}

fn part_cleanup(cleanup: PartCleanup) -> ArtifactPartCleanup {
    match cleanup {
        PartCleanup::Removed => ArtifactPartCleanup::Removed,
        PartCleanup::Emptied { remove } => ArtifactPartCleanup::Emptied {
            remove: file_step(remove),
        },
        PartCleanup::Left { remove, restart } => ArtifactPartCleanup::Left {
            remove: file_step(remove),
            restart: file_step(restart),
        },
    }
}

fn file_step(step: FileStep) -> ArtifactFileStep {
    match step {
        FileStep::Open => ArtifactFileStep::Open,
        FileStep::Locked => ArtifactFileStep::Locked,
        FileStep::Read => ArtifactFileStep::Read,
        FileStep::Restart => ArtifactFileStep::Restart,
        FileStep::Write => ArtifactFileStep::Write,
        FileStep::Sync => ArtifactFileStep::Sync,
        FileStep::Link => ArtifactFileStep::Link,
        FileStep::SyncDirectory => ArtifactFileStep::SyncDirectory,
        FileStep::Remove => ArtifactFileStep::Remove,
        FileStep::Name => ArtifactFileStep::Name,
        FileStep::Changed => ArtifactFileStep::Changed,
    }
}

fn fault_from(fetch_failure: FetchFailure) -> ArtifactFault {
    let kind = match fetch_failure.cause {
        FetchCause::Gateway(error) => ArtifactFaultKind::Gateway(failure(error)),
        FetchCause::WrongRange => ArtifactFaultKind::WrongRange,
        FetchCause::Integrity { cleanup } => ArtifactFaultKind::Integrity {
            cleanup: part_cleanup(cleanup),
        },
        FetchCause::OutputExists => ArtifactFaultKind::OutputExists,
        FetchCause::File(step) => ArtifactFaultKind::File(file_step(step)),
    };
    ArtifactFault {
        kind,
        reached: fetch_failure.progress.reached,
        resumed_from: fetch_failure.progress.resumed_from,
    }
}

/// The held file a phone names, and the output path it should land in.
/// Constructing one reads nothing and dials nothing.
pub struct ArtifactRequest<'a> {
    conversation: &'a ConversationId,
    digest: &'a str,
    media_type: &'a str,
    size: u64,
    output: &'a Path,
}
impl<'a> ArtifactRequest<'a> {
    /// Name the file and where to keep it.
    pub fn new(
        conversation: &'a ConversationId,
        digest: &'a str,
        media_type: &'a str,
        size: u64,
        output: &'a Path,
    ) -> Self {
        Self {
            conversation,
            digest,
            media_type,
            size,
            output,
        }
    }
}

/// Whether `digest`, `media_type`, and `size` name a held file.
/// A malformed digest or media type, a size of zero, or a size above the
/// held-file maximum does not. This reads nothing.
pub fn artifact_name_accepted(digest: &str, media_type: &str, size: u64) -> bool {
    Sha256Digest::parse(digest).is_ok()
        && MediaType::parse(media_type).is_ok()
        && std::num::NonZeroU64::new(size).is_some_and(|value| value.get() <= MAX_HELD_FILE_BYTES)
}

/// The output's existing bytes, before any dial. `Ok(None)` means the file
/// still has to be fetched. `Err` is a local refusal, also before any dial.
pub fn local_held_file(
    request: &ArtifactRequest<'_>,
) -> Result<Option<Result<ArtifactFreshness, ArtifactFault>>, ReadFailure> {
    let file = named(
        request.conversation,
        request.digest,
        request.media_type,
        request.size,
    )?;
    let mut files = match DiskFiles::new(request.output, &file) {
        Ok(files) => files,
        Err(step) => {
            return Ok(Some(Err(ArtifactFault {
                kind: ArtifactFaultKind::File(file_step(step)),
                reached: 0,
                resumed_from: 0,
            })))
        }
    };
    match kept_output(&mut files, &file) {
        Ok(None) => Ok(None),
        Ok(Some(progress)) => Ok(Some(Ok(ArtifactFreshness::LocalMatch {
            bytes: progress.reached,
        }))),
        Err(failure) => Ok(Some(Err(fault_from(failure)))),
    }
}

/// A local inspection published only while `stop` stayed clear. A stop that
/// is already set, or that arrives while the file is read, is `Stopped`
/// and is not a local match.
pub fn local_held_outcome(
    stop: &ReadStop,
    inspect: impl FnOnce() -> Result<Option<Result<ArtifactFreshness, ArtifactFault>>, ReadFailure>,
) -> Result<Option<Result<ArtifactFreshness, ArtifactFault>>, ReadFailure> {
    if stop.is_stopped() {
        return Err(ReadFailure::Stopped);
    }
    let outcome = inspect();
    if stop.is_stopped() {
        return Err(ReadFailure::Stopped);
    }
    outcome
}

/// Files whose `keep` refuses once `stop` is set, so a cancellation during
/// the part's re-read does not link the output.
struct StopGate<'a> {
    files: DiskFiles,
    stop: &'a ReadStop,
    /// `keep` saw the stop and did not link. A failure settled before that
    /// stays the failure, even if `stop` is set afterwards.
    keep_saw_stop: bool,
}
impl FetchFiles for StopGate<'_> {
    fn read_output(
        &mut self,
        size: u64,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<OutputFound, FileStep> {
        self.files.read_output(size, visit)
    }
    fn remove_unheld_part(&mut self) {
        self.files.remove_unheld_part();
    }
    fn open_part(&mut self) -> Result<u64, FileStep> {
        self.files.open_part()
    }
    fn read_part(&mut self, visit: &mut dyn FnMut(&[u8])) -> Result<u64, FileStep> {
        self.files.read_part(visit)
    }
    fn restart(&mut self) -> Result<(), FileStep> {
        self.files.restart()
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), FileStep> {
        self.files.append(bytes)
    }
    fn keep(&mut self) -> Result<(), KeepRefusal> {
        if self.stop.is_stopped() {
            self.keep_saw_stop = true;
            // The step is not published. Only this refusal becomes
            // `ReadFailure::Stopped`. A failure already returned does not.
            return Err(KeepRefusal::File(FileStep::Link));
        }
        self.files.keep()
    }
    fn discard(&mut self) -> Result<(), FileStep> {
        self.files.discard()
    }
}

fn stopped_fault(fault: ArtifactFault) -> ArtifactFault {
    ArtifactFault::progress(
        ArtifactFaultKind::Gateway(ReadFailure::Stopped),
        fault.reached(),
        fault.resumed_from(),
    )
}

/// `keep_saw_stop` is the only cancellation that replaces a settled failure.
/// An integrity or file failure returned before `keep` keeps its cause.
fn publish_fetch(failure: FetchFailure, keep_saw_stop: bool) -> ArtifactFault {
    let fault = fault_from(failure);
    if keep_saw_stop {
        stopped_fault(fault)
    } else {
        fault
    }
}

/// Ask the gateway and keep the file only when the part matches. The caller
/// has already applied [`local_held_file`]. A stop set while the part is
/// re-read refuses `keep` and is `Stopped`, not a gateway confirmation.
pub(crate) fn confirmed_held_file(
    source: &mut dyn HeldFileSource,
    pause: &dyn Pause,
    request: &ArtifactRequest<'_>,
    stop: &ReadStop,
) -> Result<Result<ArtifactFreshness, ArtifactFault>, ReadFailure> {
    let file = named(
        request.conversation,
        request.digest,
        request.media_type,
        request.size,
    )?;
    let mut files = match DiskFiles::new(request.output, &file) {
        Ok(files) => StopGate {
            files,
            stop,
            keep_saw_stop: false,
        },
        Err(step) => {
            return Ok(Err(ArtifactFault {
                kind: ArtifactFaultKind::File(file_step(step)),
                reached: 0,
                resumed_from: 0,
            }))
        }
    };
    if stop.is_stopped() {
        return Ok(Err(stopped_fault(ArtifactFault::progress(
            ArtifactFaultKind::Gateway(ReadFailure::Stopped),
            0,
            0,
        ))));
    }
    match fetch_held_file(source, pause, &mut files, &file) {
        Ok(progress) => Ok(Ok(ArtifactFreshness::GatewayConfirmed {
            ranges: progress.ranges,
            resumed_from: progress.resumed_from,
        })),
        Err(failure) => Ok(Err(publish_fetch(failure, files.keep_saw_stop))),
    }
}

impl RetainedCache {
    /// Fetch one held file for `access` into `output`. A local match returns
    /// before a connection. Gateway confirmation uses the existing range fetch.
    /// `elapsed_ms` is this call on the cache clock.
    ///
    /// A socket registered on `stop` is released on every exit after the dial,
    /// including a handshake that fails. A budget that cancels a range is
    /// `Unreachable`. `ReadStop::stop` stays `Stopped`, including a local
    /// match that was cancelled while the output was read. A relay that says
    /// the gateway is offline, when the direct dial did not connect, is `Away`.
    pub fn fetch_artifact(
        &mut self,
        access: ReaderAccess<'_>,
        stop: &Arc<ReadStop>,
        budget: Duration,
        request: &ArtifactRequest<'_>,
        pause: &dyn Pause,
    ) -> Result<ArtifactFetch, ReadFailure> {
        let started = self.clock.elapsed_ms();
        let elapsed = |cache: &Self| cache.clock.elapsed_ms().saturating_sub(started);
        if let Some(outcome) = local_held_outcome(stop, || local_held_file(request))? {
            return Ok(ArtifactFetch {
                outcome,
                elapsed_ms: elapsed(self),
            });
        }
        let ending = Arc::new(Stopping {
            stop: stop.clone(),
            clock: self.clock.clone(),
            until: started.saturating_add(u64::try_from(budget.as_millis()).unwrap_or(u64::MAX)),
        });
        let offline = Arc::new(AtomicBool::new(false));
        let session = match self.open_session(&access, &ending, &offline) {
            Ok(session) => session,
            Err(failure) => {
                stop.release();
                return Err(dial_failure(
                    failure,
                    stop,
                    &ending,
                    offline.load(Ordering::SeqCst),
                ));
            }
        };
        let fetched = {
            let mut source = StoppedSource {
                source: GatewayConnection::new(session),
                stop: stop.clone(),
            };
            confirmed_held_file(&mut source, pause, request, stop)
        };
        stop.release();
        let outcome = match fetched? {
            Ok(fresh) => Ok(fresh),
            Err(fault) => Err(classify_artifact_fault(fault, stop, &ending)),
        };
        Ok(ArtifactFetch {
            outcome,
            elapsed_ms: elapsed(self),
        })
    }
}

/// A dial that never reached the gateway. A caller stop stays a stop. A
/// relay `Offline` is `Away`. A spent budget is `Unreachable`.
fn dial_failure(
    failure: ReadFailure,
    stop: &ReadStop,
    ending: &Stopping,
    offline: bool,
) -> ReadFailure {
    if stop.is_stopped() {
        ReadFailure::Stopped
    } else if offline && matches!(failure, ReadFailure::Unreachable) {
        ReadFailure::Away
    } else if ending.spent() && matches!(failure, ReadFailure::Stopped | ReadFailure::Unreachable) {
        ReadFailure::Unreachable
    } else {
        failure
    }
}

/// A range the budget cancelled is not a caller stop. `failure` maps that
/// cancellation to `Stopped` before this sees it.
fn classify_artifact_fault(
    fault: ArtifactFault,
    stop: &ReadStop,
    ending: &Stopping,
) -> ArtifactFault {
    let ArtifactFaultKind::Gateway(failure) = fault.kind() else {
        return fault;
    };
    let failure = dial_failure(failure, stop, ending, false);
    ArtifactFault::progress(
        ArtifactFaultKind::Gateway(failure),
        fault.reached(),
        fault.resumed_from(),
    )
}

#[cfg(test)]
#[path = "../../tests/retained/artifact.rs"]
mod tests;
