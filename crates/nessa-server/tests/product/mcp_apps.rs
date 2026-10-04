//! What an MCP App's wire commands take from the caller, and what a server's
//! error becomes on the wire (#348).
use super::{app, remote_message, McpAppReference};
use nessa_protocol::protocol::MAX_PAYLOAD_BYTES;

fn reference(execution: &str, tool: &str, instance: &str) -> McpAppReference {
    McpAppReference {
        execution_id: execution.into(),
        tool_id: tool.into(),
        instance_id: instance.into(),
    }
}

const MOUNT: &str = "6f1d6c0e-8f8c-4a52-9b8e-1f6c3d2a4b5c";

#[test]
fn an_app_is_named_by_bounded_identities_and_the_hosts_lowercase_uuid() {
    let named = app(reference("e", "t", MOUNT)).unwrap();
    assert_eq!(named.instance_id, MOUNT);
    let long = "x".repeat(257);
    for refused in [
        reference("", "t", MOUNT),
        reference("e", "", MOUNT),
        reference(&long, "t", MOUNT),
        reference("e", &long, MOUNT),
        // Only the schema's form: lowercase, hyphenated, 36 characters.
        reference("e", "t", &MOUNT.to_uppercase()),
        reference("e", "t", &MOUNT.replace('-', "")),
        reference("e", "t", &format!("{{{MOUNT}}}")),
        reference("e", "t", "not-a-mount"),
        reference("e", "t", ""),
    ] {
        assert!(app(refused).is_err());
    }
    // Exactly at the identity bound is taken.
    assert!(app(reference(&"x".repeat(256), &"y".repeat(256), MOUNT)).is_ok());
}

#[test]
fn a_servers_message_is_bounded_in_characters_with_control_characters_as_spaces() {
    assert_eq!(remote_message("bad\nparams\t!"), "bad params !");
    let long = "é".repeat(600);
    let bounded = remote_message(&long);
    assert_eq!(bounded.chars().count(), 512);
    assert!(bounded.chars().all(|character| character == 'é'));
    assert_eq!(remote_message(&"a".repeat(512)), "a".repeat(512));
}

#[test]
fn the_schema_states_the_bounds_these_commands_keep() {
    use super::{MAX_IDENTITY_BYTES, MAX_REMOTE_CODE, MAX_REMOTE_MESSAGE_CHARS};
    use nessa_sdk::domain::agent_execution::tools::MAX_MCP_NAME_BYTES;
    use nessa_sdk::domain::mcp_apps::MAX_UI_URI_BYTES;
    let schema: serde_json::Value = serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    )))
    .unwrap();
    let defs = &schema["$defs"];
    let bytes = |value: &serde_json::Value| value["x-utf8MaxBytes"].as_u64().unwrap() as usize;
    let call = &defs["McpCallToolParams"]["properties"];
    assert_eq!(bytes(&call["server"]), MAX_MCP_NAME_BYTES);
    assert_eq!(bytes(&call["tool"]), MAX_MCP_NAME_BYTES);
    let app = &defs["McpAppReference"]["properties"];
    assert_eq!(bytes(&app["executionId"]), MAX_IDENTITY_BYTES);
    assert_eq!(bytes(&app["toolId"]), MAX_IDENTITY_BYTES);
    assert_eq!(
        bytes(&defs["McpReadResourceParams"]["properties"]["uri"]),
        MAX_UI_URI_BYTES
    );
    let remote = &defs["McpRemoteErrorDetails"]["properties"];
    assert_eq!(
        remote["message"]["maxLength"].as_u64(),
        Some(MAX_REMOTE_MESSAGE_CHARS as u64)
    );
    assert_eq!(remote["code"]["maximum"].as_i64(), Some(MAX_REMOTE_CODE));
    assert_eq!(remote["code"]["minimum"].as_i64(), Some(-MAX_REMOTE_CODE));
}

#[test]
fn a_reordering_character_is_shown_as_a_space() {
    assert_eq!(
        remote_message("a\u{202E}b\u{2066}c\u{2028}d\u{200F}e"),
        "a b c d e"
    );
}

#[test]
fn the_largest_resource_the_gateway_reads_is_answered_within_one_message() {
    // Its URI, CSP and domain at the bound the conversation service holds
    // them to, every other field at its largest: one socket message.
    use crate::conversation::application::MAX_RESOURCE_META_BYTES;
    use nessa_protocol::product::generated::{McpReadResourceResult, McpUiCsp, McpUiPermissions};
    let uri = format!("ui://{}", "u".repeat(2000));
    let domain = "d".repeat(512);
    let fixed = serde_json::to_vec(&serde_json::json!({
        "uri": uri,
        "connect": Vec::<String>::new(),
        "resource": Vec::<String>::new(),
        "frame": Vec::<String>::new(),
        "baseUri": Vec::<String>::new(),
        "domain": domain,
    }))
    .unwrap()
    .len();
    // The rest of the bound, across the four lists at their most sources —
    // what the schema allows — each source needing escaping.
    let per_source = (MAX_RESOURCE_META_BYTES - fixed) / (4 * 64);
    let source = "\"".repeat((per_source - 3) / 2);
    let list = vec![source; 64];
    let result = McpReadResourceResult {
        uri,
        mime_type: crate::mcp_servers::entrypoint::http::CONTENT_TYPE.into(),
        size: 4 * 1024 * 1024,
        sha256: "f".repeat(64),
        ticket: "t".repeat(43),
        expires_in_ms: 60_000,
        csp: McpUiCsp {
            connect_domains: list.clone(),
            resource_domains: list.clone(),
            frame_domains: list.clone(),
            base_uri_domains: list,
        },
        permissions: McpUiPermissions {
            camera: true,
            microphone: true,
            geolocation: true,
            clipboard_write: true,
        },
        domain: Some(domain),
        prefers_border: Some(true),
    };
    // The longest request id, every character escaped.
    let request_id = "\u{1}".repeat(256);
    let message = super::super::socket::success(&request_id, &result);
    let text = message.to_wire_text().unwrap();
    assert!(text.len() <= MAX_PAYLOAD_BYTES as usize, "{}", text.len());
}
