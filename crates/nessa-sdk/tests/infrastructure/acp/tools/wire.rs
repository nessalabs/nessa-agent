//! Tool wire parsing and atomic sparse-update validation at the adapter boundary.
use super::*;
use crate::domain::agent_execution::tools::{McpTool, ToolObservation};
use serde_json::json;
#[test]
fn standard_tool_updates_preserve_sparse_content_without_provider_metadata() {
    let full = tool_call(
        &json!({"toolCallId":"read", "title":"Inspect file", "kind":"read",
        "content":[{"type":"diff", "path":"/example", "oldText":null, "newText":"new"}]}),
    )
    .unwrap();
    assert_eq!(full.kind(), &Some(ToolKind::Read));
    assert_eq!(
        full.content(),
        &Some(vec![ToolContent::diff(
            FilePath::new("/example").unwrap(),
            None,
            "new"
        )])
    );
    let patch =
        tool_call(&json!({"toolCallId":"read", "content":[], "status":"completed"})).unwrap();
    assert_eq!(patch.content(), &Some(vec![]));
    assert_eq!(patch.title(), &None);
    assert_eq!(patch.locations(), &None);
    assert_eq!(patch.status(), &Some(ToolStatus::Completed));
    assert!(tool_call(&json!({"toolCallId":"read", "locations":[{"path":""}]})).is_err());
}
#[test]
fn tagged_unsupported_content_becomes_bounded_placeholders_beside_supported_content() {
    let parsed = tool_call(&json!({"toolCallId":"read","content":[
        {"type":"content","content":{"type":"text","text":"before"}},
        {"type":"content","content":{"type":"image","data":"opaque"}},
        {"type":"terminal","terminalId":"command"},
        {"type":"bash_code_execution_result","stdout":"opaque"},
        {"type":"content","content":{"type":"text","text":"after"}}
    ]}))
    .unwrap();
    assert_eq!(
        parsed.content(),
        &Some(vec![
            ToolContent::text("before"),
            ToolContent::text(UNSUPPORTED_TOOL_CONTENT),
            ToolContent::text(UNSUPPORTED_TOOL_CONTENT),
            ToolContent::text(UNSUPPORTED_TOOL_CONTENT),
            ToolContent::text("after"),
        ])
    );
}

#[test]
fn malformed_content_rejects_the_whole_patch_without_clearing_previous_observations() {
    let initial = tool_call(&json!({"toolCallId":"read","content":[{"type":"content","content":{"type":"text","text":"retained"}}]})).unwrap();
    let tool = ToolObservation::default().with_update(initial);
    let before = tool.clone();
    for malformed in [
        json!({"type":"content","content":{}}),
        json!({"type":"content"}),
        json!({"type":"diff","path":"file","newText":7}),
        json!({"type":7}),
        json!({}),
    ] {
        let update = json!({"toolCallId":"read","content":[{"type":"content","content":{"type":"text","text":"partial"}},malformed]});
        assert!(matches!(tool_call(&update), Err(AgentError::Protocol(_))));
        assert_eq!(&tool, &before);
    }
    assert_eq!(
        tool_call(&json!({"toolCallId":"read","content":[]}))
            .unwrap()
            .content(),
        &Some(vec![])
    );
    assert_eq!(
        tool_call(&json!({"toolCallId":"read"})).unwrap().content(),
        &None
    );
}

#[test]
fn tool_wire_preserves_empty_text_and_unknown_versus_empty_prior_diff() {
    for (old, expected) in [(json!(null), None), (json!(""), Some(String::new()))] {
        let parsed = tool_call(&json!({"toolCallId":"read", "content":[
            {"type":"content", "content":{"type":"text", "text":""}},
            {"type":"diff", "path":"file", "oldText":old, "newText":""}
        ]}))
        .unwrap();
        assert_eq!(
            parsed.content(),
            &Some(vec![
                ToolContent::text(""),
                ToolContent::diff(FilePath::new("file").unwrap(), expected, "")
            ])
        );
    }
}

/// A call's update with `status` and `content`, naming an MCP tool when `mcp`.
fn call_update(
    status: Option<ToolStatus>,
    content: Option<Vec<ToolContent>>,
    mcp: bool,
) -> ToolCallUpdate {
    let update = ToolCallUpdate::new(
        ToolCallId::new("toolu_1").unwrap(),
        None,
        None,
        status,
        None,
        content,
    );
    if mcp {
        update.with_mcp_tool(McpTool::new("mcptest", "report_rows").unwrap())
    } else {
        update
    }
}

/// Forwarded results holding one for `toolu_1`.
fn forwarded_rows() -> ForwardedResults {
    let forwarded = ForwardedResults::new();
    forwarded.record("toolu_1".into(), rows());
    forwarded
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
    assert_eq!(forwarded.take("toolu_1"), Some(rows()));
}

#[test]
fn w6_an_open_without_forwarded_results_leaves_the_update_as_it_was() {
    let update = attach_forwarded(completed(vec![]), None);
    assert_eq!(update.content(), &Some(vec![]));
}

#[test]
fn w7_a_failed_update_takes_nothing() {
    // Claude reports an interrupted call `failed`, without its answer; one
    // that arrived anyway was never the model's, and is not attached.
    let forwarded = forwarded_rows();
    let interrupted = vec![ToolContent::text("[Request interrupted by user]")];
    let update = attach_forwarded(
        call_update(Some(ToolStatus::Failed), Some(interrupted.clone()), true),
        Some(&forwarded),
    );
    assert_eq!(update.content(), &Some(interrupted));
    assert_eq!(forwarded.take("toolu_1"), Some(rows()));
}
