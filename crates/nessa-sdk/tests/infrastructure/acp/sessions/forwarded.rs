//! The store of forwarded results, and attaching one to the call it answers:
//! rows S9, S10 and W1–W7 of the "Forwarded results" table in
//! `docs/design/mcp-connections.md`, each test named after its row.
use super::*;
use crate::domain::agent_execution::tools::{McpTool, ToolCallId};

/// A call's update with `status` and `content`, naming an MCP tool when `mcp`.
fn call_update(
    status: Option<ToolStatus>,
    content: Option<Vec<ToolContent>>,
    mcp: bool,
) -> ToolCallUpdate {
    let update = ToolCallUpdate::new(id("toolu_1"), None, None, status, None, content);
    if mcp {
        update.with_mcp_tool(McpTool::new("mcptest", "report_rows").unwrap())
    } else {
        update
    }
}

/// Forwarded results holding one for `toolu_1`.
fn forwarded_rows() -> ForwardedResults {
    let forwarded = ForwardedResults::new();
    forwarded.record(id("toolu_1"), rows());
    forwarded
}

fn id(id: &str) -> ToolCallId {
    ToolCallId::new(id).unwrap()
}

fn rows() -> ToolContent {
    ToolContent::structured(r#"{"rows":[1,2]}"#).unwrap()
}

fn completed(content: Vec<ToolContent>) -> ToolCallUpdate {
    call_update(Some(ToolStatus::Completed), Some(content), true)
}

#[test]
fn w1_a_completed_update_with_content_gets_its_forwarded_result_after_its_content() {
    let forwarded = forwarded_rows();
    let said = ToolContent::text(r#"{"rows":[1,2]}"#);
    let update = attach_forwarded(completed(vec![said.clone()]), Some(&forwarded));
    assert_eq!(update.content(), &Some(vec![said, rows()]));
}

#[test]
fn w2_an_update_without_content_or_before_the_end_leaves_the_result_waiting() {
    let forwarded = forwarded_rows();
    // Claude's PostToolUse frame: completed-looking or not, it has no content.
    for status in [None, Some(ToolStatus::Completed)] {
        let update = attach_forwarded(call_update(status, None, true), Some(&forwarded));
        assert_eq!(update.content(), &None);
    }
    for status in [None, Some(ToolStatus::Pending), Some(ToolStatus::Running)] {
        let update = attach_forwarded(call_update(status, Some(vec![]), true), Some(&forwarded));
        assert_eq!(update.content(), &Some(vec![]));
    }
    // Still there for the completed update.
    let update = attach_forwarded(completed(vec![]), Some(&forwarded));
    assert_eq!(update.content(), &Some(vec![rows()]));
}

#[test]
fn w3_a_call_with_nothing_forwarded_keeps_its_text_alone() {
    let said = vec![ToolContent::text("Two rows")];
    let update = attach_forwarded(completed(said.clone()), Some(&ForwardedResults::new()));
    assert_eq!(update.content(), &Some(said));
}

#[test]
fn w4_a_second_completed_update_gets_nothing_more() {
    let forwarded = forwarded_rows();
    attach_forwarded(completed(vec![]), Some(&forwarded));
    let again = attach_forwarded(completed(vec![]), Some(&forwarded));
    assert_eq!(again.content(), &Some(vec![]));
}

#[test]
fn w5_a_call_naming_no_mcp_tool_takes_nothing() {
    let forwarded = forwarded_rows();
    let update = attach_forwarded(
        call_update(Some(ToolStatus::Completed), Some(vec![]), false),
        Some(&forwarded),
    );
    assert_eq!(update.content(), &Some(vec![]));
    assert_eq!(forwarded.take(&id("toolu_1")), Some(rows()));
}

#[test]
fn w6_an_open_without_forwarded_results_leaves_the_update_as_it_was() {
    let update = attach_forwarded(completed(vec![]), None);
    assert_eq!(update.content(), &Some(vec![]));
}

#[test]
fn w7_a_failed_update_takes_nothing() {
    // The harness reports an `isError` result `failed`: its text is told,
    // and its structured content is not attached.
    let forwarded = forwarded_rows();
    let refused = vec![ToolContent::text("This tool always fails, on purpose.")];
    let update = attach_forwarded(
        call_update(Some(ToolStatus::Failed), Some(refused.clone()), true),
        Some(&forwarded),
    );
    assert_eq!(update.content(), &Some(refused));
    assert_eq!(forwarded.take(&id("toolu_1")), Some(rows()));
}

fn structured(json: &str) -> ToolContent {
    ToolContent::structured(json).unwrap()
}

#[test]
fn s9_past_the_bound_the_oldest_result_is_dropped() {
    let forwarded = ForwardedResults::new();
    for call in 0..=MAX_FORWARDED_RESULTS {
        forwarded.record(id(&format!("toolu_{call}")), structured(&call.to_string()));
    }
    assert_eq!(forwarded.len(), MAX_FORWARDED_RESULTS);
    assert_eq!(forwarded.take(&id("toolu_0")), None);
    assert_eq!(forwarded.take(&id("toolu_1")), Some(structured("1")));
    let last = format!("toolu_{MAX_FORWARDED_RESULTS}");
    assert_eq!(
        forwarded.take(&id(&last)),
        Some(structured(&MAX_FORWARDED_RESULTS.to_string()))
    );
}

#[test]
fn s10_an_id_kept_again_holds_the_later_result_once() {
    let forwarded = ForwardedResults::new();
    forwarded.record(id("toolu_1"), structured("1"));
    forwarded.record(id("toolu_1"), structured("2"));
    assert_eq!(forwarded.len(), 1);
    assert_eq!(forwarded.take(&id("toolu_1")), Some(structured("2")));
    assert_eq!(forwarded.take(&id("toolu_1")), None);
}
