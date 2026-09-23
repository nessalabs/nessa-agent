//! Tool wire parsing and atomic sparse-update validation at the adapter boundary.
use super::*;
use crate::domain::agent_execution::tools::ToolObservation;
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
