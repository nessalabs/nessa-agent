//! How long a send waited for the first reply text, on this machine only.
//!
//! `conversation.send` notes the gateway clock when it starts. The next view
//! of that turn that contains reply text (`kind` `text`, not empty, not a
//! thought) records the elapsed milliseconds. A steer is not a send. A view
//! with no reply text leaves the clock running. Pending clocks are keyed by
//! conversation and execution, because an execution id is only unique inside
//! one conversation.
//!
//! The read path does not open a file. It asks [`FirstTokenStore`], and the
//! file work runs on a blocking thread. Composition builds the store: a file
//! in the log directory, or an unavailable one. Tests substitute a store that
//! is slow, failing, or unavailable. Nothing is sent off the machine.
//!
//! ```text
//! conversation read ──ask──▶ FirstTokenStore
//!                                │
//!                    blocking thread ──record──▶ message-to-first-token-ms
//! ```
//!
//! The arrow is the duration handed to the store. The file is the one
//! `nessa doctor` reads.

use super::super::core::local_timings::FileTimingJournal;
use nessa_protocol::conversation::view::{ConversationPart, ConversationView};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};

/// Pending sends kept in memory. Older than this are dropped so a turn that
/// never produces text cannot grow the map.
const PENDING_LIMIT: usize = 32;

/// Where a first-token duration is kept. The conversation read path owns this
/// port; composition supplies the file, and tests supply their own.
pub(crate) trait FirstTokenStore: Send + Sync {
    /// Whether a duration would be kept. An unavailable store is not.
    fn available(&self) -> bool {
        true
    }

    fn record(&self, elapsed_ms: u64) -> Result<(), FirstTokenFailure>;
}

/// Why a duration was not kept. `Failed` carries the store's own words for
/// the log; callers branch on the variant.
#[derive(Debug)]
pub(crate) enum FirstTokenFailure {
    Unavailable,
    Failed(String),
}

/// The file beside `gateway.log`. Its journal serializes the whole rewrite.
pub(crate) struct FileFirstTokenStore {
    journal: FileTimingJournal,
}

impl FileFirstTokenStore {
    pub(crate) fn new(path: PathBuf) -> Self {
        Self {
            journal: FileTimingJournal::new(path),
        }
    }
}

impl FirstTokenStore for FileFirstTokenStore {
    fn record(&self, elapsed_ms: u64) -> Result<(), FirstTokenFailure> {
        self.journal
            .record(elapsed_ms)
            .map_err(|error| FirstTokenFailure::Failed(error.to_string()))
    }
}

/// A store that keeps nothing. Used until composition installs a file, and
/// by tests that have no timing file.
pub(crate) struct UnavailableFirstTokenStore;

impl FirstTokenStore for UnavailableFirstTokenStore {
    fn available(&self) -> bool {
        false
    }

    fn record(&self, _elapsed_ms: u64) -> Result<(), FirstTokenFailure> {
        Err(FirstTokenFailure::Unavailable)
    }
}

struct PendingTurn {
    conversation_id: String,
    execution_id: String,
    /// `None` after the duration was recorded, so a later retry of the same
    /// send does not start another clock against text that is already there.
    started_ms: Option<u64>,
}

pub(crate) struct TurnTimings {
    pending: Mutex<Vec<PendingTurn>>,
    store: Arc<dyn FirstTokenStore>,
}

impl TurnTimings {
    pub(crate) fn quiet() -> Self {
        Self::recording(Arc::new(UnavailableFirstTokenStore))
    }

    pub(crate) fn recording(store: Arc<dyn FirstTokenStore>) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            store,
        }
    }

    /// Arm the clock for a newly admitted send. A second call for the same
    /// conversation and execution leaves the original clock, including after
    /// its duration was already recorded. Nothing is written.
    pub(crate) fn sent(&self, conversation_id: &str, execution_id: &str, started_ms: u64) {
        if !self.store.available() {
            return;
        }
        let mut pending = lock(&self.pending);
        *pending = note_sent(
            std::mem::take(&mut pending),
            conversation_id,
            execution_id,
            started_ms,
        );
    }

    /// Record every pending turn of this view that now has reply text.
    ///
    /// The file write is not done here. A blocking thread asks the store, so
    /// a slow disk does not hold the conversation read.
    pub(crate) fn observe(&self, view: &ConversationView, now_ms: u64) {
        if !self.store.available() {
            return;
        }
        let ready: Vec<&str> = view
            .messages
            .iter()
            .filter(|message| has_reply_text(&message.parts))
            .map(|message| message.execution_id.as_str())
            .collect();
        self.record_ready(view.conversation_id.as_str(), &ready, now_ms);
    }

    fn record_ready(&self, conversation_id: &str, ready: &[&str], now_ms: u64) {
        if !self.store.available() {
            return;
        }
        let durations = {
            let mut pending = lock(&self.pending);
            let (next, durations) =
                take_ready(std::mem::take(&mut pending), conversation_id, ready, now_ms);
            *pending = next;
            durations
        };
        schedule(&self.store, durations);
    }
}

