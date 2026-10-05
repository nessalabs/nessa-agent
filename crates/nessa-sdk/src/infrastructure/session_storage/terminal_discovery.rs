//! Bounded validation progress shared by physical read operations.
//!
//! Cached entries contain immutable-prefix evidence and framing hash metadata,
//! never a worker or semantic body. An operation removes its state from the
//! cache before I/O; its drop guard returns progress on completion or unwind.
#![deny(missing_docs)]

use super::save_batch::RecordRuntime;
use super::{
    record_source::source_error,
    save_group::GroupProgress,
    stream_fact::{FrameStep, FrameValidator},
};
use event_stream::{Cursor, EventReader, PageLimits, StreamKey};
use nessa_sync::replication::application::SourceError;
#[cfg(test)]
use std::sync::atomic::{AtomicUsize, Ordering};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

const CACHE_ENTRIES: usize = 16;
// The prior source-local 64-entry policy has this single shared owner.
const PROVEN_COMPLETIONS: usize = 64;
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
    /// validation owner. Call again with the same scope for the next step; a
    /// host may make several calls within one authorized read and
    /// reauthorizes before a later one.
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
/// An ordinary head's fixed physical ceiling, captured before cache checkout.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct CapturedCeiling(pub(super) u64);
/// An exact outer-save publication requested by a page reader.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct ExactPublication(pub(super) u64);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) enum DiscoveryQuery {
    Head,
    CapturedHead(CapturedCeiling),
    Publication(ExactPublication),
}
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum FixedQuery {
    CapturedHead(CapturedCeiling),
    Publication(ExactPublication),
}
impl FixedQuery {
    fn offset(self) -> u64 {
        match self {
            Self::CapturedHead(CapturedCeiling(offset))
            | Self::Publication(ExactPublication(offset)) => offset,
        }
    }
}
// The exclusive Owner is the sole mutator. Idle entries retain these original
// allocations; checkout/drop moves them without duplicating validation owners.
struct Progress {
    forward: Scan,
    historical: Option<Box<Historical>>,
    proven: VecDeque<u64>,
    physical_tail: u64,
}
struct Historical {
    query: FixedQuery,
    scan: Scan,
}
struct Scan {
    validator: FrameValidator,
    groups: GroupProgress,
    through: Option<CapturedCeiling>,
    failed: bool,
}
impl Scan {
    fn new() -> Self {
        Self {
            validator: FrameValidator::after(0),
            groups: GroupProgress::after(0),
            through: None,
            failed: false,
        }
    }
    async fn advance(
        &mut self,
        runtime: &RecordRuntime,
        key: &StreamKey,
        until: u64,
        proven: &mut VecDeque<u64>,
        _cache: &TerminalCache,
    ) -> Result<(), SourceError> {
        let state = self;
        if state.failed {
            return Err(SourceError::Unavailable);
        }
        if state.validator.offset() < until {
            let page = runtime
                .read_after(
                    &Cursor::new(key.clone(), state.validator.offset()),
                    STEP,
                    Some(&Cursor::new(key.clone(), until)),
                )
                .await
                .map_err(source_error)?;
            if page.records.is_empty() {
                return Err(SourceError::Unavailable);
            }
            #[cfg(test)]
            _cache
                .returned_records
                .fetch_add(page.records.len(), Ordering::SeqCst);
            #[cfg(test)]
            _cache.returned_bytes.fetch_add(
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
                        let previous = state.groups.published();
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
                        let published = state.groups.published();
                        if published != previous && !proven.contains(&published) {
                            if proven.len() == PROVEN_COMPLETIONS {
                                proven.pop_front();
                            }
                            proven.push_back(published);
                        }
                    }
                }
            }
        }
        Ok(())
    }
}
struct Owner {
    cache: Arc<TerminalCache>,
    key: StreamKey,
    state: Option<Progress>,
}
impl Drop for Owner {
    fn drop(&mut self) {
        let mut entries = self.cache.entries.lock().unwrap_or_else(|e| e.into_inner());
        let index = entries
            .iter()
            .position(|e| e.key == self.key)
            .expect("active cache entry retains its owner");
        let mut entry = entries.remove(index).unwrap();
        // Transfer the original allocation instead of cloning both retained scans.
        entry.state = self.state.take();
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
                forward: Scan::new(),
                historical: None,
                proven: VecDeque::with_capacity(PROVEN_COMPLETIONS),
                physical_tail: 0,
            }
        };
        Some(Owner {
            cache: self.clone(),
            key: key.clone(),
            state: Some(state),
        })
    }

    pub(super) async fn discover(
        self: &Arc<Self>,
        runtime: &RecordRuntime,
        key: &StreamKey,
        query: DiscoveryQuery,
    ) -> Result<RecordReadStatus<u64>, SourceError> {
        let Some(mut owner) = self.acquire(key) else {
            return Ok(RecordReadStatus::Preparing);
        };
        let state = owner.state.as_mut().expect("acquired discovery progress");
        // Only this exclusive owner observes physical bounds against retained
        // progress. A captured ceiling or page target cannot report a shrink.
        let bounds = runtime.bounds(key).await.map_err(source_error)?;
        if bounds.floor.offset != 0 {
            return Err(SourceError::Pruned);
        }
        let tail = bounds.tail.offset;
        if tail < state.physical_tail {
            return Err(SourceError::IdentityChanged);
        }
        state.physical_tail = tail;
        let fixed = match query {
            DiscoveryQuery::Head => None,
            DiscoveryQuery::CapturedHead(ceiling) => {
                if ceiling.0 > tail {
                    return Err(SourceError::IdentityChanged);
                }
                Some(FixedQuery::CapturedHead(ceiling))
            }
            DiscoveryQuery::Publication(target) => {
                if target.0 > tail {
                    return Err(SourceError::InvalidRequest);
                }
                Some(FixedQuery::Publication(target))
            }
        };
        if let Some(query) = fixed {
            let target = query.offset();
            if target == 0
                || target == state.forward.groups.published()
                || state.proven.contains(&target)
            {
                return Ok(RecordReadStatus::Ready(target));
            }
            if target <= state.forward.validator.offset() {
                if state.forward.failed && target > state.forward.groups.published() {
                    return Err(SourceError::Unavailable);
                }
                // The forward scan validated (published, validator] and found
                // no completion there, so a target in that range is answered by
                // the last publication without replaying history from zero.
                if target > state.forward.groups.published() {
                    return Ok(RecordReadStatus::Ready(state.forward.groups.published()));
                }
                let replace = state.historical.as_ref().is_none_or(|historical| {
                    historical.query != query
                        && (historical.scan.failed
                            || historical.scan.validator.offset() == historical.query.offset())
                });
                if replace {
                    state.historical = Some(Box::new(Historical {
                        query,
                        scan: Scan::new(),
                    }));
                }
                let historical = state
                    .historical
                    .as_mut()
                    .expect("historical query retained");
                let result = historical
                    .scan
                    .advance(
                        runtime,
                        key,
                        historical.query.offset(),
                        &mut state.proven,
                        self,
                    )
                    .await;
                // A competing query may advance the original proof, but neither
                // its last publication nor its failure is an answer to this query.
                if historical.query != query {
                    return match result {
                        Err(error @ (SourceError::IdentityChanged | SourceError::Pruned)) => {
                            Err(error)
                        }
                        _ => Ok(RecordReadStatus::Preparing),
                    };
                }
                result?;
                return if historical.scan.validator.offset() == target {
                    Ok(RecordReadStatus::Ready(historical.scan.groups.published()))
                } else {
                    Ok(RecordReadStatus::Preparing)
                };
            }
        }
        let through = *state
            .forward
            .through
            .get_or_insert(CapturedCeiling(fixed.map_or(tail, FixedQuery::offset)));
        let until = fixed.map_or(through.0, |query| query.offset().min(through.0));
        state
            .forward
            .advance(runtime, key, until, &mut state.proven, self)
            .await?;
        if state.forward.validator.offset() == through.0 {
            state.forward.through = None;
        }
        if state.forward.validator.offset() == fixed.map_or(through.0, FixedQuery::offset) {
            Ok(RecordReadStatus::Ready(state.forward.groups.published()))
        } else {
            Ok(RecordReadStatus::Preparing)
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/infrastructure/session_storage/terminal_discovery.rs"]
mod tests;
