//! The results a grant's stand-ins forwarded to their harness, kept for the
//! tool calls the harness reports them under.
//!
//! ```text
//! stand_in ──record(call id, result)──▶ ForwardedResults ◀──take(tool call id)── ACP worker
//!                (before the harness is answered)            (the call's completed update)
//! ```
//!
//! Arrows are calls. Claude's harness gives its model, and its ACP client,
//! `structuredContent` only as JSON text, in place of the result's own text,
//! so no ACP frame says which text was structured. The stand-in saw the
//! object, and the harness names the call it forwarded
//! (`_meta["claudecode/toolUseId"]`) by the id it reports that call under over
//! ACP. The key is read here, where the forwarded bytes pass; the take is the
//! shared ACP worker's, since a profile cannot see the open's grant. A harness
//! that names no call id (Codex, OpenCode) has nothing kept, and its takes
//! find nothing. The states and orderings are tabled in
//! `docs/design/mcp-connections.md` ("Forwarded results"): S1–S11 are tested
//! in `tests/infrastructure/mcp/forwarded.rs`, W1–W7 in
//! `tests/infrastructure/acp/tools/wire.rs`.
#![deny(missing_docs)]

use crate::domain::agent_execution::tools::ToolContent;
use crate::infrastructure::acp::fields::identifier;
use serde_json::Value;
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

/// The most results one grant keeps waiting for their tool call's report. A
/// result is taken as soon as its call is reported completed, so only results
/// the harness never reports so wait; past this, the oldest is dropped.
pub const MAX_FORWARDED_RESULTS: usize = 32;

/// Where Claude's harness puts its own id for the call in a forwarded
/// `tools/call`: the id its ACP frames give the same call (`toolCallId`).
const CALL_ID: &str = "claudecode/toolUseId";

/// One grant's forwarded results, by the harness's id for each call. Clones
/// share them, and two values are equal when they are the same store. Only
/// the SDK writes and takes them: a host can hand them on
/// ([`StandInGrant::with_forwarded`](crate::infrastructure::acp::sessions::StandInGrant::with_forwarded))
/// and compare them, nothing else.
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
    /// `tool_call`, taken: a second take of the same id finds nothing. The ACP
    /// worker is its one caller.
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

/// The harness's id for a forwarded `tools/call`, from its `params`: one the
/// ACP binding would accept as a tool call's id
/// ([`identifier`](crate::infrastructure::acp::fields::identifier)), or `None`.
pub(crate) fn call_id(params: Option<&Value>) -> Option<String> {
    identifier(params?.get("_meta")?, CALL_ID)
        .ok()
        .map(str::to_owned)
}
