//! Borrowed evaluation window over observations already retained for one invocation.
//!
//! ```text
//! InvocationRecord.events --> InvocationObservationWindow --> hook callback
//! PermissionRequested -----> ProposedToolCall -------------> same callback
//! ```
//!
//! The window borrows a suffix of the existing slice. It does not copy payloads
//! on construction. The turn-boundary copy is a separate owned suffix, bounded
//! by the same caps, and it does not clone an oversized body.
#![deny(missing_docs)]

use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::{
    executions::{Initiation, MessageKind},
    tools::{ToolCallId, ToolStatus},
};
use std::fmt;

/// What one retained observation is.
///
/// A hook tells thinking, model text, a tool call, and a tool result apart by
/// this kind. The window yields the original [`ExecutionEvent`](ExecutionEvent);
/// [`ObservationKind::of`] reads the same event without a second representation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum ObservationKind {
    /// Provider reasoning. It is its own phrase channel and never joins model text.
    Thinking,
    /// Model text the transcript shows.
    ModelText,
    /// A tool call the provider has not finished.
    ToolCall,
    /// A tool call the provider reported as finished, succeeded or failed.
    ToolResult,
    /// A permission review the harness is asking about.
    PermissionReview,
    /// A permission review that was cancelled.
    PermissionCancelled,
    /// A permission review its deadline answered because nobody did.
    DeadlineAnswered,
    /// A review the provider declined before it became a choice.
    ReviewDeclined,
    /// A question the provider asked.
    Question,
    /// A question that is no longer waiting.
    QuestionClosed,
    /// The invocation finished.
    Finished,
}

impl ObservationKind {
    /// Kind of `event`.
    ///
    /// A tool update with no status, or with [`ToolStatus::Pending`] or
    /// [`ToolStatus::Running`], is a call. [`ToolStatus::Completed`] and
    /// [`ToolStatus::Failed`] are results.
    pub fn of(event: &ExecutionEvent) -> Self {
        match event.update() {
            ExecutionUpdate::Message(chunk) => match chunk.kind() {
                MessageKind::Thought => Self::Thinking,
                MessageKind::Text => Self::ModelText,
            },
            ExecutionUpdate::Tool(tool) => match tool.status() {
                Some(ToolStatus::Completed | ToolStatus::Failed) => Self::ToolResult,
                _ => Self::ToolCall,
            },
            ExecutionUpdate::PermissionRequested { .. } => Self::PermissionReview,
            ExecutionUpdate::PermissionCancelled(_) => Self::PermissionCancelled,
            ExecutionUpdate::DeadlineAnswered { .. } => Self::DeadlineAnswered,
            ExecutionUpdate::ReviewDeclined(_) => Self::ReviewDeclined,
            ExecutionUpdate::QuestionAsked { .. } => Self::Question,
            ExecutionUpdate::QuestionClosed { .. } => Self::QuestionClosed,
            ExecutionUpdate::Finished(_) => Self::Finished,
        }
    }
}

/// The proposed permission-gated call, borrowed from the review under judgement.
///
/// This is the review the harness is asking about. A tool the harness runs
/// without asking does not appear here. The arguments are borrowed, not copied,
/// and this value is not authority to run them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProposedToolCall<'a> {
    tool_id: &'a ToolCallId,
    name: &'a str,
    arguments_json: &'a str,
    initiation: &'a Initiation,
}

impl<'a> ProposedToolCall<'a> {
    pub(crate) fn new(
        tool_id: &'a ToolCallId,
        input: &'a ToolReviewInput,
        initiation: &'a Initiation,
    ) -> Self {
        Self {
            tool_id,
            name: input.name.as_str(),
            arguments_json: input.arguments_json.as_str(),
            initiation,
        }
    }

    /// Who started the turn this call belongs to, as its saved record says.
    ///
    /// It is read from the turn's record when the invocation begins. Only its
    /// own initiator's input is steered into a running turn; anyone else's
    /// waits for a turn of its own ([`Agent::steer`]). So every call in the
    /// turn is judged under this one initiation.
    ///
    /// [`Agent::steer`]: crate::application::agent_execution::agents::Agent::steer
    pub fn initiation(&self) -> &'a Initiation {
        self.initiation
    }

    /// Tool call the review is about.
    pub fn tool_id(&self) -> &'a ToolCallId {
        self.tool_id
    }

    /// Provider tool name, borrowed from the review input.
    pub fn name(&self) -> &'a str {
        self.name
    }

    /// Provider arguments as JSON text, borrowed from the review input.
    pub fn arguments_json(&self) -> &'a str {
        self.arguments_json
    }
}

