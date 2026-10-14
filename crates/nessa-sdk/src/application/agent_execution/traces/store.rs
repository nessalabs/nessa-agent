//! Local span store and the exporter plug a later sender implements.
#![deny(missing_docs)]

use super::span::{AttributeValue, GenAiSpan};
use std::collections::{HashMap, VecDeque};
use std::sync::{Arc, Mutex, MutexGuard};

const CAPACITY: usize = 2048;
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

struct HookNote {
    event: String,
    verdict: String,
}

struct Pending {
    grant_id: Option<String>,
    policy_revision: Option<u64>,
    tools_checked: Option<bool>,
    hooks: Vec<HookNote>,
    subagent_id: Option<String>,
    subagent_parent: Option<String>,
}

impl Pending {
    fn new() -> Self {
        Self {
            grant_id: None,
            policy_revision: None,
            tools_checked: None,
            hooks: Vec::new(),
            subagent_id: None,
            subagent_parent: None,
        }
    }
}

struct Inner {
    spans: VecDeque<GenAiSpan>,
    pending: HashMap<String, Pending>,
    extra: Option<Arc<dyn SpanExporter>>,
    capture: ContentCapture,
    busy: bool,
}

/// Finished spans for the conversations this process has run, and the place a
/// later exporter is attached.
///
/// The store keeps the most recent 2048 finished spans. Older spans are dropped.
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
                extra: None,
                capture,
                busy: false,
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

    /// Record that `execution_id` was started under `grant_id` at `policy_revision`.
    ///
    /// `tools_checked` is whether the host checked the turn's tool calls against
    /// that grant. The note is applied to the agent span when it is exported,
    /// or to the span already stored when the note arrives later.
    pub fn note_grant(
        &self,
        execution_id: &str,
        grant_id: &str,
        policy_revision: u64,
        tools_checked: bool,
    ) {
        let mut inner = self.lock();
        if let Some(span) = agent_span(&mut inner.spans, execution_id) {
            let updated = grant(span.clone(), grant_id, policy_revision, tools_checked);
            *span = updated;
            return;
        }
        let pending = inner
            .pending
            .entry(execution_id.to_owned())
            .or_insert_with(Pending::new);
        pending.grant_id = Some(grant_id.to_owned());
        pending.policy_revision = Some(policy_revision);
        pending.tools_checked = Some(tools_checked);
    }

    /// Record a hook verdict for `execution_id`.
    ///
    /// The latest `event` and `verdict` are stored, and `nessa.hook.count` is
    /// how many notes have been applied to that turn.
    pub fn note_hook(&self, execution_id: &str, event: &str, verdict: &str) {
        let mut inner = self.lock();
        if let Some(span) = agent_span(&mut inner.spans, execution_id) {
            let updated = hook_note(span, event, verdict, 1);
            *span = updated;
            return;
        }
        inner
            .pending
            .entry(execution_id.to_owned())
            .or_insert_with(Pending::new)
            .hooks
            .push(HookNote {
                event: event.to_owned(),
                verdict: verdict.to_owned(),
            });
    }

    /// Record that `execution_id` spawned subagent `child_id` under `parent_id`.
    ///
    /// A later note for the same execution replaces the pair.
    pub fn note_subagent(&self, execution_id: &str, child_id: &str, parent_id: &str) {
        let mut inner = self.lock();
        if let Some(span) = agent_span(&mut inner.spans, execution_id) {
            let updated = subagent(span, child_id, parent_id);
            *span = updated;
            return;
        }
        let pending = inner
            .pending
            .entry(execution_id.to_owned())
            .or_insert_with(Pending::new);
        pending.subagent_id = Some(child_id.to_owned());
        pending.subagent_parent = Some(parent_id.to_owned());
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
        let (finished, extra) = {
            let mut inner = self.lock();
            if inner.busy {
                return;
            }
            let finished = finish_span(&mut inner, span.clone());
            inner.spans.push_back(finished.clone());
            while inner.spans.len() > CAPACITY {
                inner.spans.pop_front();
            }
            let extra = inner.extra.clone();
            if extra.is_some() {
                inner.busy = true;
            }
            (finished, extra)
        };
        if let Some(extra) = extra {
            let _guard = Busy(self);
            extra.export(&finished);
        }
    }
}

struct Busy<'a>(&'a LocalTraceStore);

impl Drop for Busy<'_> {
    fn drop(&mut self) {
        self.0.lock().busy = false;
    }
}

fn finish_span(inner: &mut Inner, span: GenAiSpan) -> GenAiSpan {
    if span.operation() != OP_AGENT {
        return span;
    }
    let Some(execution_id) = span.string(EXECUTION).map(str::to_owned) else {
        return span;
    };
    let Some(pending) = inner.pending.remove(&execution_id) else {
        return span;
    };
    let mut span = span;
    if let (Some(grant_id), Some(revision), Some(checked)) = (
        pending.grant_id.as_deref(),
        pending.policy_revision,
        pending.tools_checked,
    ) {
        span = grant(span, grant_id, revision, checked);
    }
    if let Some(note) = pending.hooks.last() {
        let count = i64::try_from(pending.hooks.len()).unwrap_or(i64::MAX);
        span = hook_note(&span, &note.event, &note.verdict, count);
    }
    if let (Some(child), Some(parent)) = (
        pending.subagent_id.as_deref(),
        pending.subagent_parent.as_deref(),
    ) {
        span = subagent(&span, child, parent);
    }
    span
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
    execution_id: &str,
) -> Option<&'a mut GenAiSpan> {
    spans
        .iter_mut()
        .rev()
        .find(|span| span.operation() == OP_AGENT && span.string(EXECUTION) == Some(execution_id))
}
