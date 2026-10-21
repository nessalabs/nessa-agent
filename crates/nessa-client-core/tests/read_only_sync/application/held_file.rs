//! Fetching a held file against a scripted source, a recording pause and an
//! in-memory part file: assembly, resume, verification, busy retries and
//! refusals, with no clock and no file system.
use super::*;
use crate::read_only_sync::domain::HELD_FILE_RANGE_BYTES;
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use std::collections::VecDeque;
use std::sync::Mutex;

const RANGE: usize = HELD_FILE_RANGE_BYTES as usize;

fn content(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}
fn named(bytes: &[u8]) -> HeldFile {
    named_as(bytes, Sha256::digest(bytes).into())
}
fn named_as(bytes: &[u8], digest: [u8; 32]) -> HeldFile {
    HeldFile::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        Sha256Digest::from_bytes(digest),
        MediaType::parse("application/octet-stream").unwrap(),
        bytes.len() as u64,
    )
    .unwrap()
}

/// What the gateway answers, in order. With no script left, it answers the
/// range asked from `bytes`.
struct Source {
    bytes: Vec<u8>,
    script: VecDeque<Result<RangeAnswer, GatewayError>>,
    asked: Vec<(u64, u64)>,
}
impl Source {
    fn serving(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            script: VecDeque::new(),
            asked: vec![],
        }
    }
    fn then(mut self, answer: Result<RangeAnswer, GatewayError>) -> Self {
        self.script.push_back(answer);
        self
    }
}
impl HeldFileSource for Source {
    fn read(&mut self, file: &HeldFile, range: HeldRange) -> Result<RangeAnswer, GatewayError> {
        assert_eq!(file.size(), self.bytes.len() as u64);
        self.asked.push((range.offset(), range.max_bytes()));
        if let Some(answer) = self.script.pop_front() {
            return answer;
        }
        let start = range.offset() as usize;
        let end = (start + range.max_bytes() as usize).min(self.bytes.len());
        Ok(RangeAnswer {
            offset: range.offset(),
            bytes: self.bytes[start..end].to_vec(),
        })
    }
}

#[derive(Default)]
struct Pauses(Mutex<Vec<Duration>>);
impl Pause for Pauses {
    fn pause(&self, delay: Duration, _: &dyn Fn() -> bool) {
        self.0.lock().expect("pause record").push(delay);
    }
}
impl Pauses {
    fn millis(&self) -> Vec<u64> {
        self.0
            .lock()
            .expect("pause record")
            .iter()
            .map(|delay| delay.as_millis() as u64)
            .collect()
    }
}

