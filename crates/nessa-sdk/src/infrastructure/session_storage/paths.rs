//! One canonical spelling for every local session's journal and stable lease file.
use crate::domain::agent_execution::sessions::SessionId;
use data_encoding::BASE32HEX_NOPAD;
use std::path::{Path, PathBuf};

pub(super) struct SessionPaths {
    pub(super) journal: PathBuf,
    pub(super) lock: PathBuf,
}
impl SessionPaths {
    pub(super) fn new(root: &Path, id: &SessionId) -> Self {
        // Exact bytes round-trip through one lowercase alphabet, so filesystem
        // case folding cannot merge distinct IDs. The prefix avoids device names;
        // no separators, trailing dots, or spaces occur in the encoded filename.
        // 128 input bytes require at most 205 base32 digits: with prefix and the
        // longest extension, a component is at most 213 ASCII bytes.
        let stem = format!(
            "s-{}",
            BASE32HEX_NOPAD
                .encode(id.as_str().as_bytes())
                .to_ascii_lowercase()
        );
        Self {
            journal: root.join(format!("{stem}.jsonl")),
            lock: root.join(format!("{stem}.lock")),
        }
    }
}
