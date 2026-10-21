//! Fetching one held file: its ranges in order from where the kept part ends,
//! each answer held to the range asked, then the part as it is on disk
//! checked against the file's size and SHA-256 before it is kept under the
//! output's name.
//!
//! ```text
//! output already there --a regular file of the size and digest--> done
//!                      --anything else (not opened if not a regular
//!                        file of the size, links never followed)--> refused
//!   either way: a part no fetch holds is removed; nothing is asked
//! part (held exclusively, named for the whole file: conversation, digest,
//!   media type, size) --length--> range at that length
//!   --complete--> the last range asked again all the same: a part never
//!                 stands in for the gateway saying the file is held now
//!   --> source.read --busy--> pause; a wait that has ended asks nothing more
//!                   --answer is the range--> append, next range
//!                   --anything else--> fail; the part stays for a resume
//! all ranges --> re-read the part from disk: size and digest
//!   --match--> keep (sync, then link at the output's name only if nothing
//!              is there). The link is the publication. Removing the part's
//!              name or syncing the directory can still fail after it, and
//!              that failure reports the output as published.
//!   --differ--> discard the part (or empty it), fail; nothing counts as reached
//! ```
//! Arrows are calls in order. The source, the pause and the files are ports
//! composition supplies; nothing here reads a clock or a file itself.
use super::{GatewayError, HeldFileRefusal};
use crate::read_only_sync::domain::{HeldFile, HeldRange};
use nessa_protocol::product_contract::generated::ConversationErrorCode;
use sha2::{Digest, Sha256};
use std::time::Duration;

/// How long to wait before asking for the same range again after the
/// gateway answered `temporarily_unavailable` (its read slots were taken):
/// one wait per retry, so a range is asked at most once more than this has
/// entries. A range that is answered starts the count again for the next.
pub(crate) const BUSY_RETRY_DELAYS_MS: [u64; 3] = [250, 500, 1000];

/// One range as the gateway answered it, decoded but not yet believed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct RangeAnswer {
    pub(crate) offset: u64,
    pub(crate) bytes: Vec<u8>,
}

/// Where ranges come from: one `artifact.read` per call.
pub(crate) trait HeldFileSource {
    fn read(&mut self, file: &HeldFile, range: HeldRange) -> Result<RangeAnswer, GatewayError>;
}

/// Waits between retries of a busy range. Callers inject the wait so a test
/// can refuse to sleep. `ended` is checked by the production wait on each
/// stop slice, and by the fetch before the range is asked again.
pub trait Pause: Send + Sync {
    /// Wait `delay` before the same range is asked again. Return early when
    /// `ended` is true. A no-op pause records `delay` and ignores `ended`;
    /// the fetch still asks nothing more once `ended` is true.
    fn pause(&self, delay: Duration, ended: &dyn Fn() -> bool);
}

/// The step of the files' lifecycle that failed.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FileStep {
    Open,
    /// Another fetch of the same file holds its part.
    Locked,
    Read,
    Restart,
    Write,
    Sync,
    /// The part could not be linked at the output's name for a reason other
    /// than something being there; a file system without hard links fails
    /// here every time.
    Link,
    /// The directory that holds the output could not be synced.
    SyncDirectory,
    Remove,
    /// The output's name has no room for a part beside it: it names no file,
    /// or the part's name (the output's file name and 70 more bytes) would
    /// pass the 255 a file name may have. Refused before anything is dialled.
    Name,
    /// The part's name moved to another file while it was being held, and
    /// again when it was held once more.
    Changed,
}

/// What is at the output's name before anything is fetched.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum OutputFound {
    Absent,
    /// Something that cannot be the file, so it was not opened: not a
    /// regular file (a directory, or a link even to nothing, never followed),
    /// or not the named size.
    Other,
    /// A regular file of the named size, every byte handed to the visitor;
    /// how many there were.
    Read(u64),
}

/// Why the part could not be kept under the output's name.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum KeepRefusal {
    /// Something appeared at the output's name during the fetch; it is never
    /// replaced.
    OutputExists,
    /// A local step failed before this keep published the output.
    File(FileStep),
    /// Cancellation won before the hard link. This keep created no output.
    Stopped,
    /// The hard link exists. `step` is the cleanup that then failed.
    Published { step: FileStep },
}