/// The output and part, in memory, and where their lifecycle ended.
#[derive(Default)]
struct Part {
    output: Option<Vec<u8>>,
    /// Something at the output's name that is not a regular file of the size.
    output_is_other: bool,
    /// Times an unheld part was asked to be removed.
    unheld_removals: usize,
    bytes: Vec<u8>,
    kept: Option<Vec<u8>>,
    discarded: bool,
    restarted: bool,
    /// Steps that fail.
    fail: Vec<FileStep>,
    /// Bytes another writer adds to the part after each of ours.
    foreign: Vec<u8>,
    output_appears: bool,
}
impl Part {
    fn holding(bytes: &[u8]) -> Self {
        Self {
            bytes: bytes.to_vec(),
            ..Self::default()
        }
    }
    fn step(&self, step: FileStep) -> Result<(), FileStep> {
        if self.fail.contains(&step) {
            return Err(step);
        }
        Ok(())
    }
}
fn pieces(bytes: &[u8], visit: &mut dyn FnMut(&[u8])) -> u64 {
    // In pieces, as a file is read.
    for piece in bytes.chunks(1000) {
        visit(piece);
    }
    bytes.len() as u64
}
impl FetchFiles for Part {
    fn read_output(
        &mut self,
        _: u64,
        visit: &mut dyn FnMut(&[u8]),
    ) -> Result<OutputFound, FileStep> {
        self.step(FileStep::Open)?;
        if self.output_is_other {
            return Ok(OutputFound::Other);
        }
        Ok(match self.output.as_deref() {
            None => OutputFound::Absent,
            Some(bytes) => OutputFound::Read(pieces(bytes, visit)),
        })
    }
    fn remove_unheld_part(&mut self) {
        self.unheld_removals += 1;
    }
    fn open_part(&mut self) -> Result<u64, FileStep> {
        self.step(FileStep::Locked)?;
        Ok(self.bytes.len() as u64)
    }
    fn read_part(&mut self, visit: &mut dyn FnMut(&[u8])) -> Result<u64, FileStep> {
        self.step(FileStep::Read)?;
        Ok(pieces(&self.bytes, visit))
    }
    fn restart(&mut self) -> Result<(), FileStep> {
        self.step(FileStep::Restart)?;
        self.restarted = true;
        self.bytes.clear();
        Ok(())
    }
    fn append(&mut self, bytes: &[u8]) -> Result<(), FileStep> {
        self.step(FileStep::Write)?;
        self.bytes.extend_from_slice(bytes);
        self.bytes.extend_from_slice(&self.foreign);
        Ok(())
    }
    fn keep(&mut self) -> Result<(), KeepRefusal> {
        if self.output_appears {
            return Err(KeepRefusal::OutputExists);
        }
        self.step(FileStep::Link).map_err(KeepRefusal::File)?;
        self.kept = Some(std::mem::take(&mut self.bytes));
        Ok(())
    }
    fn discard(&mut self) -> Result<(), FileStep> {
        self.step(FileStep::Remove)?;
        self.discarded = true;
        self.bytes.clear();
        Ok(())
    }
}

fn busy_answer() -> Result<RangeAnswer, GatewayError> {
    Err(GatewayError::HeldFile(HeldFileRefusal::Conversation(
        ConversationErrorCode::TemporarilyUnavailable,
    )))
}

#[test]
fn ranges_are_asked_in_order_and_the_whole_is_kept_once_verified() {
    let bytes = content(2 * RANGE + 5);
    let file = named(&bytes);
    let mut source = Source::serving(&bytes);
    let pauses = Pauses::default();
    let mut part = Part::default();
    let progress = fetch_held_file(&mut source, &pauses, &mut part, &file, &|| false).unwrap();
    assert_eq!(
        source.asked,
        [
            (0, HELD_FILE_RANGE_BYTES),
            (HELD_FILE_RANGE_BYTES, HELD_FILE_RANGE_BYTES),
            (2 * HELD_FILE_RANGE_BYTES, HELD_FILE_RANGE_BYTES)
        ]
    );
    assert_eq!(
        progress,
        FetchProgress {
            resumed_from: 0,
            reached: bytes.len() as u64,
            ranges: 3,
            busy_retries: 0
        }
    );
    assert_eq!(part.kept.as_deref(), Some(&bytes[..]));
    assert!(pauses.millis().is_empty());
}

#[test]
fn a_kept_part_is_resumed_from_its_length_and_hashed_with_the_rest() {
    let bytes = content(2 * RANGE + 5);
    let file = named(&bytes);
    let mut source = Source::serving(&bytes);
    let mut part = Part::holding(&bytes[..RANGE + 10]);
    let progress =
        fetch_held_file(&mut source, &Pauses::default(), &mut part, &file, &|| false).unwrap();
    // What is left is less than one range: the last, short one.
    assert_eq!(source.asked, [(RANGE as u64 + 10, HELD_FILE_RANGE_BYTES)]);
    assert_eq!(progress.resumed_from, RANGE as u64 + 10);
    assert_eq!(progress.ranges, 1);
    assert_eq!(part.kept.as_deref(), Some(&bytes[..]));
}

