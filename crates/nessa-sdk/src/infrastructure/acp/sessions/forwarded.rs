//! What an open's MCP stand-ins saw of the calls they forwarded, kept for the
//! tool calls the harness reports them under, and attached to those calls.
//!
//! ```text
//! mcp stand-in ──record(call id, arguments)──▶ ForwardedResults ◀──take_arguments── attach_arguments ◀── ACP worker
//!              (when the tools/call is accepted)                  (any update naming that MCP tool)
//!              ──record(call id, result)──────▶                 ◀──take──────────── attach_forwarded
//!              (before the harness is answered)                 (the call's completed update)
//! ```
//!
//! Arrows are calls. Claude's harness gives its model, and its ACP client,
//! `structuredContent` only as JSON text, in place of the result's own text,
//! so no ACP frame says which text was structured, and a call's arguments
//! reach the conversation view only when a permission request showed them.
//! The stand-in saw both: the arguments on the request, and the structured
//! result on the answer. The harness names each call it forwards by the id it
//! reports that call under over ACP; the stand-in reads that id
//! (`infrastructure::mcp`), and keeps what it saw here, in the store of the
//! open's grant ([`StandInGrant`](super::StandInGrant)). Taking it is the
//! shared ACP worker's, since a profile cannot see the open's grant; a harness
//! that names no call id (Codex, OpenCode) has nothing kept, and its takes
//! find nothing. The states and orderings are tabled in
//! `docs/design/mcp-connections.md` ("Forwarded results", "Forwarded
//! arguments"): the stand-in's rows (S1–S8, S11, A1–A6, A9) are tested in
//! `tests/infrastructure/mcp/forwarded.rs`, this store's (S9, S10, A7, A8, A15)
//! and W1–W8, A10–A13 in `tests/infrastructure/acp/sessions/forwarded.rs`.
#![deny(missing_docs)]

use crate::domain::agent_execution::tools::{
    McpCallArguments, ToolCallId, ToolCallUpdate, ToolContent, ToolStatus,
};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

/// The most results one grant keeps waiting for their tool call's completed
/// update. A result is taken as soon as that update is read, so what waits is
/// mostly results never taken (a call reported `failed`, or abandoned); past
/// this the oldest is dropped — and a burst of more than this many results
/// ahead of one call's completed update would drop that call's, which then
/// shows its text alone.
pub const MAX_FORWARDED_RESULTS: usize = 32;

/// One grant's forwarded results, by the harness's id for each call — a
/// [`ToolCallId`], so only an id the binding can report a call under is kept —
/// each with the configured server that answered it. Clones share them, and
/// two values are equal when they are the same store. Only the SDK writes and
/// takes them; a host can only compare them.
#[derive(Clone)]
pub struct ForwardedResults {
    kept: Arc<Mutex<Kept>>,
}

/// One grant's forwarded calls. Results and arguments wait apart, so a burst
/// of one cannot push the other out.
struct Kept {
    results: VecDeque<Forwarded>,
    arguments: VecDeque<ForwardedArguments>,
}

/// One forwarded result: the call it answered, the server that answered it,
/// and its structured content.
struct Forwarded {
    call: ToolCallId,
    server: Box<str>,
    result: ToolContent,
}

/// One forwarded call's arguments: the call, the server it was sent to, and
/// the encoded object.
struct ForwardedArguments {
    call: ToolCallId,
    server: Box<str>,
    arguments: McpCallArguments,
}

impl ForwardedResults {
    /// An empty store, for a new grant.
    pub(crate) fn new() -> Self {
        Self {
            kept: Arc::new(Mutex::new(Kept {
                results: VecDeque::new(),
                arguments: VecDeque::new(),
            })),
        }
    }

    /// Keep `result`, from the configured server `server`, for the call the
    /// harness names `call`, replacing one kept under the same id, and
    /// dropping the oldest past [`MAX_FORWARDED_RESULTS`].
    pub(crate) fn record(&self, call: ToolCallId, server: &str, result: ToolContent) {
        let mut kept = self.kept.lock().expect("forwarded results");
        kept.results.retain(|kept| kept.call != call);
        if kept.results.len() == MAX_FORWARDED_RESULTS {
            kept.results.pop_front();
        }
        kept.results.push_back(Forwarded {
            call,
            server: server.into(),
            result,
        });
    }

    /// Keep `arguments`, sent to the configured server `server`, for the call
    /// the harness names `call`, replacing any kept under the same id, and
    /// dropping the oldest past [`MAX_FORWARDED_RESULTS`].
    pub(crate) fn record_arguments(
        &self,
        call: ToolCallId,
        server: &str,
        arguments: McpCallArguments,
    ) {
        let mut kept = self.kept.lock().expect("forwarded results");
        kept.arguments.retain(|kept| kept.call != call);
        if kept.arguments.len() == MAX_FORWARDED_RESULTS {
            kept.arguments.pop_front();
        }
        kept.arguments.push_back(ForwardedArguments {
            call,
            server: server.into(),
            arguments,
        });
    }

