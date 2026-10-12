//! Borrowed evaluation window over observations already retained for one invocation.
//!
//! ```text
//! InvocationRecord.events --> InvocationObservationWindow --> hook callback
//! PermissionRequested -----> ProposedToolCall -------------> same callback
//! ```
//!
//! Arrows are borrows. The window stores a start index into the existing slice.
//! It does not copy payloads, append history, or allocate on construction.
#![deny(missing_docs)]

use crate::application::agent_execution::executions::{ExecutionEvent, ExecutionUpdate};
use crate::application::agent_execution::tools::ToolReviewInput;
use crate::domain::agent_execution::{executions::MessageKind, tools::ToolCallId};

/// Whether a hook may read provider reasoning.
///
/// Handing thinking text to a hook discloses model output that the user-visible
/// transcript does not. The withheld variant is the default. A hook sees thoughts
/// only when its registration opts in.
///
/// ```
/// use nessa_sdk::application::agent_execution::hooks::ThinkingDisclosure;
/// assert_eq!(ThinkingDisclosure::default(), ThinkingDisclosure::Withheld);
/// ```
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum ThinkingDisclosure {
    /// Thoughts stay out of the window. They still separate adjacent text chunks.
    #[default]
    Withheld,
    /// Thoughts are visible and searchable, as their own channel.
    Disclosed,
}

/// The proposed permission-gated call, borrowed from the review that is about to
/// be admitted. This is not a copy of the arguments and not authority to run them.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ProposedToolCall<'a> {
    tool_id: &'a ToolCallId,
    name: &'a str,
    arguments_json: &'a str,
}

impl<'a> ProposedToolCall<'a> {
    /// Borrow the tool identity and the exact review input.
    ///
    /// `tool_id` is the call the review names. `input` is the name and arguments
    /// the adapter captured. Neither value is copied.
    pub fn new(tool_id: &'a ToolCallId, input: &'a ToolReviewInput) -> Self {
        Self {
            tool_id,
            name: input.name.as_str(),
            arguments_json: input.arguments_json.as_str(),
        }
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

/// Where a phrase sits relative to the visible text sequences.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PhrasePresence {
    /// The phrase occurs inside one included logical text sequence.
    Present,
    /// Every visible text sequence is fully included, and none contains the phrase.
    Absent,
    /// A visible text sequence was left outside the window, and the phrase was
    /// not found in the included part. That absence does not prove the turn
    /// lacks the phrase.
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

    /// The window cut or omitted visible text, and the phrase was not found.
    pub fn is_inconclusive(self) -> bool {
        matches!(self, Self::Inconclusive)
    }
}

/// Read-only suffix of one invocation's retained observations.
///
/// Construction borrows `events` and records how much of the suffix fits.
/// Copying the window copies that borrow, not the payloads. The borrow is valid
/// only while the caller holds the invocation's event slice, which for a hook
/// is the callback itself. A hook must not store the window or call back into
/// the session while the callback runs.
///
/// Adjacent message chunks of the same kind and message identity are one logical
/// sequence. [`Self::contains_phrase`] searches across those chunk boundaries.
/// A tool event, a question, or a withheld thought separates sequences. A chunk
/// cut by the suffix is not treated as proof that the phrase is absent.
#[derive(Clone, Copy, Debug)]
pub struct InvocationObservationWindow<'a> {
    events: &'a [ExecutionEvent],
    start: usize,
    disclosure: ThinkingDisclosure,
    included_events: usize,
    included_payload_bytes: usize,
    omitted_prefix: bool,
    omitted_text: bool,
    incomplete_text: bool,
    excluded_oversized: bool,
}

impl<'a> InvocationObservationWindow<'a> {
    /// Maximum visible observations in one evaluation.
    ///
    /// ADR 0014 fixed this so a hook walks a turn suffix rather than the
    /// retained history. The history bound stays 262_144 events; this cap does
    /// not retain anything new.
    pub const MAX_EVENTS: usize = 256;

    /// Maximum payload bytes of the visible suffix.
    ///
    /// ADR 0014 fixed this evaluation budget at 64 KiB. An observation larger
    /// than the budget is omitted whole. Its text is not sliced into the window.
    pub const MAX_PAYLOAD_BYTES: usize = 64 * 1024;

