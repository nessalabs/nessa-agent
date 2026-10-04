//! The one length-prefixed frame reader: bound, refusal and partial progress.
use super::{encode_frame, FrameReader, FrameTooLarge};

fn feed(reader: &mut FrameReader, mut bytes: &[u8], step: usize) -> Vec<Vec<u8>> {
    let mut frames = Vec::new();
    while !bytes.is_empty() {
        let buffer = reader.unfilled().unwrap();
        let count = buffer.len().min(step).min(bytes.len());
        buffer[..count].copy_from_slice(&bytes[..count]);
        bytes = &bytes[count..];
        if let Some(frame) = reader.filled(count).unwrap() {
            frames.push(frame);
        }
    }
    frames
}

/// Row PR9 (bound): a prefix announcing one byte over the bound is refused
/// before a body exists, and the refusal is kept; the exact bound and an empty
/// frame are accepted, one byte at a time as readily as whole.
#[test]
fn frame_reader_refuses_oversize_before_body_and_keeps_partial_progress() {
    let limit = 8;
    let mut reader = FrameReader::new(limit);
    let exact = encode_frame(limit, b"12345678").unwrap();
    let empty = encode_frame(limit, b"").unwrap();
    let stream = [exact.clone(), empty, exact].concat();
    assert_eq!(
        feed(&mut reader, &stream, 1),
        vec![b"12345678".to_vec(), Vec::new(), b"12345678".to_vec()]
    );
    assert_eq!(
        feed(&mut reader, &stream, 64),
        vec![b"12345678".to_vec(), Vec::new(), b"12345678".to_vec()]
    );

    let mut reader = FrameReader::new(limit);
    let buffer = reader.unfilled().unwrap();
    assert_eq!(buffer.len(), 4, "only the prefix is asked for");
    buffer.copy_from_slice(&9u32.to_be_bytes());
    assert_eq!(reader.filled(4), Err(FrameTooLarge));
    assert_eq!(reader.unfilled().err(), Some(FrameTooLarge));
    assert_eq!(reader.filled(0), Err(FrameTooLarge));

    assert_eq!(encode_frame(limit, b"123456789"), Err(FrameTooLarge));
    assert_eq!(
        &encode_frame(limit, b"ab").unwrap(),
        &[0, 0, 0, 2, b'a', b'b']
    );
}
