use super::{ChunkWriter, MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES};
use crate::infrastructure::session_storage::{
    physical_record_schema, TranscriptError, TranscriptFold,
};
use nessa_sync::replication::domain::{Id, Scope};
use std::io::Write;

fn fold() -> TranscriptFold {
    let id = |value| Id::new(value).unwrap();
    TranscriptFold::new(Scope::new(
        id("receiver"),
        id("gateway"),
        id("conversation"),
        id("incarnation"),
        physical_record_schema(),
        id("epoch"),
    ))
    .unwrap()
}

#[test]
fn checkpoint_budget_counts_metadata_and_preserves_complete_encoding() {
    let transcript = fold();
    let full = transcript.checkpoint().unwrap();
    let bytes = full.chunks().map(<[u8]>::len).sum::<usize>();
    assert!(bytes > 1);
    for limit in [0, 1, bytes - 1] {
        assert_eq!(
            transcript.checkpoint_with_limit(limit),
            Err(TranscriptError::CheckpointTooLarge)
        );
        assert_eq!(transcript.checkpoint().unwrap(), full);
    }
    assert_eq!(transcript.checkpoint_with_limit(bytes).unwrap(), full);
    assert_eq!(transcript.checkpoint_with_limit(bytes + 1).unwrap(), full);
}

#[test]
fn chunk_writer_refuses_before_retaining_bytes_beyond_aggregate_allowance() {
    let limit = MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES + 13;
    let mut writer = ChunkWriter::with_limit(Some(limit));
    writer
        .write_all(&vec![b'x'; MAX_TRANSCRIPT_CHECKPOINT_CHUNK_BYTES])
        .unwrap();
    assert_eq!(writer.chunks.len(), 1);
    assert_eq!(writer.current.capacity(), 13);
    writer.write_all(b"thirteenbytes").unwrap();
    assert_eq!(writer.current.len(), 13);
    let before = writer.current.clone();
    let error = writer.write_all(b"y").unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::FileTooLarge);
    assert!(writer.limit_exceeded());
    assert_eq!(writer.written, limit);
    assert_eq!(writer.current, before);
    assert_eq!(writer.chunks.len(), 1);
    assert_eq!(
        writer.finish().chunks().map(<[u8]>::len).sum::<usize>(),
        limit
    );
}

#[test]
fn a_single_oversized_write_retains_no_checkpoint_prefix() {
    let mut writer = ChunkWriter::with_limit(Some(7));
    assert_eq!(writer.current.capacity(), 7);
    assert!(writer.write_all(b"eight888").is_err());
    assert!(writer.limit_exceeded());
    assert_eq!(writer.written, 0);
    assert!(writer.current.is_empty());
    assert!(writer.chunks.is_empty());
}

#[test]
fn borrowed_guard_checkpoint_budget_preserves_staged_encoding_and_drop_undo() {
    let mut transcript = fold();
    let before = transcript.checkpoint().unwrap();
    let scope = transcript.scope().clone();
    {
        let mut staged = transcript.transaction();
        staged.observe_source_head(&scope, 0).unwrap();
        let complete = staged.checkpoint().unwrap();
        let bytes = complete.chunks().map(<[u8]>::len).sum::<usize>();
        assert_eq!(staged.checkpoint_with_limit(bytes).unwrap(), complete);
        assert_eq!(
            staged.checkpoint_with_limit(bytes - 1),
            Err(TranscriptError::CheckpointTooLarge)
        );
        assert_eq!(staged.checkpoint().unwrap(), complete);
    }
    assert_eq!(transcript.checkpoint().unwrap(), before);
}