fn schedule(store: &Arc<dyn FirstTokenStore>, durations: Vec<u64>) {
    for elapsed_ms in durations {
        let store = Arc::clone(store);
        let write = move || {
            if let Err(FirstTokenFailure::Failed(error)) = store.record(elapsed_ms) {
                tracing::warn!(%error, "could not record a first-token timing");
            }
        };
        match tokio::runtime::Handle::try_current() {
            Ok(handle) => {
                handle.spawn_blocking(write);
            }
            Err(_) => write(),
        }
    }
}

fn lock(pending: &Mutex<Vec<PendingTurn>>) -> std::sync::MutexGuard<'_, Vec<PendingTurn>> {
    pending
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// A part the person would read as the reply, not a thought or an empty chunk.
pub(crate) fn has_reply_text(parts: &[ConversationPart]) -> bool {
    parts
        .iter()
        .any(|part| part.kind == "text" && !part.text.is_empty())
}

fn note_sent(
    entries: Vec<PendingTurn>,
    conversation_id: &str,
    execution_id: &str,
    started_ms: u64,
) -> Vec<PendingTurn> {
    if entries.iter().any(|pending| {
        pending.conversation_id == conversation_id && pending.execution_id == execution_id
    }) {
        return entries;
    }
    let mut next = entries;
    next.push(PendingTurn {
        conversation_id: conversation_id.to_owned(),
        execution_id: execution_id.to_owned(),
        started_ms: Some(started_ms),
    });
    let skip = next.len().saturating_sub(PENDING_LIMIT);
    next.into_iter().skip(skip).collect()
}

