//! The results an open's MCP stand-ins forwarded to their harness, kept for
//! the tool calls the harness reports them under, and attached to those calls.
//!
//! ```text
//! mcp stand-in ──record(call id, result)──▶ ForwardedResults ◀──take── attach_forwarded ◀── ACP worker
//!              (before the harness is answered)              (the call's completed update)
//! ```
//!
//! Arrows are calls. Claude's harness gives its model, and its ACP client,
//! `structuredContent` only as JSON text, in place of the result's own text,
//! so no ACP frame says which text was structured. The stand-in saw the
//! object, and the harness names each call it forwards by the id it reports
//! that call under over ACP; the stand-in reads that id
//! (`infrastructure::mcp`), and keeps the result here, in the store of the
//! open's grant ([`StandInGrant`](super::StandInGrant)). Taking it is the
//! shared ACP worker's, since a profile cannot see the open's grant; a harness
//! that names no call id (Codex, OpenCode) has nothing kept, and its takes
//! find nothing. The states and orderings are tabled in
//! `docs/design/mcp-connections.md` ("Forwarded results"): the stand-in's
//! rows (S1–S8, S11) are tested in `tests/infrastructure/mcp/forwarded.rs`,
//! this store's (S9, S10) and W1–W7 in
//! `tests/infrastructure/acp/sessions/forwarded.rs`.
#![deny(missing_docs)]

use crate::domain::agent_execution::tools::{ToolCallUpdate, ToolContent, ToolStatus};
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

/// One grant's forwarded results, by the harness's id for each call. Clones
/// share them, and two values are equal when they are the same store. Only
/// the SDK writes and takes them; a host can only compare them.
#[derive(Clone)]
pub struct ForwardedResults {
    results: Arc<Mutex<VecDeque<(String, ToolContent)>>>,
}
impl ForwardedResults {
    /// An empty store, for a new grant.
    pub(crate) fn new() -> Self {
        Self {
            results: Arc::default(),
        }
    }

    /// Keep `result` for the call the harness names `call`, replacing one kept
    /// under the same id, and dropping the oldest past
    /// [`MAX_FORWARDED_RESULTS`].
    pub(crate) fn record(&self, call: String, result: ToolContent) {
        let mut results = self.results.lock().expect("forwarded results");
        results.retain(|(kept, _)| *kept != call);
        if results.len() == MAX_FORWARDED_RESULTS {
            results.pop_front();
        }
        results.push_back((call, result));
    }

    /// The result forwarded for the tool call the harness reports as
    /// `tool_call`, taken: a second take of the same id finds nothing.
    pub(crate) fn take(&self, tool_call: &str) -> Option<ToolContent> {
        let mut results = self.results.lock().expect("forwarded results");
        let index = results.iter().position(|(call, _)| call == tool_call)?;
        results.remove(index).map(|(_, result)| result)
    }

    /// How many results are kept now.
    #[cfg(all(test, unix))]
    pub(crate) fn len(&self) -> usize {
        self.results.lock().expect("forwarded results").len()
    }
}
impl PartialEq for ForwardedResults {
    fn eq(&self, other: &Self) -> bool {
        Arc::ptr_eq(&self.results, &other.results)
    }
}
impl Eq for ForwardedResults {}
impl std::fmt::Debug for ForwardedResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Results are a server's data: counted, not printed.
        let kept = self.results.lock().map_or(0, |results| results.len());
        f.debug_struct("ForwardedResults")
            .field("kept", &kept)
            .finish()
    }
}

/// `update`, with the result its stand-in forwarded for the call appended
/// after its content, when it is the `completed` update of a call naming an
/// MCP tool, carries content, and a result was forwarded under its id. The
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
    let (Some(forwarded), Some(content), true, true) = (
        forwarded,
        update.content(),
        completed,
        update.mcp_tool().is_some(),
    ) else {
        return update;
    };
    let Some(result) = forwarded.take(update.id().as_str()) else {
        return update;
    };
    let mut content = content.clone();
    content.push(result);
    update.with_content(content)
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/sessions/forwarded.rs"]
mod tests;
