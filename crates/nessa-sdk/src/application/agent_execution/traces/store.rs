//! Local span store and the exporter plug a later sender implements.
#![deny(missing_docs)]

use super::span::{AttributeValue, GenAiSpan};
use std::cell::RefCell;
use std::collections::{HashMap, VecDeque};
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::{Arc, Mutex, MutexGuard};

thread_local! {
    /// Stores currently forwarding to an exporter on this thread.
    /// A re-entrant export still records the span and does not forward again.
    static EXPORTING: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

const CAPACITY: usize = 2048;
// Captured message and tool text retained across finished and withheld spans.
// Twice the 4 MiB user-message limit.
const CONTENT_BUDGET: usize = 8 * 1024 * 1024;
const CONTENT_KEYS: [&str; 4] = [
    "gen_ai.input.messages",
    "gen_ai.output.messages",
    "gen_ai.tool.call.arguments",
    "gen_ai.tool.call.result",
];
const OP_AGENT: &str = "invoke_agent";
const CONVERSATION: &str = "gen_ai.conversation.id";
const EXECUTION: &str = "nessa.execution.id";
const GRANT_ID: &str = "nessa.grant.id";
const GRANT_REVISION: &str = "nessa.grant.policy_revision";
const GRANT_TOOLS: &str = "nessa.grant.tools_checked";
const HOOK_EVENT: &str = "nessa.hook.event";
const HOOK_VERDICT: &str = "nessa.hook.verdict";
const HOOK_COUNT: &str = "nessa.hook.count";
const SUBAGENT_ID: &str = "nessa.subagent.id";
const SUBAGENT_PARENT: &str = "nessa.subagent.parent";

/// Whether message text, tool arguments, and tool results are copied onto spans.
///
/// Off is the default. A caller that wants the GenAI content attributes selects
/// [`Self::on`] when it builds the store. The recorder is the check that drops
/// the text; a worker that also skips the copy is only saving the allocation.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct ContentCapture {
    enabled: bool,
}

impl ContentCapture {
    /// Do not copy message text, tool arguments, or tool results.
    pub const fn off() -> Self {
        Self { enabled: false }
    }

    /// Copy message text, tool arguments, and tool results onto the spans.
    ///
    /// Thought text is still omitted. This is the convention's opt-in.
    pub const fn on() -> Self {
        Self { enabled: true }
    }

    /// Whether content is copied.
    pub const fn enabled(self) -> bool {
        self.enabled
    }
}

/// Receives a finished span. Implementations must not fail the turn.
///
/// A later OTLP sender implements this and is installed with
/// [`LocalTraceStore::set_exporter`]. Nothing in this module opens a connection.
pub trait SpanExporter: Send + Sync {
    /// Accept one finished span. Losing it is acceptable; failing the turn is not.
    ///
    /// [`LocalTraceStore`] catches a panic from an installed exporter and keeps
    /// the span it already stored.
    fn export(&self, span: &GenAiSpan);
}

/// Sends each span to every exporter, in order.
pub struct ExporterList {
    exporters: Vec<Arc<dyn SpanExporter>>,
}

impl ExporterList {
    /// Fan out to `exporters`. An empty list drops every span.
    pub fn new(exporters: Vec<Arc<dyn SpanExporter>>) -> Self {
        Self { exporters }
    }
}

impl SpanExporter for ExporterList {
    fn export(&self, span: &GenAiSpan) {
        for exporter in &self.exporters {
            exporter.export(span);
        }
    }
}

#[derive(Clone, PartialEq, Eq, Hash)]
struct NoteKey {
    conversation_id: String,
    execution_id: String,
}

impl NoteKey {
    fn new(conversation_id: &str, execution_id: &str) -> Self {
        Self {
            conversation_id: conversation_id.to_owned(),
            execution_id: execution_id.to_owned(),
        }
    }
}

struct Pending {
    grant_id: Option<String>,
    policy_revision: Option<u64>,
    tools_checked: Option<bool>,
    hook_event: Option<String>,
    hook_verdict: Option<String>,
    hook_count: u64,
    subagent_id: Option<String>,
    subagent_parent: Option<String>,
}

impl Pending {
    fn new() -> Self {
        Self {
            grant_id: None,
            policy_revision: None,
            tools_checked: None,
            hook_event: None,
            hook_verdict: None,
            hook_count: 0,
            subagent_id: None,
            subagent_parent: None,
        }
    }
}

