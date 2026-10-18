//! A stdio ACP agent with no model and almost no footprint, for the gateway
//! scale check (`scripts/scale/gateway-scale.mjs`):
//!
//!   scale_agent <harness-name> <harness-version> [reply-delay-ms] [chunks]
//!
//! It answers the handshake as Claude's pinned harness would (the same frames
//! `scripts/mcp-test-server/scripted-agent.mjs` sends for `claude`), and every
//! prompt with `chunks` text chunks (1 by default; the first is `Noted.`, the
//! rest about 120 bytes each, as a real reply streams), then `end_turn`,
//! after `reply-delay-ms` (0 by default). More chunks make each turn's
//! history larger, so a long conversation grows as a real one does. The scripted agent is a Node process of
//! tens of megabytes; ten thousand of them do not fit on one machine, which
//! would make the check measure Node instead of the gateway. This one is a
//! few hundred kilobytes, so what grows with the conversation count is the
//! gateway's own share.
//!
//! It reads no credential and starts no MCP server: the check configures none.
use serde_json::{json, Value};
use std::collections::HashMap;
use std::io::{BufRead, Write};
use std::sync::{Arc, Mutex};
use std::time::Duration;

fn main() {
    let args: Vec<String> = std::env::args().collect();
    let [_, name, version, rest @ ..] = args.as_slice() else {
        eprintln!("usage: scale_agent <harness-name> <harness-version> [reply-delay-ms] [chunks]");
        std::process::exit(2);
    };
    let delay = Duration::from_millis(rest.first().and_then(|v| v.parse().ok()).unwrap_or(0));
    let chunks: usize = rest.get(1).and_then(|v| v.parse().ok()).unwrap_or(1).max(1);
    let out = Arc::new(Mutex::new(std::io::stdout()));
    // Option values by session id, as the claude harness reports them.
    let mut sessions: HashMap<String, Vec<(String, String)>> = HashMap::new();
    let mut next = 0u64;
    for line in std::io::stdin().lock().lines() {
        let Ok(line) = line else { break };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            continue;
        };
        let (Some(id), Some(method)) = (message.get("id"), message["method"].as_str()) else {
            continue;
        };
        let params = &message["params"];
        let result = match method {
            "initialize" => Ok(json!({
                "protocolVersion": 1,
                "agentInfo": { "name": name, "version": version },
                // Resumable, so a parked conversation's agent can be started
                // again on its session (docs/design/agent-parking.md).
                "agentCapabilities": {
                    "loadSession": false,
                    "promptCapabilities": {},
                    "sessionCapabilities": { "resume": {} },
                },
                "authMethods": [],
            })),
            "session/new" => {
                next += 1;
                let session = format!("scale-{}-{next}", std::process::id());
                open(&mut sessions, params, session)
            }
            // The session it names, opened again as if new: this agent keeps
            // no history, so there is nothing else to restore.
            "session/resume" => match params["sessionId"].as_str() {
                None | Some("") => Err("session/resume names no session".to_owned()),
                Some(session) if sessions.contains_key(session) => {
                    Err(format!("session {session} is already open"))
                }
                Some(session) => open(&mut sessions, params, session.to_owned()),
            },
            "session/set_config_option" => {
                let session = params["sessionId"].as_str().unwrap_or_default();
                let config = params["configId"].as_str().unwrap_or_default();
                let value = params["value"].as_str().unwrap_or_default();
                match sessions.get_mut(session) {
                    None => Err(format!("no session {session}")),
                    Some(_) if !["model", "mode", "effort"].contains(&config) => {
                        Err(format!("no config option {config}"))
                    }
                    Some(values) => {
                        match values.iter_mut().find(|(id, _)| id == config) {
                            Some(entry) => entry.1 = value.to_owned(),
                            None => values.push((config.to_owned(), value.to_owned())),
                        }
                        Ok(json!({ "configOptions": config_options(values) }))
                    }
                }
            }
            "session/prompt" => {
                let session = params["sessionId"].clone();
                let id = id.clone();
                let out = Arc::clone(&out);
                // Off this thread, so a cancel or a second session is still read.
                std::thread::spawn(move || {
                    std::thread::sleep(delay);
                    for index in 0..chunks {
                        let text = if index == 0 {
                            "Noted.".to_owned()
                        } else {
                            format!(
                                " {index:>4} {}",
                                "a reply streams in pieces like this one. ".repeat(3)
                            )
                        };
                        let chunk = json!({ "jsonrpc": "2.0", "method": "session/update", "params": {
                            "sessionId": session,
                            "update": {
                                "sessionUpdate": "agent_message_chunk",
                                "content": { "type": "text", "text": text },
                            },
                        }});
                        write(&out, &chunk);
                    }
                    let end = json!({ "jsonrpc": "2.0", "id": id, "result": { "stopReason": "end_turn" } });
                    write(&out, &end);
                });
                continue;
            }
            other => Err(format!("no method {other}")),
        };
        let reply = match result {
            Ok(result) => json!({ "jsonrpc": "2.0", "id": id, "result": result }),
            Err(message) => {
                json!({ "jsonrpc": "2.0", "id": id, "error": { "code": -32603, "message": message } })
            }
        };
        write(&out, &reply);
    }
}

/// Open `session` with the model and mode `params` name, as the claude
/// harness does for both `session/new` and `session/resume`.
fn open(
    sessions: &mut HashMap<String, Vec<(String, String)>>,
    params: &Value,
    session: String,
) -> Result<Value, String> {
    let options = &params["_meta"]["claudeCode"]["options"];
    let Some(model) = options["model"].as_str() else {
        return Err("the session names no model".to_owned());
    };
    let mode = options["permissionMode"].as_str().unwrap_or("default");
    let values = vec![
        ("model".to_owned(), model.to_owned()),
        ("mode".to_owned(), mode.to_owned()),
    ];
    let listed = config_options(&values);
    sessions.insert(session.clone(), values);
    Ok(json!({ "sessionId": session, "configOptions": listed }))
}

/// Each option offers only its current value, as the scripted agent's do.
fn config_options(values: &[(String, String)]) -> Value {
    Value::Array(
        values
            .iter()
            .map(|(id, value)| {
                let mut option = json!({
                    "id": id, "name": id, "type": "select", "currentValue": value,
                    "options": [{ "value": value, "name": value }],
                });
                if id == "effort" {
                    option["category"] = json!("thought_level");
                }
                option
            })
            .collect(),
    )
}

fn write(out: &Mutex<std::io::Stdout>, message: &Value) {
    let mut out = out.lock().unwrap_or_else(|poisoned| poisoned.into_inner());
    // A gateway that stopped reading leaves this agent nothing to do.
    if writeln!(out, "{message}")
        .and_then(|()| out.flush())
        .is_err()
    {
        std::process::exit(0);
    }
}
