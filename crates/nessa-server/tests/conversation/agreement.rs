//! Numbers this context enforces and the published protocol schema states
//! again. Each is valid on its own; these check that they describe the same
//! gateway, so changing one without the other fails here.
use crate::conversation::{
    application::{MAX_LISTED_CONVERSATIONS, MAX_STRUCTURED_CONTENT_BYTES},
    domain::{ConversationPreview, ConversationTitle, LATEST_TIME_MS},
};
use nessa_sdk::domain::agent_execution::tools::MAX_MCP_NAME_BYTES;
use nessa_sdk::domain::mcp_apps::MAX_UI_URI_BYTES;
use serde_json::Value;

fn schema() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    )))
    .expect("the product schema is valid JSON")
}

#[test]
fn the_list_schema_states_the_bounds_the_summary_rules_keep() {
    let schema = schema();
    let summary = &schema["$defs"]["ConversationSummary"]["properties"];
    assert_eq!(
        summary["preview"]["x-utf8MaxBytes"].as_u64(),
        Some(ConversationPreview::MAX_BYTES as u64)
    );
    // A title is bounded in characters; at four bytes each it still fits.
    let title_bytes = summary["title"]["x-utf8MaxBytes"].as_u64().unwrap();
    assert!(ConversationTitle::MAX_CHARS as u64 * 4 <= title_bytes);
    // Every time a row carries is one the domain can hold, and no later.
    for field in ["createdAtMs", "updatedAtMs"] {
        assert_eq!(
            summary[field]["maximum"].as_u64(),
            Some(LATEST_TIME_MS),
            "{field}"
        );
    }
    assert_eq!(
        schema["$defs"]["ConversationListResult"]["properties"]["conversations"]["maxItems"]
            .as_u64(),
        Some(MAX_LISTED_CONVERSATIONS as u64)
    );
}

#[test]
fn a_read_states_the_same_title_bound_as_the_list() {
    let schema = schema();
    let view = &schema["$defs"]["ConversationView"];
    let summary = &schema["$defs"]["ConversationSummary"]["properties"];
    // One title, shown by both: the same bound, and required on a read so a
    // conversation with no title yet says `null` rather than leaving it out.
    assert_eq!(
        view["properties"]["title"]["x-utf8MaxBytes"],
        summary["title"]["x-utf8MaxBytes"]
    );
    let title_bytes = view["properties"]["title"]["x-utf8MaxBytes"]
        .as_u64()
        .unwrap();
    assert!(ConversationTitle::MAX_CHARS as u64 * 4 <= title_bytes);
    assert!(view["required"]
        .as_array()
        .unwrap()
        .contains(&Value::from("title")));
}

#[test]
fn a_tool_states_the_mcp_name_and_structured_result_bounds_the_view_keeps() {
    let schema = schema();
    let tool = &schema["$defs"]["ConversationTool"]["properties"];
    assert_eq!(
        tool["structuredContent"]["x-utf8MaxBytes"].as_u64(),
        Some(MAX_STRUCTURED_CONTENT_BYTES as u64)
    );
    // The names are the SDK domain's, which bounds them; the schema says the same.
    let mcp = &schema["$defs"]["ConversationMcpTool"]["properties"];
    for field in ["server", "tool"] {
        assert_eq!(
            mcp[field]["x-utf8MaxBytes"].as_u64(),
            Some(MAX_MCP_NAME_BYTES as u64),
            "{field}"
        );
    }
    // The UI resource's URI is the SDK domain's too, bounded the same.
    assert_eq!(
        mcp["resourceUri"]["x-utf8MaxBytes"].as_u64(),
        Some(MAX_UI_URI_BYTES as u64)
    );
}

#[test]
fn an_app_calls_schema_states_the_bounds_the_gateway_keeps() {
    use crate::conversation::application::{McpAppCode, RESOURCE_TICKET_LIFETIME_MS};
    use crate::mcp_servers::domain::{MAX_APP_ARGUMENTS_BYTES, MAX_APP_RESULT_BYTES};
    use nessa_sdk::domain::mcp_apps::MAX_UI_HTML_BYTES;
    let schema = schema();
    let defs = &schema["$defs"];
    let arguments = &defs["McpCallToolParams"]["properties"]["argumentsJson"];
    assert_eq!(
        arguments["x-utf8MaxBytes"].as_u64(),
        Some(MAX_APP_ARGUMENTS_BYTES as u64)
    );
    // An app's arguments are shown whole in its review.
    assert_eq!(
        arguments["x-utf8MaxBytes"],
        defs["ConversationPermission"]["properties"]["argumentsJson"]["x-utf8MaxBytes"]
    );
    assert_eq!(
        defs["McpCallToolResult"]["properties"]["resultJson"]["x-utf8MaxBytes"].as_u64(),
        Some(MAX_APP_RESULT_BYTES as u64)
    );
    let read = &defs["McpReadResourceResult"]["properties"];
    assert_eq!(
        read["size"]["maximum"].as_u64(),
        Some(MAX_UI_HTML_BYTES as u64)
    );
    assert_eq!(
        read["expiresInMs"]["const"].as_u64(),
        Some(RESOURCE_TICKET_LIFETIME_MS)
    );
    // Every code audit names a step with is one the protocol carries.
    let codes = defs["ConversationErrorCode"]["enum"].as_array().unwrap();
    for code in McpAppCode::ALL {
        assert!(
            codes.iter().any(|known| known == code.as_str()),
            "{}",
            code.as_str()
        );
    }
}