struct Inner {
    spans: VecDeque<GenAiSpan>,
    pending: HashMap<NoteKey, Pending>,
    pending_order: VecDeque<NoteKey>,
    /// Child spans of turns whose agent span has not closed, oldest first.
    withheld: VecDeque<GenAiSpan>,
    /// Bytes of captured content attributes on `spans` and `withheld`.
    content_bytes: usize,
    extra: Option<Arc<dyn SpanExporter>>,
    capture: ContentCapture,
}

/// Finished spans for the conversations this process has run, and the place a
/// later exporter is attached.
///
/// The store keeps the most recent 2048 finished spans, and the same number of
/// child spans from turns that have not closed. Older spans are dropped one at
/// a time. Captured message and tool text is capped at 8 MiB across both.
/// Older finished spans go first. The last finished span stays, and open-turn
/// children go after that. A span that exceeds the budget on its own is stored
/// without the oversized text.
/// A turn still running is absent until its agent span closes. Reads do not
/// block on the provider.
pub struct LocalTraceStore {
    inner: Mutex<Inner>,
}

impl LocalTraceStore {
    /// A store with content capture off and no extra exporter.
    pub fn new() -> Arc<Self> {
        Self::with_capture(ContentCapture::off())
    }

    /// A store that copies content only when `capture` is [`ContentCapture::on`].
    pub fn with_capture(capture: ContentCapture) -> Arc<Self> {
        Arc::new(Self {
            inner: Mutex::new(Inner {
                spans: VecDeque::new(),
                pending: HashMap::new(),
                pending_order: VecDeque::new(),
                withheld: VecDeque::new(),
                content_bytes: 0,
                extra: None,
                capture,
            }),
        })
    }

    /// The capture choice this store was built with.
    pub fn capture(&self) -> ContentCapture {
        self.lock().capture
    }

    /// Finished spans whose `gen_ai.conversation.id` is `conversation_id`, in
    /// the order they closed.
    ///
    /// A view reads this. An open turn is not included. An unknown conversation
    /// returns an empty list.
    pub fn read(&self, conversation_id: &str) -> Vec<GenAiSpan> {
        self.lock()
            .spans
            .iter()
            .filter(|span| span.string(CONVERSATION) == Some(conversation_id))
            .cloned()
            .collect()
    }

    /// Record that `execution_id` in `conversation_id` was started under `grant_id`
    /// at `policy_revision`.
    ///
    /// `tools_checked` is whether the host checked the turn's tool calls against
    /// that grant. The note is applied to the agent span when it is exported,
    /// or to the span already stored when the note arrives later. Notes for two
    /// conversations do not share an execution id. Unconsumed notes are capped
    /// at the same 2048 entries as finished spans; the oldest is dropped.
    pub fn note_grant(
        &self,
        conversation_id: &str,
        execution_id: &str,
        grant_id: &str,
        policy_revision: u64,
        tools_checked: bool,
    ) {
        let mut inner = self.lock();
        if let Some(span) = agent_span(&mut inner.spans, conversation_id, execution_id) {
            let updated = grant(span.clone(), grant_id, policy_revision, tools_checked);
            *span = updated;
            return;
        }
        let pending = hold_pending(&mut inner, conversation_id, execution_id);
        pending.grant_id = Some(grant_id.to_owned());
        pending.policy_revision = Some(policy_revision);
        pending.tools_checked = Some(tools_checked);
    }

    /// Record a hook verdict for `execution_id` in `conversation_id`.
    ///
    /// The latest `event` and `verdict` are stored, and `nessa.hook.count` is
    /// how many notes have been applied to that turn. The conversation scopes
    /// the note the same way [`Self::note_grant`] does.
    pub fn note_hook(&self, conversation_id: &str, execution_id: &str, event: &str, verdict: &str) {
        let mut inner = self.lock();
        if let Some(span) = agent_span(&mut inner.spans, conversation_id, execution_id) {
            let updated = hook_note(span, event, verdict, 1);
            *span = updated;
            return;
        }
        let pending = hold_pending(&mut inner, conversation_id, execution_id);
        pending.hook_event = Some(event.to_owned());
        pending.hook_verdict = Some(verdict.to_owned());
        pending.hook_count = pending.hook_count.saturating_add(1);
    }

    /// Record that `execution_id` in `conversation_id` spawned subagent `child_id`
    /// under `parent_id`.
    ///
    /// A later note for the same conversation and execution replaces the pair.
    pub fn note_subagent(
        &self,
        conversation_id: &str,
        execution_id: &str,
        child_id: &str,
        parent_id: &str,
    ) {
        let mut inner = self.lock();
        if let Some(span) = agent_span(&mut inner.spans, conversation_id, execution_id) {
            let updated = subagent(span, child_id, parent_id);
            *span = updated;
            return;
        }
        let pending = hold_pending(&mut inner, conversation_id, execution_id);
        pending.subagent_id = Some(child_id.to_owned());
        pending.subagent_parent = Some(parent_id.to_owned());
    }