    /// The result forwarded for the tool call the harness reports as
    /// `tool_call` to `server`, taken: a second take of the same id finds
    /// nothing, and one kept from another server is not this call's and
    /// stays.
    pub(crate) fn take(&self, tool_call: &ToolCallId, server: &str) -> Option<ToolContent> {
        let mut kept = self.kept.lock().expect("forwarded results");
        let index = kept
            .results
            .iter()
            .position(|item| item.call == *tool_call && *item.server == *server)?;
        kept.results.remove(index).map(|item| item.result)
    }

    /// The arguments forwarded for the tool call the harness reports as
    /// `tool_call` to `server`, taken: a second take of the same id finds
    /// nothing, and arguments kept for another server stay.
    pub(crate) fn take_arguments(
        &self,
        tool_call: &ToolCallId,
        server: &str,
    ) -> Option<McpCallArguments> {
        let mut kept = self.kept.lock().expect("forwarded results");
        let index = kept
            .arguments
            .iter()
            .position(|item| item.call == *tool_call && *item.server == *server)?;
        kept.arguments.remove(index).map(|item| item.arguments)
    }

    /// Drop arguments no update has taken. The next execution of this grant
    /// must not be told them: its running update can arrive before its own
    /// `tools/call`, and `tool-input` is sent at most once, so the previous
    /// execution's arguments would stick. Results stay; a cancelled call's
    /// result was already dropped when the call was cancelled.
    pub(crate) fn discard_arguments(&self) {
        self.kept
            .lock()
            .expect("forwarded results")
            .arguments
            .clear();
    }

    /// How many results are kept now.
    #[cfg(test)]
    pub(crate) fn len(&self) -> usize {
        self.kept.lock().expect("forwarded results").results.len()
    }

    /// How many argument sets are kept now.
    #[cfg(test)]
    pub(crate) fn arguments_len(&self) -> usize {
        self.kept.lock().expect("forwarded results").arguments.len()
    }
}
impl PartialEq for ForwardedResults {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.kept, &other.kept)
    }
}
impl Eq for ForwardedResults {}
impl std::fmt::Debug for ForwardedResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Results are a server's data: counted, not printed.
        let (results, arguments) = self
            .kept
            .lock()
            .map_or((0, 0), |kept| (kept.results.len(), kept.arguments.len()));
        f.debug_struct("ForwardedResults")
            .field("results", &results)
            .field("arguments", &arguments)
            .finish()
    }
}

/// `update`, with the result its stand-in forwarded for the call appended
/// after its content, when it is the `completed` update of a call naming an
/// MCP tool, carries content, and a result was forwarded under its id by the
/// server that tool names. The
/// result is taken then, and only then. An update without content (Claude's
/// PostToolUse frame) would replace the call's text with the result alone;
/// one before the end has no result yet; and a `failed` one is an `isError`
/// result (the pinned harness reports `status: is_error ? "failed" :
/// "completed"`), told as the harness told it.
pub(crate) fn attach_forwarded(
    update: ToolCallUpdate,
    forwarded: Option<&ForwardedResults>,
) -> ToolCallUpdate {
    let completed = matches!(update.status(), Some(ToolStatus::Completed));
    let (Some(forwarded), Some(content), true, Some(tool)) =
        (forwarded, update.content(), completed, update.mcp_tool())
    else {
        return update;
    };
    let Some(result) = forwarded.take(update.id(), tool.server()) else {
        return update;
    };
    let mut content = content.clone();
    content.push(result);
    update.with_content(content)
}

/// `update`, carrying the arguments its stand-in forwarded for the call, when
/// it names an MCP tool and arguments were forwarded under its id by the
/// server that tool names. Taken then, whatever the update's status: the
/// arguments were the request, so a `failed` call still has them, and a
/// running update that arrives after the request carries them before the
/// result. An update that arrives first finds nothing and leaves them for a
/// later one. A second update finds nothing; the view keeps what the first
/// carried.
pub(crate) fn attach_arguments(
    update: ToolCallUpdate,
    forwarded: Option<&ForwardedResults>,
) -> ToolCallUpdate {
    let (Some(forwarded), Some(tool)) = (forwarded, update.mcp_tool()) else {
        return update;
    };
    let Some(arguments) = forwarded.take_arguments(update.id(), tool.server()) else {
        return update;
    };
    update.with_mcp_arguments(arguments)
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/sessions/forwarded.rs"]
mod tests;