/// The output and the part file it is assembled in. The part is named for
/// the whole file it holds (conversation, digest, media type and size), so a
/// part another file left, or the same bytes fetched for another
/// conversation, is never resumed; it is
/// held exclusively from `open_part` or the first `append` until `keep` or
/// `discard`, so two fetches never write one part.
pub(crate) trait FetchFiles {
    /// What is at the output's name; when it is a regular file of `size`
    /// bytes, every byte of it handed to `visit`.
    fn read_output(
        &mut self,
        size: u64,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<OutputFound, FileStep>;
    /// Remove a part this file's fetch left, when no fetch holds it. Best
    /// effort: a part another fetch holds, or one that cannot be removed,
    /// stays; it is named for this file, so no other file's fetch resumes it.
    fn remove_unheld_part(&mut self);
    /// Hold the part an earlier run left and return its length; 0 when there
    /// is none. Creates nothing.
    fn open_part(&mut self) -> Result<u64, FileStep>;
    /// Hand every byte the part holds on disk now to `visit`; its length.
    fn read_part(&mut self, visit: &mut dyn FnMut(&[u8])) -> Result<u64, FileStep>;
    /// Drop everything the part holds.
    fn restart(&mut self) -> Result<(), FileStep>;
    /// Append to the part, creating it, held, on the first append when
    /// there was none.
    fn append(&mut self, bytes: &[u8]) -> Result<(), FileStep>;
    /// Make the part durable, then give it the output's name only if
    /// nothing is there (`OutputExists` otherwise, whatever is there left as
    /// it was). Once that link exists, a failure to drop the part's name or
    /// to sync the directory is `Published`: the output is already there.
    fn keep(&mut self) -> Result<(), KeepRefusal>;
    /// Remove the part: what it holds is not the file.
    fn discard(&mut self) -> Result<(), FileStep>;
}

/// What a fetch did, counted from where it began.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub(crate) struct FetchProgress {
    /// Bytes already kept by an earlier run when this one began.
    pub(crate) resumed_from: u64,
    /// Bytes of the file kept when the fetch ended. 0 after an integrity
    /// failure: what the part held was not the file, so none of it counts.
    pub(crate) reached: u64,
    /// Ranges answered by this run: each one kept, or, for a part that was
    /// already complete, its last range asked again and not kept.
    pub(crate) ranges: u64,
    /// Busy answers asked again.
    pub(crate) busy_retries: u64,
}

/// What became of a part found not to be the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum PartCleanup {
    Removed,
    /// It could not be removed (`remove`), so it was emptied instead: the
    /// next run starts from nothing.
    Emptied {
        remove: FileStep,
    },
    /// Neither worked; the next run's check will refuse it again.
    Left {
        remove: FileStep,
        restart: FileStep,
    },
}