/// Review P2 (PR #765): a complete part is never taken in place of the
/// gateway's answer. Its last range is asked again, and kept only when
/// answered as that range.
#[test]
fn a_complete_part_still_asks_the_gateway_for_its_last_range() {
    let bytes = content(2 * RANGE + 5);
    let mut source = Source::serving(&bytes);
    let mut part = Part::holding(&bytes);
    let progress = fetch_held_file(
        &mut source,
        &Pauses::default(),
        &mut part,
        &named(&bytes),
        &|| false,
    )
    .unwrap();
    assert_eq!(source.asked, [(2 * RANGE as u64, HELD_FILE_RANGE_BYTES)]);
    assert_eq!(
        (progress.resumed_from, progress.reached, progress.ranges),
        (bytes.len() as u64, bytes.len() as u64, 1)
    );
    assert_eq!(part.kept.as_deref(), Some(&bytes[..]));
}

#[test]
fn a_complete_part_the_gateway_refuses_or_misanswers_keeps_nothing() {
    let bytes = content(RANGE + 5);
    for (answer, cause) in [
        (
            Err(GatewayError::HeldFile(HeldFileRefusal::Conversation(
                ConversationErrorCode::AttachmentNotFound,
            ))),
            FetchCause::Gateway(GatewayError::HeldFile(HeldFileRefusal::Conversation(
                ConversationErrorCode::AttachmentNotFound,
            ))),
        ),
        (
            Err(GatewayError::HeldFile(HeldFileRefusal::Forbidden)),
            FetchCause::Gateway(GatewayError::HeldFile(HeldFileRefusal::Forbidden)),
        ),
        (
            Ok(RangeAnswer {
                offset: RANGE as u64,
                bytes: vec![0; 4],
            }),
            FetchCause::WrongRange,
        ),
    ] {
        let mut source = Source::serving(&bytes).then(answer);
        let mut part = Part::holding(&bytes);
        let failure = fetch_held_file(
            &mut source,
            &Pauses::default(),
            &mut part,
            &named(&bytes),
            &|| false,
        )
        .unwrap_err();
        assert_eq!(failure.cause, cause);
        assert_eq!(source.asked.len(), 1);
        assert_eq!(failure.progress.ranges, 0);
        assert_eq!(part.kept, None);
        // The part stays for a run the gateway answers.
        assert!(!part.discarded);
        assert_eq!(part.bytes, bytes);
    }
}

#[test]
fn a_part_longer_than_the_file_is_started_again() {
    let bytes = content(100);
    let mut source = Source::serving(&bytes);
    let mut part = Part::holding(&content(101));
    let progress = fetch_held_file(
        &mut source,
        &Pauses::default(),
        &mut part,
        &named(&bytes),
        &|| false,
    )
    .unwrap();
    assert!(part.restarted);
    assert_eq!(source.asked, [(0, HELD_FILE_RANGE_BYTES)]);
    assert_eq!(progress.resumed_from, 0);
    assert_eq!(part.kept.as_deref(), Some(&bytes[..]));
}

#[test]
fn bytes_that_are_not_the_named_digest_discard_the_part_and_keep_nothing() {
    let bytes = content(RANGE + 1);
    let file = named_as(&bytes, [9; 32]);
    let mut part = Part::default();
    let failure = fetch_held_file(
        &mut Source::serving(&bytes),
        &Pauses::default(),
        &mut part,
        &file,
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        failure.cause,
        FetchCause::Integrity {
            cleanup: PartCleanup::Removed
        }
    );
    // Nothing of the file is kept, so nothing counts as reached; the ranges
    // read are still counted.
    assert_eq!(failure.progress.reached, 0);
    assert_eq!(failure.progress.ranges, 2);
    assert!(part.discarded);
    assert_eq!(part.kept, None);
}

