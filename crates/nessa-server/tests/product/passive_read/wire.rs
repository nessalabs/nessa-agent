use super::*;
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
