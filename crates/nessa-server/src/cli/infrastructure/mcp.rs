//! Stdio MCP server whose tools are the surface catalog.
//!
//! Each `tools/call` is [`crate::cli::application::invoke`] on the gateway this
//! process authenticated. One call at a time: the product socket is one
//! exchange, and a second line waits until it finishes.
use crate::cli::application::{invoke, refusal_body, tools, Gateway};
use serde::Deserialize;
use serde_json::{json, Value};
use std::io::{BufRead, Write};

const MAX_LINE: usize = 1_048_576;

#[derive(Debug)]
pub struct McpStop(String);

impl std::fmt::Display for McpStop {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.0)
    }
}

impl std::error::Error for McpStop {}

#[derive(Deserialize)]
struct Request {
    jsonrpc: String,
    id: Option<Value>,
    method: String,
    params: Option<Value>,
}

/// Read newline-delimited JSON-RPC from `input` and write replies to `output`.
/// A clean EOF is success. A frame past one mebibyte stops the server.
pub fn serve(
    gateway: &mut dyn Gateway,
    input: impl BufRead,
    mut output: impl Write,
) -> Result<(), McpStop> {
    let mut initialized = false;
    for line in input.lines() {
        let line = line.map_err(|error| McpStop(error.to_string()))?;
        if line.len() > MAX_LINE {
            return Err(McpStop("MCP frame exceeds 1 MiB".into()));
        }
        if line.is_empty() {
            continue;
        }
        let request: Request = match serde_json::from_str(&line) {
            Ok(request) => request,
            Err(_) => {
                write_message(
                    &mut output,
                    error(Value::Null, -32700, "invalid JSON-RPC request"),
                )?;
                continue;
            }
        };
        if request.jsonrpc != "2.0" || !valid_id(request.id.as_ref()) {
            write_message(
                &mut output,
                error(Value::Null, -32600, "invalid JSON-RPC envelope"),
            )?;
            continue;
        }
        let Some(id) = request.id else {
            continue;
        };
        let response = match request.method.as_str() {
            "initialize" if !initialized => {
                initialized = true;
                let version = request
                    .params
                    .as_ref()
                    .and_then(|params| params.get("protocolVersion"))
                    .and_then(Value::as_str)
                    .filter(|version| {
                        matches!(*version, "2024-11-05" | "2025-03-26" | "2025-06-18")
                    })
                    .unwrap_or("2025-06-18");
                success(
                    id,
                    json!({
                        "protocolVersion": version,
                        "capabilities": {"tools": {}},
                        "serverInfo": {"name": "nessa", "version": env!("CARGO_PKG_VERSION")},
                        "instructions": "Each tool is one gateway method. The call uses this process's nessa-mcp credential. Cedar on the gateway decides.",
                    }),
                )
            }
            "ping" => success(id, json!({})),
            _ if !initialized => error(id, -32002, "initialize first"),
            "tools/list" => success(id, json!({"tools": tools()})),
            "tools/call" => success(id, call_tool(gateway, request.params.as_ref())),
            _ => error(id, -32601, "method not found"),
        };
        write_message(&mut output, response)?;
    }
    Ok(())
}

fn call_tool(gateway: &mut dyn Gateway, params: Option<&Value>) -> Value {
    let name = params
        .and_then(|params| params.get("name"))
        .and_then(Value::as_str);
    let Some(name) = name else {
        return tool_error("invalid_request");
    };
    let arguments = params
        .and_then(|params| params.get("arguments"))
        .cloned()
        .unwrap_or_else(|| json!({}));
    match invoke(gateway, name, arguments) {
        Ok(payload) => json!({
            "content": [{"type": "text", "text": payload.to_string()}],
            "isError": false,
        }),
        Err(failure) => {
            tracing::error!(code = %failure.code, detail = %failure.detail, "MCP tool call refused");
            json!({
                "content": [{"type": "text", "text": refusal_body(&failure).to_string()}],
                "isError": true,
            })
        }
    }
}

fn tool_error(code: &str) -> Value {
    json!({
        "content": [{"type": "text", "text": json!({"ok": false, "code": code}).to_string()}],
        "isError": true,
    })
}

fn valid_id(id: Option<&Value>) -> bool {
    id.is_none_or(|id| {
        id.is_i64()
            || id.is_u64()
            || id
                .as_str()
                .is_some_and(|text| !text.is_empty() && text.len() <= 256)
    })
}

fn success(id: Value, result: Value) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "result": result})
}

fn error(id: Value, code: i64, message: &str) -> Value {
    json!({"jsonrpc": "2.0", "id": id, "error": {"code": code, "message": message}})
}

fn write_message(output: &mut impl Write, message: Value) -> Result<(), McpStop> {
    writeln!(output, "{message}").map_err(|error| McpStop(error.to_string()))?;
    output.flush().map_err(|error| McpStop(error.to_string()))
}