/// Where a phrase sits relative to the included text sequences.
///
/// A hook that needs the whole turn treats [`Inconclusive`](Self::Inconclusive)
/// as a no. [`InvocationObservationWindow::is_complete`] is the same rule for
/// the window as a whole: an incomplete window is not proof the phrase is absent.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhrasePresence {
    /// The phrase occurs inside one included logical text sequence.
    Present,
    /// Every included text sequence is complete, and none contains the phrase.
    Absent,
    /// Text was left outside the window, and the phrase was not found in the
    /// included part. That miss does not prove the turn lacks the phrase.
    /// Treat this as a no when the decision needs the whole turn.
    Inconclusive,
}

impl PhrasePresence {
    /// The phrase was found in the included sequences.
    pub fn is_present(self) -> bool {
        matches!(self, Self::Present)
    }

    /// The included text is complete and does not contain the phrase.
    pub fn is_absent(self) -> bool {
        matches!(self, Self::Absent)
    }

    /// Text was omitted and the phrase was not found. Treat this as a no when
    /// the decision needs the whole turn.
    pub fn is_inconclusive(self) -> bool {
        matches!(self, Self::Inconclusive)
    }
}

/// Counts and omission flags for one evaluation.
///
/// The window embeds this so a copied suffix can keep the same facts without
/// walking the copy again. Debug prints these fields and nothing from the
/// events themselves.
#[derive(Clone, Copy)]
struct WindowFacts {
    start: usize,
    included_events: usize,
    included_payload_bytes: usize,
    omitted_prefix: bool,
    omitted_text: bool,
    incomplete_text: bool,
    excluded_oversized: bool,
}

/// Read-only suffix of one invocation's retained observations.
///
/// Construction borrows `events` and records how much of the suffix fits.
/// Copying the window copies that borrow, not the payloads. The borrow is valid
/// only while the caller holds the invocation's event slice, which for a hook
/// is the callback itself. A hook must not store the window or call back into
/// the session while the callback runs.
///
/// Every event kind is visible, including provider reasoning, and every kind
/// spends the same event and byte budget. Adjacent message chunks of the same
/// [`ObservationKind`] and message identity are one logical sequence.
/// [`Self::contains_phrase`] searches across those chunk boundaries. Two
/// messages that both lack an id share one identity, so they join; that can
/// only add a match, which fails closed for a hook that denies on a phrase.
/// A tool event, a question, a different kind, or an oversized event separates
/// sequences. An oversized event is omitted whole and does not end the walk.
#[derive(Clone, Copy)]
pub struct InvocationObservationWindow<'a> {
    events: BorrowedSuffix<'a>,
    facts: WindowFacts,
}

/// One borrowed suffix. A copied suffix stores omissions beside the events so
/// a provider tool id is never used as a hidden marker.
#[derive(Clone, Copy)]
enum BorrowedSuffix<'a> {
    Events(&'a [ExecutionEvent]),
    Copied(&'a [SuffixEntry]),
}

enum SuffixView<'a> {
    Event(&'a ExecutionEvent),
    Omitted,
}

enum SuffixEntry {
    Event(Box<ExecutionEvent>),
    Omitted,
}

impl fmt::Debug for InvocationObservationWindow<'_> {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("InvocationObservationWindow")
            .field("included_events", &self.facts.included_events)
            .field("included_payload_bytes", &self.facts.included_payload_bytes)
            .field("omitted_prefix", &self.facts.omitted_prefix)
            .field("omitted_text", &self.facts.omitted_text)
            .field("incomplete_text", &self.facts.incomplete_text)
            .field("excluded_oversized", &self.facts.excluded_oversized)
            .finish()
    }
}

impl<'a> InvocationObservationWindow<'a> {
    /// Maximum observations in one evaluation.
    ///
    /// ADR 0014 fixed this so a hook walks a turn suffix rather than the
    /// retained history. The history bound stays 262_144 events; this cap does
    /// not retain anything new. Thinking spends this budget like any other event.
    /// An oversized event that the walk skips also spends one of these slots,
    /// so the walk never sizes, and the copy never holds, more than this many events.
    pub const MAX_EVENTS: usize = 256;