fn take_ready(
    entries: Vec<PendingTurn>,
    conversation_id: &str,
    ready: &[&str],
    now_ms: u64,
) -> (Vec<PendingTurn>, Vec<u64>) {
    let mut durations = Vec::new();
    let remaining = entries
        .into_iter()
        .map(|mut pending| {
            let this_conversation = pending.conversation_id == conversation_id
                && pending.started_ms.is_some()
                && ready
                    .iter()
                    .any(|execution| *execution == pending.execution_id);
            if this_conversation {
                durations.push(now_ms.saturating_sub(pending.started_ms.unwrap_or(now_ms)));
                pending.started_ms = None;
            }
            pending
        })
        .collect();
    (remaining, durations)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
    use std::thread;
    use std::time::Duration;

    fn part(kind: &str, text: &str) -> ConversationPart {
        ConversationPart {
            message_id: None,
            offset: 0,
            kind: kind.to_owned(),
            text: text.to_owned(),
            tool_id: String::new(),
            notice_id: String::new(),
            notice_kind: String::new(),
        }
    }

    struct Held {
        delay: Duration,
        fail: bool,
        samples: Mutex<Vec<u64>>,
        entered: Arc<AtomicBool>,
        finished: Arc<AtomicUsize>,
    }

    impl FirstTokenStore for Held {
        fn record(&self, elapsed_ms: u64) -> Result<(), FirstTokenFailure> {
            self.entered.store(true, Ordering::SeqCst);
            thread::sleep(self.delay);
            if self.fail {
                self.finished.fetch_add(1, Ordering::SeqCst);
                return Err(FirstTokenFailure::Failed("disk full".into()));
            }
            self.samples.lock().unwrap().push(elapsed_ms);
            self.finished.fetch_add(1, Ordering::SeqCst);
            Ok(())
        }
    }

    fn held(delay: Duration, fail: bool) -> (Arc<Held>, TurnTimings) {
        let store = Arc::new(Held {
            delay,
            fail,
            samples: Mutex::new(Vec::new()),
            entered: Arc::new(AtomicBool::new(false)),
            finished: Arc::new(AtomicUsize::new(0)),
        });
        let timings = TurnTimings::recording(store.clone());
        (store, timings)
    }

    fn wait_until(flag: &AtomicUsize, expected: usize) {
        let deadline = std::time::Instant::now() + Duration::from_secs(2);
        while flag.load(Ordering::SeqCst) < expected {
            assert!(
                std::time::Instant::now() < deadline,
                "the store did not finish"
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn reply_text_is_a_non_empty_text_part() {
        assert!(has_reply_text(&[part("text", "hi")]));
        assert!(!has_reply_text(&[part("text", "")]));
        assert!(!has_reply_text(&[part("thought", "hi")]));
        assert!(!has_reply_text(&[]));
    }

    #[test]
    fn the_first_text_records_the_wait_once_and_a_retry_keeps_the_clock() {
        let untouched = note_sent(Vec::new(), "conversation", "turn", 100);
        let (left, durations) = take_ready(untouched, "conversation", &[], 250);
        assert!(durations.is_empty());
        assert_eq!(left.len(), 1);
        let entries = note_sent(Vec::new(), "conversation", "turn", 100);
        let (left, durations) = take_ready(entries, "conversation", &["turn"], 250);
        assert_eq!(durations, vec![150]);
        assert_eq!(left[0].started_ms, None);
        let restarted = note_sent(
            note_sent(Vec::new(), "conversation", "turn", 100),
            "conversation",
            "turn",
            400,
        );
        let (_, durations) = take_ready(restarted, "conversation", &["turn"], 450);
        assert_eq!(durations, vec![350]);
    }

    #[test]
    fn a_retry_after_the_reply_does_not_start_another_clock() {
        let entries = note_sent(Vec::new(), "conversation", "turn", 100);
        let (left, durations) = take_ready(entries, "conversation", &["turn"], 180);
        assert_eq!(durations, vec![80]);
        let retried = note_sent(left, "conversation", "turn", 200);
        let (_, durations) = take_ready(retried, "conversation", &["turn"], 210);
        assert!(durations.is_empty());
    }

    #[test]
    fn the_same_execution_in_another_conversation_keeps_its_own_clock() {
        let entries = note_sent(
            note_sent(Vec::new(), "one", "turn", 100),
            "two",
            "turn",
            300,
        );
        let (left, durations) = take_ready(entries, "one", &["turn"], 180);
        assert_eq!(durations, vec![80]);
        let other = left
            .iter()
            .find(|pending| pending.conversation_id == "two")
            .expect("the other conversation keeps its clock");
        assert_eq!(other.started_ms, Some(300));
        let (_, durations) = take_ready(left, "two", &["turn"], 350);
        assert_eq!(durations, vec![50]);
    }

    #[test]
    fn a_clock_set_back_records_zero_and_the_pending_list_stays_bounded() {
        let (_, durations) = take_ready(
            note_sent(Vec::new(), "conversation", "turn", 500),
            "conversation",
            &["turn"],
            100,
        );
        assert_eq!(durations, vec![0]);
        let mut entries = Vec::new();
        for index in 0..PENDING_LIMIT + 5 {
            entries = note_sent(
                entries,
                "conversation",
                &format!("turn-{index}"),
                index as u64,
            );
        }
        assert_eq!(entries.len(), PENDING_LIMIT);
        assert_eq!(entries[0].execution_id, format!("turn-{}", 5));
    }

    #[test]
    fn an_unavailable_store_keeps_no_clock_and_writes_nothing() {
        let timings = TurnTimings::quiet();
        timings.sent("conversation", "turn", 10);
        timings.record_ready("conversation", &["turn"], 40);
        assert!(lock(&timings.pending).is_empty());
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_slow_store_does_not_hold_the_caller() {
        let (store, timings) = held(Duration::from_millis(200), false);
        timings.sent("conversation", "turn", 10);
        let started = std::time::Instant::now();
        timings.record_ready("conversation", &["turn"], 25);
        assert!(
            started.elapsed() < Duration::from_millis(100),
            "the read waited for the store"
        );
        assert!(
            !store.entered.load(Ordering::SeqCst) || started.elapsed() < Duration::from_millis(100)
        );
        wait_until(&store.finished, 1);
        assert_eq!(store.samples.lock().unwrap().as_slice(), &[15]);
    }

    #[tokio::test(flavor = "current_thread")]
    async fn a_failing_store_is_asked_and_does_not_panic() {
        let (store, timings) = held(Duration::ZERO, true);
        timings.sent("conversation", "turn", 1);
        timings.record_ready("conversation", &["turn"], 4);
        wait_until(&store.finished, 1);
        assert!(store.samples.lock().unwrap().is_empty());
        assert_eq!(lock(&timings.pending)[0].started_ms, None);
    }
}