/// Why a fetch did not keep the file.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum FetchCause {
    /// The gateway refused a range, or no answer came. A busy range past
    /// [`BUSY_RETRY_DELAYS_MS`] is the gateway's `temporarily_unavailable`.
    Gateway(GatewayError),
    /// An answer was not the range asked: another offset or another length.
    WrongRange,
    /// The part on disk is not the file named: another size or SHA-256.
    Integrity { cleanup: PartCleanup },
    /// Something other than this file is at the output's name.
    OutputExists,
    /// The local files failed at this step, before the output was published.
    File(FileStep),
    /// The output was linked. `step` failed afterwards.
    Published { step: FileStep },
}
impl FetchCause {
    /// Whether the cause is on this device's files rather than the gateway
    /// or what it sent.
    pub(crate) fn local(self) -> bool {
        matches!(
            self,
            Self::OutputExists | Self::File(_) | Self::Published { .. }
        )
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct FetchFailure {
    pub(crate) cause: FetchCause,
    pub(crate) progress: FetchProgress,
}

/// Whether the output is already this file, asked before anything is
/// dialled. `Ok(None)`: there is no output, so fetch it. `Ok(Some)`: it is a
/// regular file of the named size and digest, so nothing is fetched.
/// Anything else at that name is refused, never replaced. When the output
/// decides the run either way, a part no fetch holds is removed.
pub(crate) fn kept_output(
    files: &mut dyn FetchFiles,
    file: &HeldFile,
) -> Result<Option<FetchProgress>, FetchFailure> {
    let fail = |cause| FetchFailure {
        cause,
        progress: FetchProgress::default(),
    };
    let mut hash = Sha256::new();
    let found = files
        .read_output(file.size(), &mut |bytes| hash.update(bytes))
        .map_err(|step| fail(FetchCause::File(step)))?;
    let decided = match found {
        OutputFound::Absent => return Ok(None),
        OutputFound::Read(length) if is_file(file, length, hash) => Ok(Some(FetchProgress {
            resumed_from: length,
            reached: length,
            ..FetchProgress::default()
        })),
        OutputFound::Read(_) | OutputFound::Other => Err(fail(FetchCause::OutputExists)),
    };
    files.remove_unheld_part();
    decided
}

/// Fetch `file` into its part and keep it under the output's name only once
/// the part, read back from disk, has the named size and digest. Resumes
/// from the part an earlier run left when it is shorter than the file; one
/// longer cannot be it and is started again; one already complete still
/// asks the gateway for its last range, so at least one range is always
/// asked. `ended` cuts a busy wait short and asks no later range. Every
/// failure but `Integrity` leaves the part for a later run to resume.
/// `Published` is past the link: the output is the file, and the part's
/// name may already be gone.
pub(crate) fn fetch_held_file(
    source: &mut dyn HeldFileSource,
    pause: &dyn Pause,
    files: &mut dyn FetchFiles,
    file: &HeldFile,
    ended: &dyn Fn() -> bool,
) -> Result<FetchProgress, FetchFailure> {
    let mut progress = FetchProgress::default();
    let fail = |cause, progress| Err(FetchFailure { cause, progress });
    let kept = match files.open_part() {
        Ok(kept) => kept,
        Err(step) => return fail(FetchCause::File(step), progress),
    };
    if kept > file.size() {
        if let Err(step) = files.restart() {
            return fail(FetchCause::File(step), progress);
        }
    } else {
        progress.resumed_from = kept;
        progress.reached = kept;
    }
    if progress.reached == file.size() {
        // A complete part is the bytes an earlier run was given, not this
        // run's authority to have them: the gateway is asked for the last
        // range again, and anything but that range keeps nothing.
        let range = file.last_range();
        let answer = match read_range(
            source,
            pause,
            file,
            range,
            &mut progress.busy_retries,
            ended,
        ) {
            Ok(answer) => answer,
            Err(error) => return fail(FetchCause::Gateway(error), progress),
        };
        if !range.answered_by(answer.offset, &answer.bytes) {
            return fail(FetchCause::WrongRange, progress);
        }
        progress.ranges += 1;
    }
    while let Some(range) = file.range_at(progress.reached) {
        let answer = match read_range(
            source,
            pause,
            file,
            range,
            &mut progress.busy_retries,
            ended,
        ) {
            Ok(answer) => answer,
            Err(error) => return fail(FetchCause::Gateway(error), progress),
        };
        if !range.answered_by(answer.offset, &answer.bytes) {
            return fail(FetchCause::WrongRange, progress);
        }
        if let Err(step) = files.append(&answer.bytes) {
            return fail(FetchCause::File(step), progress);
        }
        progress.reached += range.length();
        progress.ranges += 1;
    }
    // What is kept is what is on disk, so that is what is checked.
    let mut hash = Sha256::new();
    let length = match files.read_part(&mut |bytes| hash.update(bytes)) {
        Ok(length) => length,
        Err(step) => return fail(FetchCause::File(step), progress),
    };
    if !is_file(file, length, hash) {
        let cleanup = match files.discard() {
            Ok(()) => PartCleanup::Removed,
            Err(remove) => match files.restart() {
                Ok(()) => PartCleanup::Emptied { remove },
                Err(restart) => PartCleanup::Left { remove, restart },
            },
        };
        progress.reached = 0;
        return fail(FetchCause::Integrity { cleanup }, progress);
    }
    match files.keep() {
        Ok(()) => Ok(progress),
        Err(KeepRefusal::OutputExists) => fail(FetchCause::OutputExists, progress),
        Err(KeepRefusal::File(step)) => fail(FetchCause::File(step), progress),
        Err(KeepRefusal::Stopped) => fail(FetchCause::Gateway(GatewayError::Cancelled), progress),
        Err(KeepRefusal::Published { step }) => fail(FetchCause::Published { step }, progress),
    }
}

fn is_file(file: &HeldFile, length: u64, hash: Sha256) -> bool {
    let digest: [u8; 32] = hash.finalize().into();
    length == file.size() && &digest == file.digest().as_bytes()
}

/// One range, asked again after each of [`BUSY_RETRY_DELAYS_MS`] while the
/// gateway answers that it is busy. When `ended` is true the range is not
/// asked again.
fn read_range(
    source: &mut dyn HeldFileSource,
    pause: &dyn Pause,
    file: &HeldFile,
    range: HeldRange,
    retries: &mut u64,
    ended: &dyn Fn() -> bool,
) -> Result<RangeAnswer, GatewayError> {
    let mut delays = BUSY_RETRY_DELAYS_MS.iter();
    loop {
        if ended() {
            return Err(GatewayError::Cancelled);
        }
        match source.read(file, range) {
            Err(cause) if busy(cause) => {
                let Some(delay) = delays.next() else {
                    return Err(cause);
                };
                pause.pause(Duration::from_millis(*delay), ended);
                *retries += 1;
            }
            answer => return answer,
        }
    }
}

/// The gateway's answer that its read slots were taken: the same range may
/// be asked again.
pub(crate) fn busy(cause: GatewayError) -> bool {
    cause
        == GatewayError::HeldFile(HeldFileRefusal::Conversation(
            ConversationErrorCode::TemporarilyUnavailable,
        ))
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/application/held_file.rs"]
mod tests;
