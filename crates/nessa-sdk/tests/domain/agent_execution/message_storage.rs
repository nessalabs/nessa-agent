//! Domain output values compact ownership without importing application policy.
use super::{MessageChunk, MessageKind};

#[test]
fn chunks_preserve_kind_exact_bytes_and_compact_original_and_cloned_storage() {
    for kind in [MessageKind::Text, MessageKind::Thought] {
        for text in ["", " \n\t", " exact é🦀 "] {
            let mut allocation = String::with_capacity(1024 * 1024);
            allocation.push_str(text);
            let chunk = match kind {
                MessageKind::Text => MessageChunk::text(allocation),
                MessageKind::Thought => MessageChunk::thought(allocation),
            };
            assert_eq!(chunk.kind(), kind);
            assert_eq!(chunk.as_str(), text);
            assert_eq!(chunk.payload_bytes(), text.len());
            for value in [chunk.clone(), chunk] {
                let storage = value.into_text();
                assert_eq!(storage, text);
                assert_eq!(storage.capacity(), text.len());
            }
        }
    }
}