    /// Maximum payload bytes of the included suffix.
    ///
    /// ADR 0014 fixed this evaluation budget at 64 KiB. An observation larger
    /// than the budget is omitted whole and does not end the walk. Its text is
    /// not sliced into the window. Thinking spends this budget like any other event.
    pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

    /// Borrow `events` and select the longest suffix that fits.
    ///
    /// The walk counts every event, including thoughts. An event whose own
    /// payload exceeds [`Self::MAX_PAYLOAD_BYTES`] is omitted whole, recorded
    /// as oversized, and left as a separator while the walk continues. It
    /// spends an event slot but no bytes. The
    /// walk records the omission flags as it goes. It allocates nothing.
    /// Payload bytes are the text or tool content a hook can read, not spare
    /// capacity of a message chunk.
    pub fn view(events: &'a [ExecutionEvent]) -> Self {
        let mut start = events.len();
        let mut included_events = 0usize;
        let mut included_payload_bytes = 0usize;
        let mut omitted_text = false;
        let mut excluded_oversized = false;
        // Every walked event spends a slot, an oversized one included.
        for (walked_events, (index, event)) in events.iter().enumerate().rev().enumerate() {
            let payload = payload_bytes(event);
            let oversized = payload > Self::MAX_PAYLOAD_BYTES;
            let next_bytes = if oversized {
                included_payload_bytes
            } else {
                included_payload_bytes.saturating_add(payload)
            };
            if walked_events == Self::MAX_EVENTS || next_bytes > Self::MAX_PAYLOAD_BYTES {
                if is_message(event) || prefix_may_hold_text(&events[..index]) {
                    omitted_text = true;
                }
                break;
            }
            if oversized {
                excluded_oversized = true;
                if is_message(event) {
                    omitted_text = true;
                }
                continue;
            }
            included_events += 1;
            included_payload_bytes = next_bytes;
            start = index;
        }
        let omitted_prefix = start > 0;
        if omitted_prefix && prefix_may_hold_text(&events[..start]) {
            omitted_text = true;
        }
        let facts = WindowFacts {
            start,
            included_events,
            included_payload_bytes,
            omitted_prefix,
            omitted_text,
            incomplete_text: sequence_was_cut(events, start),
            excluded_oversized,
        };
        Self {
            events: BorrowedSuffix::Events(events),
            facts,
        }
    }

    /// Observations that fit in the suffix.
    pub fn included_events(&self) -> usize {
        self.facts.included_events
    }

    /// Payload bytes of the included observations.
    pub fn included_payload_bytes(&self) -> usize {
        self.facts.included_payload_bytes
    }

    /// An observation older than the suffix was left out.
    pub fn omitted_prefix(&self) -> bool {
        self.facts.omitted_prefix
    }

    /// Message text, including thinking, was left outside the included suffix.
    ///
    /// Only the newest [`Self::MAX_EVENTS`] omitted events are checked for
    /// text, so the walk stays bounded. When more than that were omitted, the
    /// older ones are not looked at and this is `true`.
    pub fn omitted_text(&self) -> bool {
        self.facts.omitted_text
    }

    /// The oldest included text chunk continues a sequence that starts outside
    /// the suffix.
    pub fn incomplete_text(&self) -> bool {
        self.facts.incomplete_text
    }

    /// An observation was left out because its own payload exceeds
    /// [`Self::MAX_PAYLOAD_BYTES`]. The window does not contain a prefix of that
    /// observation. The walk continued past it.
    pub fn excluded_oversized(&self) -> bool {
        self.facts.excluded_oversized
    }

    /// The suffix includes every observation, with no cut text sequence and no
    /// oversized omission.
    ///
    /// A hook that needs the whole turn treats `false` as a no.
    pub fn is_complete(&self) -> bool {
        !self.facts.omitted_prefix && !self.facts.incomplete_text && !self.facts.excluded_oversized
    }

    /// Included observations in retained order.
    ///
    /// Oversized events are not yielded. An omission in a copied suffix is not
    /// an event, so a provider tool id cannot hide a real observation. Use
    /// [`ObservationKind::of`] on each item to tell thinking, model text, a
    /// tool call, and a tool result apart.
    pub fn iter(&self) -> impl Iterator<Item = &'a ExecutionEvent> + 'a {
        let start = self.facts.start;
        let events = self.events;
        let len = self.len();
        (start..len).filter_map(move |index| match slot_at(events, index) {
            SuffixView::Event(event) if event_included(event) => Some(event),
            _ => None,
        })
    }