    /// Borrow `events` and select the longest visible suffix that fits.
    ///
    /// `disclosure` withholds thoughts unless the hook opted in. Withheld
    /// thoughts do not consume the event or byte budget. The walk is a single
    /// reverse pass and allocates nothing. Payload bytes are the text or tool
    /// content a hook can read, not allocator spare capacity of a message chunk.
    pub fn view(events: &'a [ExecutionEvent], disclosure: ThinkingDisclosure) -> Self {
        let mut start = events.len();
        let mut included_events = 0usize;
        let mut included_payload_bytes = 0usize;
        let mut excluded_oversized = false;
        for (index, event) in events.iter().enumerate().rev() {
            if !event_visible(event, disclosure) {
                continue;
            }
            let payload = payload_bytes(event);
            if payload > Self::MAX_PAYLOAD_BYTES {
                excluded_oversized = true;
                break;
            }
            let next_bytes = included_payload_bytes.saturating_add(payload);
            if included_events == Self::MAX_EVENTS || next_bytes > Self::MAX_PAYLOAD_BYTES {
                break;
            }
            included_events += 1;
            included_payload_bytes = next_bytes;
            start = index;
        }
        let omitted_prefix = events[..start]
            .iter()
            .any(|event| event_visible(event, disclosure));
        let omitted_text = events[..start]
            .iter()
            .any(|event| text_key(event, disclosure).is_some());
        let incomplete_text = sequence_was_cut(events, start, disclosure);
        Self {
            events,
            start,
            disclosure,
            included_events,
            included_payload_bytes,
            omitted_prefix,
            omitted_text,
            incomplete_text,
            excluded_oversized,
        }
    }

    /// Disclosure this window was built with.
    pub fn disclosure(&self) -> ThinkingDisclosure {
        self.disclosure
    }

    /// Visible observations that fit in the suffix.
    pub fn included_events(&self) -> usize {
        self.included_events
    }

    /// Payload bytes of the included observations.
    pub fn included_payload_bytes(&self) -> usize {
        self.included_payload_bytes
    }

    /// A visible observation older than the suffix was left out.
    pub fn omitted_prefix(&self) -> bool {
        self.omitted_prefix
    }

    /// Visible message or disclosed thought text was left outside the suffix.
    pub fn omitted_text(&self) -> bool {
        self.omitted_text
    }

    /// The oldest included text chunk continues a sequence that starts outside
    /// the suffix.
    pub fn incomplete_text(&self) -> bool {
        self.incomplete_text
    }

    /// An observation was left out because its own payload exceeds
    /// [`Self::MAX_PAYLOAD_BYTES`]. The window does not contain a prefix of that
    /// observation.
    pub fn excluded_oversized(&self) -> bool {
        self.excluded_oversized
    }

    /// The suffix includes every visible observation, with no cut text sequence
    /// and no oversized omission.
    pub fn is_complete(&self) -> bool {
        !self.omitted_prefix && !self.incomplete_text && !self.excluded_oversized
    }

    /// Visible observations in retained order.
    ///
    /// The iterator yields the same event references the invocation already
    /// holds. Withheld thoughts are skipped.
    pub fn iter(&self) -> impl Iterator<Item = &'a ExecutionEvent> + 'a {
        let disclosure = self.disclosure;
        self.events[self.start..]
            .iter()
            .filter(move |event| event_visible(event, disclosure))
    }

    /// Search included logical text sequences for `phrase`.
    ///
    /// Adjacent chunks of the same kind and message identity are one sequence,
    /// so a phrase split across those chunks matches. An empty phrase is
    /// [`PhrasePresence::Absent`]. Finding the phrase is [`PhrasePresence::Present`]
    /// even when the suffix is otherwise incomplete. A miss is
    /// [`PhrasePresence::Absent`] only when every visible text sequence is fully
    /// included. Otherwise a miss is [`PhrasePresence::Inconclusive`].
    ///
    /// The search does not copy observation text. It allocates the needle's
    /// failure table only.
    pub fn contains_phrase(&self, phrase: &str) -> PhrasePresence {
        if phrase.is_empty() {
            return PhrasePresence::Absent;
        }
        if self.phrase_in_sequences(phrase) {
            return PhrasePresence::Present;
        }
        if self.omitted_text || self.incomplete_text {
            PhrasePresence::Inconclusive
        } else {
            PhrasePresence::Absent
        }
    }

    fn metrics_from(&self, origin: usize) -> WindowMetrics {
        WindowMetrics {
            start: self.start - origin,
            disclosure: self.disclosure,
            included_events: self.included_events,
            included_payload_bytes: self.included_payload_bytes,
            omitted_prefix: self.omitted_prefix,
            omitted_text: self.omitted_text,
            incomplete_text: self.incomplete_text,
            excluded_oversized: self.excluded_oversized,
        }
    }

    fn from_metrics(events: &'a [ExecutionEvent], metrics: WindowMetrics) -> Self {
        Self {
            events,
            start: metrics.start,
            disclosure: metrics.disclosure,
            included_events: metrics.included_events,
            included_payload_bytes: metrics.included_payload_bytes,
            omitted_prefix: metrics.omitted_prefix,
            omitted_text: metrics.omitted_text,
            incomplete_text: metrics.incomplete_text,
            excluded_oversized: metrics.excluded_oversized,
        }
    }

