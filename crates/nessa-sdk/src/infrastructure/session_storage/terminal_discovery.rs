//! Bounded validation progress shared by physical read operations.
//!
//! Cached entries contain immutable-prefix evidence and framing hash metadata,
//! never a worker or semantic body. An operation removes its state from the
//! cache before I/O; its drop guard returns progress on completion or unwind.
#![deny(missing_docs)]

use super::{
    record_source::source_error,
    save_group::GroupProgress,
    stream_fact::{FrameStep, FrameValidator},
};
use event_stream::{
    infrastructure::SqliteStore, Cursor, EventReader, PageLimits, Runtime, StreamKey,
};
use nessa_sync::replication::application::SourceError;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const CACHE_ENTRIES: usize = 16;
const STEP: PageLimits = PageLimits {
    max_records: 16,
    max_bytes: super::MAX_STORED_RECORD_BYTES,
};

/// Result of bounded discovery without replacing committed heads with raw tails.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RecordReadStatus<T> {
    /// The requested operation completed using validated terminal evidence.
    Ready(T),
    /// More bounded validation work is needed, or this stream has another
    /// validation owner. Retry with fresh authorization and the same scope.
    Preparing,
}

#[derive(Default)]
pub(super) struct TerminalCache {
    #[cfg(test)]
    returned_records: AtomicUsize,
    #[cfg(test)]
    returned_bytes: AtomicUsize,
    entries: Mutex<VecDeque<Entry>>,
}
struct Entry {
    key: StreamKey,
    state: Option<Progress>,
}
struct Progress {
    validator: FrameValidator,
    terminal: u64,
    groups: GroupProgress,
    through: Option<u64>,
    failed: bool,
}
struct Owner {
    cache: Arc<TerminalCache>,
    key: StreamKey,
    state: Progress,
}
impl Drop for Owner {
    fn drop(&mut self) {
        let mut entries = self.cache.entries.lock().unwrap_or_else(|e| e.into_inner());
        let index = entries
            .iter()
            .position(|e| e.key == self.key)
            .expect("active cache entry retains its owner");
        let mut entry = entries.remove(index).unwrap();
        entry.state = Some(Progress {
            validator: self.state.validator.clone(),
            terminal: self.state.terminal,
            groups: self.state.groups.clone(),
            through: self.state.through,
            failed: self.state.failed,
        });
        entries.push_back(entry);
    }
}
impl TerminalCache {
    fn acquire(self: &Arc<Self>, key: &StreamKey) -> Option<Owner> {
        let mut entries = self.entries.lock().unwrap_or_else(|e| e.into_inner());
        let state = if let Some(index) = entries.iter().position(|e| &e.key == key) {
            entries[index].state.take()?
        } else {
            if entries.len() == CACHE_ENTRIES {
                let index = entries.iter().position(|e| e.state.is_some())?;
                entries.remove(index);
            }
            entries.push_back(Entry {
                key: key.clone(),
                state: None,
            });
            Progress {
                validator: FrameValidator::after(0),
                terminal: 0,
                groups: GroupProgress::after(0),
                through: None,
                failed: false,
            }
        };
        Some(Owner {
            cache: self.clone(),
            key: key.clone(),
            state,
        })
    }

    pub(super) async fn discover(
        self: &Arc<Self>,
        runtime: &Runtime<SqliteStore>,
        key: &StreamKey,
        tail: u64,
        target: Option<u64>,
    ) -> Result<RecordReadStatus<u64>, SourceError> {
        let Some(mut owner) = self.acquire(key) else {
            return Ok(RecordReadStatus::Preparing);
        };
        let state = &mut owner.state;
        if state.failed {
            if target.is_some_and(|target| target <= state.terminal) {
                *state = Progress {
                    validator: FrameValidator::after(0),
                    terminal: 0,
                    groups: GroupProgress::after(0),
                    through: None,
                    failed: false,
                };
            } else {
                return Err(SourceError::Unavailable);
            }
        }
        if tail < state.terminal {
            return Err(SourceError::IdentityChanged);
        }
        if target == Some(state.terminal) {
            return Ok(RecordReadStatus::Ready(state.terminal));
        }
        if target.is_some_and(|target| target <= state.validator.offset()) {
            // Old targets outside the one remembered publication must prove the
            // same group terminal from the immutable prefix, not a physical seal.
            *state = Progress {
                validator: FrameValidator::after(0),
                terminal: 0,
                groups: GroupProgress::after(0),
                through: None,
                failed: false,
            };
        }
        let through = *state.through.get_or_insert(target.unwrap_or(tail));
        if through > tail {
            return Err(SourceError::IdentityChanged);
        }
        if state.validator.offset() < through {
            let page = runtime
                .read_after(
                    &Cursor::new(key.clone(), state.validator.offset()),
                    STEP,
                    Some(&Cursor::new(key.clone(), through)),
                )
                .await
                .map_err(source_error)?;
            if page.records.is_empty() {
                return Err(SourceError::Unavailable);
            }
            #[cfg(test)]
            self.returned_records
                .fetch_add(page.records.len(), Ordering::SeqCst);
            #[cfg(test)]
            self.returned_bytes.fetch_add(
                page.records
                    .iter()
                    .map(|record| record.event.accounted_bytes())
                    .sum::<usize>(),
                Ordering::SeqCst,
            );
            for record in page.records {
                if record.cursor.stream != *key {
                    return Err(SourceError::IdentityChanged);
                }
                let step = state
                    .validator
                    .push(&record.event, record.cursor.offset)
                    .map_err(|_| {
                        state.failed = true;
                        SourceError::Unavailable
                    })?;
                match step {
                    FrameStep::Pending(Some(bytes)) => state.groups.piece(bytes).map_err(|_| {
                        state.failed = true;
                        SourceError::Unavailable
                    })?,
                    FrameStep::Pending(None) => {}
                    FrameStep::Aborted => state.groups.reset_frame(),
                    FrameStep::Complete { key, body } => {
                        if let Some(bytes) = body {
                            state.groups.piece(bytes).map_err(|_| {
                                state.failed = true;
                                SourceError::Unavailable
                            })?;
                        }
                        let header =
                            state
                                .groups
                                .complete(&key, record.cursor.offset)
                                .map_err(|_| {
                                    state.failed = true;
                                    SourceError::Unavailable
                                })?;
                        if !header.identity.matches_stream(&record.cursor.stream) {
                            state.failed = true;
                            return Err(SourceError::Unavailable);
                        }
                        state.terminal = state.groups.published();
                    }
                }
            }
        }
        if state.validator.offset() == through {
            state.through = None;
            if target.is_some_and(|target| target > state.validator.offset()) {
                return Ok(RecordReadStatus::Preparing);
            }
            Ok(RecordReadStatus::Ready(state.terminal))
        } else {
            Ok(RecordReadStatus::Preparing)
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/infrastructure/session_storage/terminal_discovery.rs"]
mod tests;