    /// Drop an unconsumed grant note for `execution_id` in `conversation_id`.
    ///
    /// Called when an invocation that recorded a grant ends without an agent
    /// span consuming it. Hook and subagent notes on the same execution stay.
    /// A grant already applied to a stored span is left alone.
    pub fn retire(&self, conversation_id: &str, execution_id: &str) {
        let mut inner = self.lock();
        let key = NoteKey::new(conversation_id, execution_id);
        let Some(pending) = inner.pending.get_mut(&key) else {
            return;
        };
        pending.grant_id = None;
        pending.policy_revision = None;
        pending.tools_checked = None;
        if pending.hook_event.is_none() && pending.subagent_id.is_none() {
            inner.pending.remove(&key);
            inner.pending_order.retain(|existing| existing != &key);
        }
    }

    /// Install the exporter that receives each span after this store has
    /// finished it.
    ///
    /// This is the plug for a later user-chosen OTLP endpoint: the sender
    /// implements [`SpanExporter`] and is set here. No endpoint is read now.
    /// The store must not be set as its own exporter.
    pub fn set_exporter(&self, exporter: Arc<dyn SpanExporter>) {
        self.lock().extra = Some(exporter);
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
    }
}

impl SpanExporter for LocalTraceStore {
    fn export(&self, span: &GenAiSpan) {
        let me = self as *const Self as usize;
        let (finished, extra) = {
            let mut inner = self.lock();
            let finished = finish_span(&mut inner, span.clone());
            if finished.operation() == OP_AGENT {
                publish_turn(&mut inner, &finished);
            } else {
                withhold(&mut inner, finished.clone());
            }
            let reentered = EXPORTING.with(|stack| stack.borrow().contains(&me));
            let extra = if reentered { None } else { inner.extra.clone() };
            (finished, extra)
        };
        if let Some(extra) = extra {
            EXPORTING.with(|stack| stack.borrow_mut().push(me));
            let _frame = ExportFrame;
            // The span is already stored. An exporter panic must not fail the turn.
            let _ = catch_unwind(AssertUnwindSafe(|| extra.export(&finished)));
        }
    }
}

struct ExportFrame;

impl Drop for ExportFrame {
    fn drop(&mut self) {
        EXPORTING.with(|stack| {
            stack.borrow_mut().pop();
        });
    }
}

fn finish_span(inner: &mut Inner, span: GenAiSpan) -> GenAiSpan {
    if span.operation() != OP_AGENT {
        return span;
    }
    let Some(execution_id) = span.string(EXECUTION).map(str::to_owned) else {
        return span;
    };
    let Some(conversation_id) = span.string(CONVERSATION).map(str::to_owned) else {
        return span;
    };
    let key = NoteKey::new(&conversation_id, &execution_id);
    let Some(pending) = inner.pending.remove(&key) else {
        return span;
    };
    inner.pending_order.retain(|existing| existing != &key);
    let mut span = span;
    if let (Some(grant_id), Some(revision), Some(checked)) = (
        pending.grant_id.as_deref(),
        pending.policy_revision,
        pending.tools_checked,
    ) {
        span = grant(span, grant_id, revision, checked);
    }
    if let (Some(event), Some(verdict)) = (
        pending.hook_event.as_deref(),
        pending.hook_verdict.as_deref(),
    ) {
        let count = i64::try_from(pending.hook_count).unwrap_or(i64::MAX);
        span = hook_note(&span, event, verdict, count);
    }
    if let (Some(child), Some(parent)) = (
        pending.subagent_id.as_deref(),
        pending.subagent_parent.as_deref(),
    ) {
        span = subagent(&span, child, parent);
    }
    span
}

fn hold_pending<'a>(
    inner: &'a mut Inner,
    conversation_id: &str,
    execution_id: &str,
) -> &'a mut Pending {
    let key = NoteKey::new(conversation_id, execution_id);
    if !inner.pending.contains_key(&key) {
        while inner.pending_order.len() >= CAPACITY {
            if let Some(old) = inner.pending_order.pop_front() {
                inner.pending.remove(&old);
            }
        }
        inner.pending_order.push_back(key.clone());
        inner.pending.insert(key.clone(), Pending::new());
    }
    inner
        .pending
        .get_mut(&key)
        .expect("pending note was just inserted")
}

