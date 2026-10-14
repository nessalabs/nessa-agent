use super::*;

#[test]
fn a_view_cursor_is_the_published_incarnation_bound_and_a_u64_position() {
    use crate::product::generated::ConversationViewCursor;
    let cursor = |incarnation: &str, position: &str| ConversationViewCursor {
        incarnation: incarnation.to_owned(),
        position: position.to_owned(),
    };
    assert!(conforming_view_cursor(&cursor("inc", "0")));
    assert!(conforming_view_cursor(&cursor(
        &"é".repeat(128),
        "18446744073709551615"
    )));
    assert!(!conforming_view_cursor(&cursor("", "1")));
    assert!(!conforming_view_cursor(&cursor(&"a".repeat(129), "1")));
    assert!(!conforming_view_cursor(&cursor("inc", "abc")));
    assert!(!conforming_view_cursor(&cursor("inc", "00")));
    assert!(!conforming_view_cursor(&cursor(
        "inc",
        "99999999999999999999"
    )));
}

#[test]
fn a_resume_floor_is_numeric_and_stops_at_another_incarnation() {
    use crate::product::generated::ConversationViewCursor;
    let cursor = |incarnation: &str, position: &str| ConversationViewCursor {
        incarnation: incarnation.to_owned(),
        position: position.to_owned(),
    };
    let floor = cursor("inc", "10");
    assert!(view_cursor_behind(&cursor("inc", "9"), &floor));
    assert!(!view_cursor_behind(&cursor("inc", "10"), &floor));
    assert!(!view_cursor_behind(&cursor("inc", "11"), &floor));
    assert!(!view_cursor_behind(&cursor("next", "1"), &floor));
}

#[test]
fn passive_writer_retains_bounded_capacity_across_large_chunk_and_suffix() {
    let mut writer = CappedWriter::new(MAX_RECORD_RESPONSE_BYTES);
    writer
        .write_all(&vec![b'x'; MAX_RECORD_RESPONSE_BYTES * 3 / 4])
        .unwrap();
    assert!(writer.bytes.capacity() <= MAX_RECORD_RESPONSE_BYTES);
    writer.write_all(b"}\n").unwrap();
    assert!(writer.bytes.capacity() <= MAX_RECORD_RESPONSE_BYTES);
    writer
        .write_all(&vec![b'x'; MAX_RECORD_RESPONSE_BYTES / 4 - 2])
        .unwrap();
    assert_eq!(writer.bytes.len(), MAX_RECORD_RESPONSE_BYTES);
    assert_eq!(writer.bytes.capacity(), MAX_RECORD_RESPONSE_BYTES);
    assert!(writer.write_all(b"x").is_err());
    assert_eq!(writer.bytes.capacity(), MAX_RECORD_RESPONSE_BYTES);
}
