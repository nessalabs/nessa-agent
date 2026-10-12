use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::common::value_objects::{MediaType, Sha256Digest};
use std::num::NonZeroU64;

/// The most bytes one range asks for: the maximum of `ArtifactReadParams.maxBytes`
/// in the published product schema. Not a generated bound, so it is written
/// here once; `ranges_and_sizes_are_the_schema_bounds` reads the schema and
/// fails when the two differ.
pub(crate) const HELD_FILE_RANGE_BYTES: u64 = 32_768;

/// The largest file `artifact.read` names: the maximum of
/// `ArtifactReadParams.size` in the published product schema, held to it by
/// the same test.
pub(crate) const MAX_HELD_FILE_BYTES: u64 = 64 * 1024 * 1024;

/// A size of no bytes, or more than [`MAX_HELD_FILE_BYTES`].
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct SizeOutOfRange;

/// A file a conversation holds, named exactly as its view names it: digest,
/// media type and size together, since the same bytes held as another type
/// are another file. The gateway decides whether it is held; this only names it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HeldFile {
    conversation: ConversationId,
    digest: Sha256Digest,
    media_type: MediaType,
    size: NonZeroU64,
}

impl HeldFile {
    pub(crate) fn new(
        conversation: ConversationId,
        digest: Sha256Digest,
        media_type: MediaType,
        size: u64,
    ) -> Result<Self, SizeOutOfRange> {
        let size = NonZeroU64::new(size)
            .filter(|size| size.get() <= MAX_HELD_FILE_BYTES)
            .ok_or(SizeOutOfRange)?;
        Ok(Self {
            conversation,
            digest,
            media_type,
            size,
        })
    }
    pub(crate) fn conversation(&self) -> &ConversationId {
        &self.conversation
    }
    pub(crate) fn digest(&self) -> &Sha256Digest {
        &self.digest
    }
    pub(crate) fn media_type(&self) -> &MediaType {
        &self.media_type
    }
    pub(crate) fn size(&self) -> u64 {
        self.size.get()
    }
    /// The range that starts at `offset`: [`HELD_FILE_RANGE_BYTES`], or the
    /// bytes left when fewer remain. `None` at or past the end.
    pub(crate) fn range_at(&self, offset: u64) -> Option<HeldRange> {
        let left = self.size().checked_sub(offset).filter(|left| *left > 0)?;
        Some(HeldRange {
            offset,
            length: left.min(HELD_FILE_RANGE_BYTES),
        })
    }
    /// The range the file ends in: where a complete part is asked about
    /// again, so a part is never taken in place of the gateway's answer.
    pub(crate) fn last_range(&self) -> HeldRange {
        let offset = (self.size() - 1) / HELD_FILE_RANGE_BYTES * HELD_FILE_RANGE_BYTES;
        HeldRange {
            offset,
            length: self.size() - offset,
        }
    }
}

/// One range of a held file: where it starts and exactly how many bytes the
/// gateway must answer with.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) struct HeldRange {
    offset: u64,
    length: u64,
}

impl HeldRange {
    pub(crate) fn offset(self) -> u64 {
        self.offset
    }
    pub(crate) fn length(self) -> u64 {
        self.length
    }
    /// What is asked for: always the full range size. The gateway answers
    /// fewer bytes only where the file ends, which `length` already says.
    pub(crate) fn max_bytes(self) -> u64 {
        HELD_FILE_RANGE_BYTES
    }
    /// Whether an answer is this range: the offset asked, echoed, and exactly
    /// `length` bytes. Nothing in a range says it is the file's; only the
    /// whole file's size and digest do.
    pub(crate) fn answered_by(self, offset: u64, bytes: &[u8]) -> bool {
        offset == self.offset && u64::try_from(bytes.len()) == Ok(self.length)
    }
}

#[cfg(test)]
#[path = "../../../tests/read_only_sync/domain/held_file.rs"]
mod tests;