fn withhold(inner: &mut Inner, span: GenAiSpan) {
    remember(inner, span, true);
}

fn publish_turn(inner: &mut Inner, agent: &GenAiSpan) {
    let trace_id = agent.trace_id();
    let mut kept = VecDeque::new();
    let mut children = VecDeque::new();
    for span in inner.withheld.drain(..) {
        if span.trace_id() == trace_id {
            children.push_back(span);
        } else {
            kept.push_back(span);
        }
    }
    inner.withheld = kept;
    inner.spans.extend(children);
    remember(inner, agent.clone(), false);
}

fn remember(inner: &mut Inner, span: GenAiSpan, withheld: bool) {
    let span = fit_captured(span);
    inner.content_bytes = inner.content_bytes.saturating_add(captured_bytes(&span));
    if withheld {
        inner.withheld.push_back(span);
    } else {
        inner.spans.push_back(span);
    }
    enforce_bounds(inner);
}

fn fit_captured(mut span: GenAiSpan) -> GenAiSpan {
    while captured_bytes(&span) > CONTENT_BUDGET {
        let Some(key) = largest_content_key(&span) else {
            break;
        };
        span = span.without_attribute(key);
    }
    span
}

fn captured_bytes(span: &GenAiSpan) -> usize {
    CONTENT_KEYS.iter().fold(0, |total, key| {
        total.saturating_add(span.string(key).map(str::len).unwrap_or(0))
    })
}

fn largest_content_key(span: &GenAiSpan) -> Option<&'static str> {
    let mut best: Option<(&str, usize)> = None;
    for key in CONTENT_KEYS {
        let len = span.string(key).map(str::len).unwrap_or(0);
        if len == 0 {
            continue;
        }
        match best {
            Some((_, best_len)) if len <= best_len => {}
            _ => best = Some((key, len)),
        }
    }
    best.map(|(key, _)| key)
}

fn enforce_bounds(inner: &mut Inner) {
    while inner.spans.len() > CAPACITY {
        drop_front(&mut inner.spans, &mut inner.content_bytes);
    }
    while inner.withheld.len() > CAPACITY {
        drop_front(&mut inner.withheld, &mut inner.content_bytes);
    }
    while inner.content_bytes > CONTENT_BUDGET {
        if inner.spans.len() > 1 {
            drop_front(&mut inner.spans, &mut inner.content_bytes);
        } else if !inner.withheld.is_empty() {
            drop_front(&mut inner.withheld, &mut inner.content_bytes);
        } else {
            break;
        }
    }
}

fn drop_front(spans: &mut VecDeque<GenAiSpan>, content_bytes: &mut usize) {
    if let Some(span) = spans.pop_front() {
        *content_bytes = content_bytes.saturating_sub(captured_bytes(&span));
    }
}

fn grant(span: GenAiSpan, grant_id: &str, policy_revision: u64, tools_checked: bool) -> GenAiSpan {
    let mut span = span
        .with_attribute(GRANT_ID, AttributeValue::String(grant_id.to_owned()))
        .with_attribute(GRANT_TOOLS, AttributeValue::Bool(tools_checked));
    if let Ok(revision) = i64::try_from(policy_revision) {
        span = span.with_attribute(GRANT_REVISION, AttributeValue::Int(revision));
    }
    span
}

fn hook_note(span: &GenAiSpan, event: &str, verdict: &str, added: i64) -> GenAiSpan {
    let count = span.int(HOOK_COUNT).unwrap_or(0).saturating_add(added);
    span.clone()
        .with_attribute(HOOK_EVENT, AttributeValue::String(event.to_owned()))
        .with_attribute(HOOK_VERDICT, AttributeValue::String(verdict.to_owned()))
        .with_attribute(HOOK_COUNT, AttributeValue::Int(count))
}

fn subagent(span: &GenAiSpan, child_id: &str, parent_id: &str) -> GenAiSpan {
    span.clone()
        .with_attribute(SUBAGENT_ID, AttributeValue::String(child_id.to_owned()))
        .with_attribute(
            SUBAGENT_PARENT,
            AttributeValue::String(parent_id.to_owned()),
        )
}

fn agent_span<'a>(
    spans: &'a mut VecDeque<GenAiSpan>,
    conversation_id: &str,
    execution_id: &str,
) -> Option<&'a mut GenAiSpan> {
    spans.iter_mut().rev().find(|span| {
        span.operation() == OP_AGENT
            && span.string(CONVERSATION) == Some(conversation_id)
            && span.string(EXECUTION) == Some(execution_id)
    })
}