    fn phrase_in_sequences(&self, phrase: &str) -> bool {
        let needle = phrase.as_bytes();
        let fail = failure_table(needle);
        let mut matched = 0usize;
        let mut previous: Option<usize> = None;
        for (index, event) in self.events.iter().enumerate().skip(self.start) {
            let Some(text) = visible_text(event, self.disclosure) else {
                previous = None;
                matched = 0;
                continue;
            };
            if let Some(prior) = previous {
                let adjacent = index == prior + 1
                    && text_key(&self.events[prior], self.disclosure)
                        == text_key(event, self.disclosure);
                if !adjacent {
                    matched = 0;
                }
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
}

fn event_visible(event: &ExecutionEvent, disclosure: ThinkingDisclosure) -> bool {
    match event.update() {
        ExecutionUpdate::Message(chunk) => {
            disclosure == ThinkingDisclosure::Disclosed || chunk.kind() == MessageKind::Text
        }
        _ => true,
    }
}

fn visible_text(event: &ExecutionEvent, disclosure: ThinkingDisclosure) -> Option<&str> {
    match event.update() {
        ExecutionUpdate::Message(chunk)
            if disclosure == ThinkingDisclosure::Disclosed || chunk.kind() == MessageKind::Text =>
        {
            Some(chunk.as_str())
        }
        _ => None,
    }
}

fn text_key(
    event: &ExecutionEvent,
    disclosure: ThinkingDisclosure,
) -> Option<(MessageKind, Option<&str>)> {
    match event.update() {
        ExecutionUpdate::Message(chunk)
            if disclosure == ThinkingDisclosure::Disclosed || chunk.kind() == MessageKind::Text =>
        {
            Some((chunk.kind(), chunk.message_id()))
        }
        _ => None,
    }
}

fn sequence_was_cut(
    events: &[ExecutionEvent],
    start: usize,
    disclosure: ThinkingDisclosure,
) -> bool {
    let Some(index) = events
        .iter()
        .enumerate()
        .skip(start)
        .find_map(|(index, event)| text_key(event, disclosure).map(|_| index))
    else {
        return false;
    };
    if index == 0 {
        return false;
    }
    text_key(&events[index - 1], disclosure) == text_key(&events[index], disclosure)
        && index - 1 < start
}

fn payload_bytes(event: &ExecutionEvent) -> usize {
    match event.update() {
        ExecutionUpdate::Finished(_) | ExecutionUpdate::QuestionClosed { .. } => 0,
        ExecutionUpdate::Message(chunk) => chunk.as_str().len(),
        ExecutionUpdate::Tool(tool) => tool.payload_bytes(),
        ExecutionUpdate::PermissionRequested {
            input,
            observation,
            options,
            ..
        } => input
            .name
            .len()
            .saturating_add(input.arguments_json.len())
            .saturating_add(observation.payload_bytes())
            .saturating_add(options.payload_bytes()),
        ExecutionUpdate::PermissionCancelled(record) => record
            .input()
            .name
            .len()
            .saturating_add(record.input().arguments_json.len())
            .saturating_add(record.request().payload_bytes()),
        ExecutionUpdate::ReviewDeclined(observation) => {
            observation.decline().declared().map_or(0, str::len)
        }
        ExecutionUpdate::QuestionAsked { question, .. } => question.payload_bytes(),
    }
}

#[derive(Clone, Copy)]
struct WindowMetrics {
    start: usize,
    disclosure: ThinkingDisclosure,
    included_events: usize,
    included_payload_bytes: usize,
    omitted_prefix: bool,
    omitted_text: bool,
    incomplete_text: bool,
    excluded_oversized: bool,
}

/// Owned suffix of one invocation, used at the turn boundary.
///
/// The pre-tool hook borrows the live record. Before and after hooks run
/// outside that borrow: copying the bounded suffix drops the evidence lock
/// before any callback, so a hook cannot stall session admission. The copy
/// starts at the older of the withheld and disclosed windows and keeps their
/// omission facts. It does not copy the omitted prefix and does not allocate
/// on the pre-tool path.
pub(crate) struct EvaluationSuffix {
    events: Vec<ExecutionEvent>,
    withheld: WindowMetrics,
    disclosed: WindowMetrics,
}

impl EvaluationSuffix {
    /// Copy the suffix `view` would expose for either disclosure.
    pub(crate) fn capture(events: &[ExecutionEvent]) -> Self {
        let withheld = InvocationObservationWindow::view(events, ThinkingDisclosure::Withheld);
        let disclosed = InvocationObservationWindow::view(events, ThinkingDisclosure::Disclosed);
        let origin = withheld.start.min(disclosed.start);
        Self {
            events: events[origin..].to_vec(),
            withheld: withheld.metrics_from(origin),
            disclosed: disclosed.metrics_from(origin),
        }
    }

    /// Window for `disclosure`, borrowing this suffix.
    pub(crate) fn window(&self, disclosure: ThinkingDisclosure) -> InvocationObservationWindow<'_> {
        let metrics = match disclosure {
            ThinkingDisclosure::Withheld => self.withheld,
            ThinkingDisclosure::Disclosed => self.disclosed,
        };
        InvocationObservationWindow::from_metrics(&self.events, metrics)
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

#[cfg(test)]
#[path = "../../../../tests/application/agent_execution/hooks/window.rs"]
mod window;
