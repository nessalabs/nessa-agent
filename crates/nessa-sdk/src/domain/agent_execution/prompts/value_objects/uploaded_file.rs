#![deny(missing_docs)]

use crate::domain::{
    agent_execution::ExecutionError,
    common::value_objects::{MediaType, Sha256Digest},
};

/// Original stored input, independent of any execution machine or placement.
#[derive(Clone, Debug, PartialEq, Eq, Hash)]
pub struct UploadedFileReference {
    digest: Sha256Digest,
    media_type: MediaType,
    size: u64,
    name: Box<str>,
}
impl UploadedFileReference {
    /// Largest stored input and combined upload payload in one message.
    pub const MAX_BYTES: u64 = 64 * 1024 * 1024;
    /// Maximum UTF-8 bytes in the original filename.
    pub const MAX_NAME_BYTES: usize = 255;
    /// Validate a stored file's reference. No bytes are read.
    ///
    /// # Errors
    /// Empty bytes or a blank name return `EmptyValue`; oversized values return
    /// `ValueTooLong`; a name containing a separator or control returns `InvalidPath`.
    pub fn new(
        digest: Sha256Digest,
        media_type: MediaType,
        size: u64,
        name: String,
    ) -> Result<Self, ExecutionError> {
        if size == 0 || name.trim().is_empty() {
            return Err(ExecutionError::EmptyValue("uploaded file"));
        }
        if size > Self::MAX_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "uploaded file",
                max_bytes: Self::MAX_BYTES as usize,
            });
        }
        if name.len() > Self::MAX_NAME_BYTES {
            return Err(ExecutionError::ValueTooLong {
                field: "uploaded file name",
                max_bytes: Self::MAX_NAME_BYTES,
            });
        }
        if name
            .chars()
            .any(|c| c.is_control() || c == '/' || c == '\\')
        {
            return Err(ExecutionError::InvalidPath);
        }
        Ok(Self {
            digest,
            media_type,
            size,
            name: name.into_boxed_str(),
        })
    }
    /// SHA-256 of the original stored bytes.
    pub fn digest(&self) -> Sha256Digest {
        self.digest
    }
    /// Declared media type.
    pub fn media_type(&self) -> &MediaType {
        &self.media_type
    }
    /// Length of the original stored bytes.
    pub fn size(&self) -> u64 {
        self.size
    }
    /// Original filename, with no directory authority.
    pub fn name(&self) -> &str {
        &self.name
    }
    /// Generated workspace filename, at most 251 UTF-8 bytes. A truncated name
    /// is a label only; the full digest distinguishes different byte content.
    pub fn destination_name(&self) -> String {
        let mut end = self.name.len().min(180);
        while !self.name.is_char_boundary(end) {
            end -= 1;
        }
        format!("nessa-{}-{}", self.digest.to_hex(), &self.name[..end])
    }
}
