//! The results a grant's stand-ins forwarded to their harness, kept for the
//! tool calls the harness reports them under.
//!
//! ```text
//! stand_in ──record(call id, result)──▶ ForwardedResults ◀──take(tool call id)── ACP worker
//!                (before the harness is answered)            (the call's terminal update)
//! ```
//!
//! Arrows are calls. Claude's harness gives its model, and its ACP client,
//! `structuredContent` only as JSON text, so the object a server returned is
//! nowhere in what the harness reports. The stand-in saw it, and the harness
//! names the call it forwarded (`_meta["claudecode/toolUseId"]`) by the id it
//! reports that call under over ACP. The states and orderings are tabled in
//! `docs/design/mcp-connections.md` ("Forwarded results"), each row tested in
//! `tests/infrastructure/mcp/forwarded.rs`.
#![deny(missing_docs)]

use crate::domain::agent_execution::tools::ToolContent;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

/// The most results one grant keeps waiting for their tool call's report. A
/// result is taken as soon as its call is reported, so only results the
/// harness never reports wait; past this, the oldest is dropped.
pub const MAX_FORWARDED_RESULTS: usize = 32;

/// Where Claude's harness puts its own id for the call in a forwarded
/// `tools/call`: the id its ACP frames give the same call (`toolCallId`).
const CALL_ID: &str = "claudecode/toolUseId";

/// The longest call id kept: the ACP binding's bound on a tool call's id.
const MAX_CALL_ID_BYTES: usize = 256;

/// One grant's forwarded results, by the harness's id for each call. Clones
/// share them.
#[derive(Clone, Default)]
pub struct ForwardedResults {
    results: Arc<Mutex<VecDeque<(String, ToolContent)>>>,
}
impl ForwardedResults {
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
    pub fn take(&self, tool_call: &str) -> Option<ToolContent> {
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
impl std::fmt::Debug for ForwardedResults {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Results are a server's data: counted, not printed.
        let kept = self.results.lock().map_or(0, |results| results.len());
        f.debug_struct("ForwardedResults")
            .field("kept", &kept)
            .finish()
    }
}

/// The harness's id for a forwarded `tools/call`, from its `params`: a
/// non-empty string of at most [`MAX_CALL_ID_BYTES`], or `None`.
pub(crate) fn call_id(params: Option<&Value>) -> Option<String> {
    params?
        .get("_meta")?
        .get(CALL_ID)?
        .as_str()
        .filter(|id| !id.is_empty() && id.len() <= MAX_CALL_ID_BYTES)
        .map(str::to_owned)
}