#[test]
fn a_resumed_part_of_other_bytes_fails_the_digest_and_is_discarded() {
    let bytes = content(RANGE + 1);
    let mut stale = bytes[..RANGE].to_vec();
    stale[0] ^= 1;
    let mut part = Part::holding(&stale);
    let failure = fetch_held_file(
        &mut Source::serving(&bytes),
        &Pauses::default(),
        &mut part,
        &named(&bytes),
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        failure.cause,
        FetchCause::Integrity {
            cleanup: PartCleanup::Removed
        }
    );
    assert!(part.discarded);
    assert_eq!(part.kept, None);
}

#[test]
fn a_part_that_cannot_be_removed_is_emptied_and_both_outcomes_are_typed() {
    let bytes = content(10);
    let mut part = Part {
        fail: vec![FileStep::Remove],
        ..Part::default()
    };
    let failure = fetch_held_file(
        &mut Source::serving(&bytes),
        &Pauses::default(),
        &mut part,
        &named_as(&bytes, [0; 32]),
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        failure.cause,
        FetchCause::Integrity {
            cleanup: PartCleanup::Emptied {
                remove: FileStep::Remove
            }
        }
    );
    assert!(part.restarted && part.bytes.is_empty());
    assert_eq!(part.kept, None);
    let mut part = Part {
        fail: vec![FileStep::Remove, FileStep::Restart],
        ..Part::default()
    };
    let failure = fetch_held_file(
        &mut Source::serving(&bytes),
        &Pauses::default(),
        &mut part,
        &named_as(&bytes, [0; 32]),
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        failure.cause,
        FetchCause::Integrity {
            cleanup: PartCleanup::Left {
                remove: FileStep::Remove,
                restart: FileStep::Restart
            }
        }
    );
    assert_eq!(part.kept, None);
}

/// Review P2-1: the check holds for what the part holds on disk, not for the
/// bytes this run sent it. Another writer's bytes in the part fail it.
#[test]
fn bytes_another_writer_added_to_the_part_fail_the_check_and_nothing_is_kept() {
    let bytes = content(RANGE + 3);
    let mut part = Part {
        foreign: vec![0xee],
        ..Part::default()
    };
    let failure = fetch_held_file(
        &mut Source::serving(&bytes),
        &Pauses::default(),
        &mut part,
        &named(&bytes),
        &|| false,
    )
    .unwrap_err();
    assert_eq!(
        failure.cause,
        FetchCause::Integrity {
            cleanup: PartCleanup::Removed
        }
    );
    assert!(part.discarded);
    assert_eq!(part.kept, None);
}

#[test]
fn an_output_that_appears_during_the_fetch_is_never_replaced() {
    let bytes = content(10);
    let mut part = Part {
        output_appears: true,
        ..Part::default()
    };
    let failure = fetch_held_file(
        &mut Source::serving(&bytes),
        &Pauses::default(),
        &mut part,
        &named(&bytes),
        &|| false,
    )
    .unwrap_err();
    assert_eq!(failure.cause, FetchCause::OutputExists);
    assert!(failure.cause.local());
    assert_eq!(part.kept, None);
}

#[test]
fn an_output_already_there_is_kept_only_when_it_is_the_file() {
    let bytes = content(RANGE + 3);
    let mut absent = Part::default();
    assert_eq!(kept_output(&mut absent, &named(&bytes)), Ok(None));
    // No output decides nothing, so a part is left for the fetch to resume.
    assert_eq!(absent.unheld_removals, 0);
    let mut same = Part {
        output: Some(bytes.clone()),
        ..Part::default()
    };
    assert_eq!(
        kept_output(&mut same, &named(&bytes)),
        Ok(Some(FetchProgress {
            resumed_from: bytes.len() as u64,
            reached: bytes.len() as u64,
            ranges: 0,
            busy_retries: 0
        }))
    );
    assert_eq!(same.unheld_removals, 1);
    let mut other_bytes = bytes.clone();
    other_bytes[5] ^= 1;
    for other in [other_bytes, bytes[..RANGE].to_vec(), vec![]] {
        let mut different = Part {
            output: Some(other),
            ..Part::default()
        };
        let failure = kept_output(&mut different, &named(&bytes)).unwrap_err();
        assert_eq!(failure.cause, FetchCause::OutputExists);
        assert_eq!(different.kept, None);
        assert_eq!(different.unheld_removals, 1);
    }
    let mut other = Part {
        output_is_other: true,
        ..Part::default()
    };
    assert_eq!(
        kept_output(&mut other, &named(&bytes)).unwrap_err().cause,
        FetchCause::OutputExists
    );
    assert_eq!(other.unheld_removals, 1);
    let mut unreadable = Part {
        fail: vec![FileStep::Open],
        ..Part::default()
    };
    assert_eq!(
        kept_output(&mut unreadable, &named(&bytes))
            .unwrap_err()
            .cause,
        FetchCause::File(FileStep::Open)
    );
}