    /// Search included logical text sequences for `phrase`.
    ///
    /// Adjacent chunks of the same kind and message identity are one sequence,
    /// so a phrase split across those chunks matches. Messages with no id share
    /// that identity and join. An empty phrase is [`PhrasePresence::Present`].
    /// Finding the phrase is [`PhrasePresence::Present`] even when the suffix is
    /// otherwise incomplete. A miss is [`PhrasePresence::Absent`] only when no
    /// text was left out. Otherwise a miss is [`PhrasePresence::Inconclusive`],
    /// which a hook treats as a no when it needs the whole turn.
    ///
    /// A needle longer than the included payload cannot match, and the search
    /// returns before allocating the needle's failure table. Otherwise the
    /// search allocates only that table.
    pub fn contains_phrase(&self, phrase: &str) -> PhrasePresence {
        if phrase.is_empty() {
            return PhrasePresence::Present;
        }
        if phrase.len() > self.facts.included_payload_bytes {
            return if self.facts.omitted_text {
                PhrasePresence::Inconclusive
            } else {
                PhrasePresence::Absent
            };
        }
        if self.phrase_in_sequences(phrase) {
            return PhrasePresence::Present;
        }
        if self.facts.omitted_text {
            PhrasePresence::Inconclusive
        } else {
            PhrasePresence::Absent
        }
    }

    fn phrase_in_sequences(&self, phrase: &str) -> bool {
        let needle = phrase.as_bytes();
        let fail = failure_table(needle);
        let mut matched = 0usize;
        let mut previous: Option<usize> = None;
        for index in self.facts.start..self.len() {
            let SuffixView::Event(event) = self.slot(index) else {
                previous = None;
                matched = 0;
                continue;
            };
            let Some(text) = searchable_text(event) else {
                previous = None;
                matched = 0;
                continue;
            };
            if previous.is_some_and(|prior| self.text_key_at(prior) != text_key(event)) {
                matched = 0;
            }
            previous = Some(index);
            for &byte in text.as_bytes() {
                while matched > 0 && byte != needle[matched] {
                    matched = fail[matched - 1];
                }
                if byte == needle[matched] {
                    matched += 1;
                    if matched == needle.len() {
                        return true;
                    }
                }
            }
        }
        false
    }

    fn len(&self) -> usize {
        match self.events {
            BorrowedSuffix::Events(events) => events.len(),
            BorrowedSuffix::Copied(entries) => entries.len(),
        }
    }

    fn slot(&self, index: usize) -> SuffixView<'a> {
        slot_at(self.events, index)
    }

    fn text_key_at(&self, index: usize) -> Option<(MessageKind, Option<&str>)> {
        match self.slot(index) {
            SuffixView::Event(event) => text_key(event),
            SuffixView::Omitted => None,
        }
    }
}

fn slot_at(events: BorrowedSuffix<'_>, index: usize) -> SuffixView<'_> {
    match events {
        BorrowedSuffix::Events(events) => SuffixView::Event(&events[index]),
        BorrowedSuffix::Copied(entries) => match &entries[index] {
            SuffixEntry::Event(event) => SuffixView::Event(event.as_ref()),
            SuffixEntry::Omitted => SuffixView::Omitted,
        },
    }
}

fn event_included(event: &ExecutionEvent) -> bool {
    payload_bytes(event) <= InvocationObservationWindow::MAX_PAYLOAD_BYTES
}

fn is_message(event: &ExecutionEvent) -> bool {
    matches!(event.update(), ExecutionUpdate::Message(_))
}

/// Whether an omitted `prefix` may hold message text.
///
/// Only the newest [`InvocationObservationWindow::MAX_EVENTS`] of it are
/// looked at, so the check stays inside the evaluation cap however long the
/// history is. Older events are not looked at and count as text.
fn prefix_may_hold_text(prefix: &[ExecutionEvent]) -> bool {
    let checked = prefix.len().min(InvocationObservationWindow::MAX_EVENTS);
    prefix.len() > checked || prefix[prefix.len() - checked..].iter().any(is_message)
}

fn searchable_text(event: &ExecutionEvent) -> Option<&str> {
    if !event_included(event) {
        return None;
    }
    match event.update() {
        ExecutionUpdate::Message(chunk) => Some(chunk.as_str()),
        _ => None,
    }
}

fn text_key(event: &ExecutionEvent) -> Option<(MessageKind, Option<&str>)> {
    match event.update() {
        ExecutionUpdate::Message(chunk) if event_included(event) => {
            Some((chunk.kind(), chunk.message_id()))
        }
        _ => None,
    }
}

fn sequence_was_cut(events: &[ExecutionEvent], start: usize) -> bool {
    let Some(index) = events
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, event)| text_key(event).map(|_| index))
    else {
        return false;
    };
    if index != start || index == 0 {
        return false;
    }
    let previous = &events[index - 1];
    event_included(previous) && text_key(previous) == text_key(&events[index])
}

