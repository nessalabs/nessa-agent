//! How long a send waited for the first reply text, on this machine only.
//!
//! `conversation.send` notes the gateway clock when it starts. The next view
//! of that turn that contains reply text (`kind` `text`, not empty, not a
//! thought) appends the elapsed milliseconds to `message-to-first-token-ms`
//! in the log directory. A steer is not a send. A view with no reply text
//! leaves the clock running. The file is the one `nessa doctor` reads; it
//! is not sent anywhere.

use super::super::core::local_timings;
use nessa_protocol::conversation::view::{ConversationPart, ConversationView};
use std::path::PathBuf;
use std::sync::Mutex;

/// Pending sends kept in memory. Older than this are dropped so a turn that
/// never produces text cannot grow the map.
const PENDING_LIMIT: usize = 32;

pub(crate) struct TurnTimings {
    pending: Mutex<Vec<(String, u64)>>,
    path: Option<PathBuf>,
}

impl TurnTimings {
    pub(crate) fn quiet() -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            path: None,
        }
    }

    pub(crate) fn recording(path: PathBuf) -> Self {
        Self {
            pending: Mutex::new(Vec::new()),
            path: Some(path),
        }
    }

    /// Start, or restart, the clock for `execution_id`. No file is written.
    pub(crate) fn sent(&self, execution_id: &str, started_ms: u64) {
        if self.path.is_none() {
            return;
        }
        let mut pending = lock(&self.pending);
        *pending = note_sent(std::mem::take(&mut pending), execution_id, started_ms);
    }

    /// Record every pending turn that now has reply text, then forget it.
    pub(crate) fn observe(&self, view: &ConversationView, now_ms: u64) {
        let Some(path) = &self.path else {
            return;
        };
        let ready = view
            .messages
            .iter()
            .filter(|message| has_reply_text(&message.parts))
            .map(|message| message.execution_id.as_str())
            .collect::<Vec<_>>();
        let durations = {
            let mut pending = lock(&self.pending);
            let (next, durations) = take_ready(std::mem::take(&mut pending), &ready, now_ms);
            *pending = next;
            durations
        };
        for ms in durations {
            if let Err(error) = local_timings::append_sample(path, ms) {
                tracing::warn!(%error, "could not record a first-token timing");
            }
        }
    }
}

fn lock(pending: &Mutex<Vec<(String, u64)>>) -> std::sync::MutexGuard<'_, Vec<(String, u64)>> {
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
    entries: Vec<(String, u64)>,
    execution_id: &str,
    started_ms: u64,
) -> Vec<(String, u64)> {
    let mut next: Vec<(String, u64)> = entries
        .into_iter()
        .filter(|(known, _)| known != execution_id)
        .collect();
    next.push((execution_id.to_owned(), started_ms));
    let skip = next.len().saturating_sub(PENDING_LIMIT);
    next.into_iter().skip(skip).collect()
}

fn take_ready(
    entries: Vec<(String, u64)>,
    ready: &[&str],
    now_ms: u64,
) -> (Vec<(String, u64)>, Vec<u64>) {
    let mut durations = Vec::new();
    let remaining = entries
        .into_iter()
        .filter(|(id, started)| {
            if ready.iter().any(|ready_id| ready_id == id) {
                durations.push(now_ms.saturating_sub(*started));
                false
            } else {
                true
            }
        })
        .collect();
    (remaining, durations)
}

#[cfg(test)]
mod tests {
    use super::*;

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

    #[test]
    fn reply_text_is_a_non_empty_text_part() {
        assert!(has_reply_text(&[part("text", "hi")]));
        assert!(!has_reply_text(&[part("text", "")]));
        assert!(!has_reply_text(&[part("thought", "hi")]));
        assert!(!has_reply_text(&[]));
    }

    #[test]
    fn the_first_text_records_the_wait_once_and_a_restart_replaces_the_clock() {
        let entries = note_sent(Vec::new(), "turn", 100);
        let (left, durations) = take_ready(entries.clone(), &[], 250);
        assert!(durations.is_empty());
        assert_eq!(left, entries);
        let (left, durations) = take_ready(entries, &["turn"], 250);
        assert_eq!(durations, vec![150]);
        assert!(left.is_empty());
        let restarted = note_sent(note_sent(Vec::new(), "turn", 100), "turn", 400);
        let (_, durations) = take_ready(restarted, &["turn"], 450);
        assert_eq!(durations, vec![50]);
    }

    #[test]
    fn a_clock_set_back_records_zero_and_the_pending_list_stays_bounded() {
        let (_, durations) = take_ready(note_sent(Vec::new(), "turn", 500), &["turn"], 100);
        assert_eq!(durations, vec![0]);
        let mut entries = Vec::new();
        for index in 0..PENDING_LIMIT + 5 {
            entries = note_sent(entries, &format!("turn-{index}"), index as u64);
        }
        assert_eq!(entries.len(), PENDING_LIMIT);
        assert_eq!(entries[0].0, format!("turn-{}", 5));
    }
}
