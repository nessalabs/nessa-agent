//! `nessa call` and `nessa mcp` send one catalog method through one gateway port.
use crate::cli::application::{
    invoke, tools, BrowserToken, CliError, Gateway, GatewayCallError, GatewayIdentity, TokenRequest,
};
use crate::cli::infrastructure::serve_mcp;
use serde_json::{json, Value};
use std::io::Cursor;

struct Recording {
    identity: GatewayIdentity,
    calls: Vec<(String, Value)>,
    result: Result<Value, GatewayCallError>,
}

impl Gateway for Recording {
    fn identity(&self) -> &GatewayIdentity {
        &self.identity
    }
    fn health(&mut self) -> Result<(), CliError> {
        Ok(())
    }
    fn issue(&mut self, _request: TokenRequest) -> Result<BrowserToken, CliError> {
        Err(CliError::Protocol)
    }
    fn call(&mut self, method: &str, params: Value) -> Result<Value, GatewayCallError> {
        self.calls.push((method.to_owned(), params));
        self.result.clone()
    }
}

fn recording(result: Result<Value, GatewayCallError>) -> Recording {
    Recording {
        identity: GatewayIdentity {
            gateway_id: "gateway".into(),
            organization_id: "org".into(),
            principal_id: "caller".into(),
            expires_at: None,
        },
        calls: Vec::new(),
        result,
    }
}

#[test]
fn a_cli_call_and_an_mcp_call_send_the_same_method() {
    let health = json!({"ok": true, "uptimeMs": 42});
    let mut cli = recording(Ok(health.clone()));
    assert_eq!(
        invoke(&mut cli, "server.health", json!({})).unwrap(),
        health
    );

    let text = health.to_string();
    let mut mcp = recording(Ok(health));
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"server.health","arguments":{}}}"#,
        "\n",
    );
    let mut output = Vec::new();
    serve_mcp(&mut mcp, Cursor::new(input), &mut output).unwrap();
    assert_eq!(cli.calls, mcp.calls);
    assert_eq!(cli.calls, vec![("server.health".into(), json!({}))]);
    let reply = String::from_utf8(output).unwrap();
    let call = reply
        .lines()
        .nth(1)
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .expect("tools/call reply");
    assert_eq!(call["result"]["isError"], false);
    assert_eq!(call["result"]["content"][0]["text"], text);
}

#[test]
fn a_gateway_refusal_keeps_its_code_and_schema_failures_are_not_sent() {
    let mut refused = recording(Err(GatewayCallError::Refused {
        code: "forbidden".into(),
    }));
    let error = invoke(&mut refused, "server.health", json!({})).unwrap_err();
    assert_eq!(error.code, "forbidden");
    assert_eq!(refused.calls.len(), 1);

    let mut gateway = recording(Ok(json!({})));
    let error = invoke(&mut gateway, "server.health", json!({"extra": true})).unwrap_err();
    assert_eq!(error.code, "invalid_request");
    assert!(gateway.calls.is_empty());

    let error = invoke(&mut gateway, "session.authenticate", json!({})).unwrap_err();
    assert_eq!(error.code, "invalid_request");
    assert!(error.detail.contains("already_authenticated"));
    assert!(gateway.calls.is_empty());
}

#[test]
fn the_agent_name_bound_is_utf8_bytes() {
    let base = json!({
        "conversationId": "00000000-0000-4000-8000-000000000001",
        "requestId": "create",
    });
    let mut accepted = base.clone();
    accepted["agent"] = json!("😀".repeat(8));
    let mut gateway = recording(Ok(json!({"conversationId": accepted["conversationId"]})));
    invoke(&mut gateway, "conversation.create", accepted).unwrap();
    assert_eq!(gateway.calls.len(), 1);

    let mut rejected = base;
    rejected["agent"] = json!("😀".repeat(9));
    let error = invoke(&mut gateway, "conversation.create", rejected).unwrap_err();
    assert_eq!(error.code, "invalid_request");
    assert_eq!(gateway.calls.len(), 1);
}

#[test]
fn mcp_lists_each_exposed_method_and_not_an_excluded_one() {
    let listed = tools();
    let names: Vec<_> = listed
        .iter()
        .map(|tool| tool["name"].as_str().unwrap())
        .collect();
    assert!(names.contains(&"server.health"));
    assert!(names.contains(&"conversation.list"));
    assert!(!names.contains(&"session.authenticate"));
    assert!(!names.contains(&"attachment.write"));
    assert!(!names.contains(&"conversation.subscribe"));
    let health = listed
        .iter()
        .find(|tool| tool["name"] == "server.health")
        .unwrap();
    assert_eq!(health["annotations"]["readOnlyHint"], true);
    assert_eq!(health["annotations"]["destructiveHint"], false);
    assert_eq!(health["inputSchema"]["additionalProperties"], false);
    let delete = listed
        .iter()
        .find(|tool| tool["name"] == "conversation.delete")
        .unwrap();
    assert_eq!(delete["annotations"]["readOnlyHint"], false);
    assert_eq!(delete["annotations"]["destructiveHint"], true);
    let create = listed
        .iter()
        .find(|tool| tool["name"] == "conversation.create")
        .unwrap();
    assert_eq!(create["inputSchema"]["type"], "object");
    assert!(create["inputSchema"].get("$ref").is_none());
    assert!(create["inputSchema"]["properties"]
        .get("conversationId")
        .is_some());
}