/// The event's payload bytes, exact up to the window's byte cap. A larger
/// result says only that the event is oversized: a tool's elements and a
/// review's options are not walked past the cap. Every other part is a fixed
/// number of strings or bounded by admission, as a question's options are.
fn payload_bytes(event: &ExecutionEvent) -> usize {
    let limit = InvocationObservationWindow::MAX_PAYLOAD_BYTES;
    match event.update() {
        ExecutionUpdate::Finished(_) | ExecutionUpdate::QuestionClosed { .. } => 0,
        ExecutionUpdate::Message(chunk) => chunk.payload_bytes(),
        ExecutionUpdate::Tool(tool) => tool.payload_bytes_up_to(limit),
        ExecutionUpdate::PermissionRequested {
            input,
            observation,
            options,
            ..
        } => input
            .name
            .len()
            .saturating_add(input.arguments_json.len())
            .saturating_add(observation.payload_bytes_up_to(limit))
            .saturating_add(options.payload_bytes_up_to(limit)),
        ExecutionUpdate::PermissionCancelled(record) => record
            .input()
            .name
            .len()
            .saturating_add(record.input().arguments_json.len())
            .saturating_add(record.request().payload_bytes_up_to(limit)),
        ExecutionUpdate::ReviewDeclined(review) => review
            .decline()
            .declared()
            .map_or(0, str::len)
            .saturating_add(review.rule_bytes()),
        ExecutionUpdate::DeadlineAnswered { reason, .. } => reason.as_str().len(),
        ExecutionUpdate::QuestionAsked { question, .. } => question.payload_bytes(),
    }
}

fn failure_table(needle: &[u8]) -> Vec<usize> {
    let mut fail = vec![0usize; needle.len()];
    let mut length = 0usize;
    let mut index = 1usize;
    while index < needle.len() {
        if needle[index] == needle[length] {
            length += 1;
            fail[index] = length;
            index += 1;
        } else if length > 0 {
            length = fail[length - 1];
        } else {
            index += 1;
        }
    }
    fail
}

/// Owned suffix of one invocation, used when a hook must not hold the evidence lock.
///
/// The copy includes only events that fit. An oversized event inside the span
/// is an [`SuffixEntry::Omitted`] slot, not a fake tool event, so adjacent
/// text does not join and a provider tool id cannot hide an observation. The
/// omitted prefix is not copied, and an oversized body is not cloned.
pub(crate) struct EvaluationSuffix {
    entries: Vec<SuffixEntry>,
    facts: WindowFacts,
}

impl EvaluationSuffix {
    /// Copy the suffix [`InvocationObservationWindow::view`] would expose.
    pub(crate) fn capture(events: &[ExecutionEvent]) -> Self {
        let view = InvocationObservationWindow::view(events);
        let entries = events
            .iter()
            .skip(view.facts.start)
            .map(|event| {
                if payload_bytes(event) > InvocationObservationWindow::MAX_PAYLOAD_BYTES {
                    SuffixEntry::Omitted
                } else {
                    SuffixEntry::Event(Box::new(event.clone()))
                }
            })
            .collect();
        Self {
            entries,
            facts: WindowFacts {
                start: 0,
                ..view.facts
            },
        }
    }

    /// Window over this suffix. The omission facts are the ones capture recorded.
    pub(crate) fn window(&self) -> InvocationObservationWindow<'_> {
        InvocationObservationWindow {
            events: BorrowedSuffix::Copied(&self.entries),
            facts: self.facts,
        }
    }

    /// No events, and the omission flags say the turn was not captured.
    pub(crate) fn incomplete() -> Self {
        Self {
            entries: Vec::new(),
            facts: WindowFacts {
                start: 0,
                included_events: 0,
                included_payload_bytes: 0,
                omitted_prefix: true,
                omitted_text: true,
                incomplete_text: true,
                excluded_oversized: false,
            },
        }
    }
}

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/hooks/window.rs"]
mod window;