#[test]
fn a_busy_range_is_asked_again_after_each_pause_then_read_on() {
    let bytes = content(RANGE + 3);
    let mut source = Source::serving(&bytes)
        .then(busy_answer())
        .then(busy_answer());
    let pauses = Pauses::default();
    let mut part = Part::default();
    let progress =
        fetch_held_file(&mut source, &pauses, &mut part, &named(&bytes), &|| false).unwrap();
    assert_eq!(pauses.millis(), [250, 500]);
    assert_eq!(progress.busy_retries, 2);
    assert_eq!(
        source.asked,
        [
            (0, HELD_FILE_RANGE_BYTES),
            (0, HELD_FILE_RANGE_BYTES),
            (0, HELD_FILE_RANGE_BYTES),
            (HELD_FILE_RANGE_BYTES, HELD_FILE_RANGE_BYTES)
        ]
    );
    assert_eq!(part.kept.as_deref(), Some(&bytes[..]));
}

#[test]
fn each_range_has_its_own_retries() {
    let bytes = content(RANGE + 3);
    let mut source = Source::serving(&bytes)
        .then(busy_answer())
        .then(busy_answer())
        .then(busy_answer());
    // The first range used all three retries; the second gets three again.
    source.script.push_back(Ok(RangeAnswer {
        offset: 0,
        bytes: bytes[..RANGE].to_vec(),
    }));
    for _ in 0..3 {
        source.script.push_back(busy_answer());
    }
    let pauses = Pauses::default();
    let mut part = Part::default();
    fetch_held_file(&mut source, &pauses, &mut part, &named(&bytes), &|| false).unwrap();
    assert_eq!(pauses.millis(), [250, 500, 1000, 250, 500, 1000]);
    assert_eq!(part.kept.as_deref(), Some(&bytes[..]));
}

#[test]
fn a_range_busy_past_the_bound_fails_typed_and_keeps_the_part() {
    let bytes = content(RANGE + 3);
    let mut source = Source::serving(&bytes).then(Ok(RangeAnswer {
        offset: 0,
        bytes: bytes[..RANGE].to_vec(),
    }));
    for _ in 0..=BUSY_RETRY_DELAYS_MS.len() {
        source.script.push_back(busy_answer());
    }
    let pauses = Pauses::default();
    let mut part = Part::default();
    let failure =
        fetch_held_file(&mut source, &pauses, &mut part, &named(&bytes), &|| false).unwrap_err();
    assert_eq!(
        failure.cause,
        FetchCause::Gateway(busy_answer().unwrap_err())
    );
    assert_eq!(pauses.millis(), BUSY_RETRY_DELAYS_MS);
    assert_eq!(source.asked.len(), 1 + BUSY_RETRY_DELAYS_MS.len() + 1);
    assert_eq!(failure.progress.reached, RANGE as u64);
    assert_eq!(failure.progress.busy_retries, 3);
    assert_eq!(part.bytes, bytes[..RANGE]);
    assert!(!part.discarded);
    assert_eq!(part.kept, None);
}

