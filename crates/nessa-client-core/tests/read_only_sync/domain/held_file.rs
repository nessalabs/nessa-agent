//! A held file's ranges: full ranges, a short last one, none past the end,
//! and an answer held to exactly the range asked.
use super::{HeldFile, SizeOutOfRange, HELD_FILE_RANGE_BYTES, MAX_HELD_FILE_BYTES};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};

fn file(size: u64) -> HeldFile {
    HeldFile::new(
        ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
        Sha256Digest::from_bytes([7; 32]),
        MediaType::parse("text/plain").unwrap(),
        size,
    )
    .unwrap()
}

#[test]
fn ranges_are_full_until_the_last_and_none_past_the_end() {
    let file = file(2 * HELD_FILE_RANGE_BYTES + 5);
    let first = file.range_at(0).unwrap();
    assert_eq!((first.offset(), first.length()), (0, HELD_FILE_RANGE_BYTES));
    let last = file.range_at(2 * HELD_FILE_RANGE_BYTES).unwrap();
    assert_eq!(last.length(), 5);
    assert_eq!(last.max_bytes(), HELD_FILE_RANGE_BYTES);
    assert_eq!(file.range_at(file.size()), None);
    assert_eq!(file.range_at(file.size() + 1), None);
    assert_eq!(file.range_at(u64::MAX), None);
    let one = self::file(1).range_at(0).unwrap();
    assert_eq!(one.length(), 1);
}

#[test]
fn an_answer_is_its_range_only_with_the_offset_echoed_and_the_exact_length() {
    let range = file(10).range_at(4).unwrap();
    assert!(range.answered_by(4, &[0; 6]));
    assert!(!range.answered_by(0, &[0; 6]), "another offset");
    assert!(!range.answered_by(4, &[0; 5]), "short");
    assert!(!range.answered_by(4, &[0; 7]), "long");
    assert!(!range.answered_by(4, &[]), "empty");
}

#[test]
fn a_held_file_has_one_byte_to_the_largest_a_read_names() {
    let sized = |size| {
        HeldFile::new(
            ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap(),
            Sha256Digest::from_bytes([7; 32]),
            MediaType::parse("text/plain").unwrap(),
            size,
        )
        .map(|file| file.size())
    };
    assert_eq!(sized(1), Ok(1));
    assert_eq!(sized(MAX_HELD_FILE_BYTES), Ok(MAX_HELD_FILE_BYTES));
    assert_eq!(sized(0), Err(SizeOutOfRange));
    assert_eq!(sized(MAX_HELD_FILE_BYTES + 1), Err(SizeOutOfRange));
}

#[test]
fn the_last_range_is_the_one_the_file_ends_in() {
    let file = self::file(2 * HELD_FILE_RANGE_BYTES + 5);
    assert_eq!(
        Some(file.last_range()),
        file.range_at(2 * HELD_FILE_RANGE_BYTES)
    );
    let whole = self::file(2 * HELD_FILE_RANGE_BYTES);
    assert_eq!(
        Some(whole.last_range()),
        whole.range_at(HELD_FILE_RANGE_BYTES)
    );
    assert_eq!(Some(self::file(1).last_range()), self::file(1).range_at(0));
}