/// The MCP tool definition, as the 2025-06-18 contract requires `inputSchema.type`
/// to be the string `object` on the schema itself.
fn mcp_tools_contract() -> Value {
    json!({
        "$schema": "https://json-schema.org/draft/2020-12/schema",
        "type": "object",
        "required": ["tools"],
        "additionalProperties": false,
        "properties": {
            "tools": {
                "type": "array",
                "items": {
                    "type": "object",
                    "required": ["name", "inputSchema"],
                    "additionalProperties": false,
                    "properties": {
                        "name": {"type": "string", "minLength": 1},
                        "description": {"type": "string"},
                        "inputSchema": {
                            "type": "object",
                            "required": ["type"],
                            "properties": {
                                "type": {"const": "object"},
                                "properties": {"type": "object"},
                                "required": {"type": "array", "items": {"type": "string"}}
                            }
                        },
                        "annotations": {
                            "type": "object",
                            "properties": {
                                "readOnlyHint": {"type": "boolean"},
                                "destructiveHint": {"type": "boolean"},
                                "idempotentHint": {"type": "boolean"},
                                "openWorldHint": {"type": "boolean"}
                            }
                        }
                    }
                }
            }
        }
    })
}

#[test]
fn tools_list_matches_the_mcp_tool_contract() {
    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"tools/list"}"#,
        "\n",
    );
    let mut gateway = recording(Ok(json!({})));
    let mut output = Vec::new();
    serve_mcp(&mut gateway, Cursor::new(input), &mut output).unwrap();
    let reply = String::from_utf8(output).unwrap();
    let listed = reply
        .lines()
        .nth(1)
        .and_then(|line| serde_json::from_str::<Value>(line).ok())
        .expect("tools/list reply");
    let document = json!({"tools": listed["result"]["tools"].clone()});
    let validator = jsonschema::options()
        .offline()
        .build(&mcp_tools_contract())
        .expect("MCP tool contract");
    assert!(
        validator.is_valid(&document),
        "{}",
        validator.validate(&document).expect_err("invalid catalog")
    );
    assert!(gateway.calls.is_empty());
}

#[test]
fn the_surface_wait_outlasts_an_archive_install() {
    use crate::cli::infrastructure::surface_call_budget;
    use nessa_agent_install::infrastructure::https_archives::ARCHIVE_TRANSFER_TIMEOUT;
    use std::time::Duration;
    let budget = surface_call_budget();
    assert!(budget >= ARCHIVE_TRANSFER_TIMEOUT);
    assert!(budget >= Duration::from_millis(233_000));
    assert!(budget >= Duration::from_millis(300_000 + 60_000));
}

#[test]
fn nessa_mcp_serves_drive_nessa_skill_with_verified_manifest() {
    use nessa_sdk::domain::mcp_skills::{SkillFileUri, EXTENSION};

    let input = concat!(
        r#"{"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2026-07-28"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":2,"method":"skills/list","params":{}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":3,"method":"skills/get","params":{"uri":"skill://drive-nessa/SKILL.md"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":4,"method":"resources/read","params":{"uri":"skill://drive-nessa/SKILL.md"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":5,"method":"resources/directory/read","params":{"uri":"skill://drive-nessa/references"}}"#,
        "\n",
        r#"{"jsonrpc":"2.0","id":6,"method":"skills/get","params":{"uri":"skill://missing/SKILL.md"}}"#,
        "\n",
    );
    let mut gateway = recording(Ok(json!({})));
    let mut output = Vec::new();
    serve_mcp(&mut gateway, Cursor::new(input), &mut output).unwrap();
    let replies: Vec<Value> = String::from_utf8(output)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    assert_eq!(replies.len(), 6);

    // 1. initialize advertises 2026-07-28, resources, and io.modelcontextprotocol/skills.
    assert_eq!(replies[0]["result"]["protocolVersion"], "2026-07-28");
    assert_eq!(
        replies[0]["result"]["capabilities"]["extensions"][EXTENSION]["directoryRead"],
        json!(true)
    );
    assert!(replies[0]["result"]["instructions"]
        .as_str()
        .unwrap()
        .contains("skill://drive-nessa/SKILL.md"));

    // 2. skills/list returns drive-nessa with complete manifest.
    assert_eq!(
        replies[1]["result"]["skills"][0]["uri"],
        "skill://drive-nessa/SKILL.md"
    );
    assert_eq!(replies[1]["result"]["resultType"], "complete");

    // 3. skills/get matches skills/list.
    assert_eq!(
        replies[2]["result"]["skill"],
        replies[1]["result"]["skills"][0]
    );

    // 4. resources/read returns SKILL.md bytes that verify against the catalog entry.
    let skill_text = replies[3]["result"]["contents"][0]["text"]
        .as_str()
        .unwrap();
    let entry = crate::cli::application::skills()
        .get("skill://drive-nessa/SKILL.md")
        .unwrap();
    entry
        .verify_file(&SkillFileUri::from(entry.uri()), skill_text.as_bytes())
        .expect("served SKILL.md matches its manifest digest and frontmatter");

    // 5. resources/directory/read lists the supporting references.
    let refs = replies[4]["result"]["resources"].as_array().unwrap();
    assert_eq!(refs.len(), 2);

    // 6. Unknown skill URI returns JSON-RPC -32602.
    assert_eq!(replies[5]["error"]["code"], -32602);
    assert!(gateway.calls.is_empty());
}