#[test]
fn refusals_other_than_busy_fail_at_once_and_keep_the_part() {
    for cause in [
        GatewayError::HeldFile(HeldFileRefusal::Conversation(
            ConversationErrorCode::AttachmentNotFound,
        )),
        GatewayError::HeldFile(HeldFileRefusal::Conversation(
            ConversationErrorCode::ConversationDeleted,
        )),
        GatewayError::HeldFile(HeldFileRefusal::Forbidden),
        GatewayError::Closed(None),
        GatewayError::Protocol,
    ] {
        let bytes = content(10);
        let mut source = Source::serving(&bytes).then(Err(cause));
        let pauses = Pauses::default();
        let mut part = Part::holding(&bytes[..4]);
        let failure = fetch_held_file(&mut source, &pauses, &mut part, &named(&bytes), &|| false)
            .unwrap_err();
        assert_eq!(failure.cause, FetchCause::Gateway(cause));
        assert_eq!(source.asked.len(), 1, "{cause:?}");
        assert!(pauses.millis().is_empty());
        assert_eq!(part.bytes, bytes[..4]);
        assert!(!part.discarded && part.kept.is_none());
    }
}

#[test]
fn an_answer_that_is_not_the_range_asked_is_kept_nowhere() {
    let bytes = content(RANGE + 3);
    for answer in [
        RangeAnswer {
            offset: 1,
            bytes: bytes[..RANGE].to_vec(),
        },
        RangeAnswer {
            offset: 0,
            bytes: bytes[..RANGE - 1].to_vec(),
        },
        RangeAnswer {
            offset: 0,
            bytes: bytes[..RANGE + 1].to_vec(),
        },
        RangeAnswer {
            offset: 0,
            bytes: vec![],
        },
    ] {
        let mut part = Part::default();
        let failure = fetch_held_file(
            &mut Source::serving(&bytes).then(Ok(answer)),
            &Pauses::default(),
            &mut part,
            &named(&bytes),
            &|| false,
        )
        .unwrap_err();
        assert_eq!(failure.cause, FetchCause::WrongRange);
        assert!(part.bytes.is_empty() && part.kept.is_none() && !part.discarded);
    }
}

#[test]
fn part_file_failures_are_typed_by_their_step() {
    let bytes = content(10);
    for step in [
        FileStep::Locked,
        FileStep::Read,
        FileStep::Write,
        FileStep::Link,
    ] {
        let mut part = Part {
            fail: vec![step],
            ..Part::default()
        };
        let failure = fetch_held_file(
            &mut Source::serving(&bytes),
            &Pauses::default(),
            &mut part,
            &named(&bytes),
            &|| false,
        )
        .unwrap_err();
        assert_eq!(failure.cause, FetchCause::File(step));
        assert_eq!(part.kept, None);
    }
    let mut part = Part {
        fail: vec![FileStep::Restart],
        ..Part::holding(&content(11))
    };
    let failure = fetch_held_file(
        &mut Source::serving(&bytes),
        &Pauses::default(),
        &mut part,
        &named(&bytes),
        &|| false,
    )
    .unwrap_err();
    assert_eq!(failure.cause, FetchCause::File(FileStep::Restart));
}

#[test]
fn a_busy_pause_that_has_ended_asks_no_further_range() {
    use std::cell::Cell;
    let bytes = content(10);
    let mut source = Source::serving(&bytes).then(busy_answer());
    let pauses = Pauses::default();
    let mut part = Part::default();
    let checks = Cell::new(0u32);
    let failure = fetch_held_file(&mut source, &pauses, &mut part, &named(&bytes), &|| {
        let seen = checks.get();
        checks.set(seen + 1);
        seen >= 1
    })
    .unwrap_err();
    assert_eq!(failure.cause, FetchCause::Gateway(GatewayError::Cancelled));
    assert_eq!(pauses.millis(), [250]);
    assert_eq!(source.asked.len(), 1);
    assert!(part.kept.is_none());
}
