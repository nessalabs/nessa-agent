//! Codex's own tool frames reach the shared vocabulary without losing what they
//! carried, and a permission request is never reviewed without naming its tool.
use super::*;
use crate::domain::agent_execution::tools::{FilePath, ToolContent, ToolKind};
use serde_json::json;

fn text(value: &str) -> ToolContent {
    ToolContent::text(value.to_owned())
}

fn terminal_command(id: &str) -> Value {
    json!({
        "sessionUpdate": "tool_call",
        "toolCallId": id,
        "kind": "execute",
        "name": "exec_command",
        "title": "npm test",
        "status": "pending",
        "content": [{"type": "terminal", "terminalId": id}],
        "rawInput": {"command": "npm test", "cwd": "/workspace"},
        "_meta": {"terminal_info": {"cwd": "/workspace", "terminal_id": id}},
    })
}

fn permission(tool: Value, meta: Option<Value>) -> Value {
    let mut request = json!({"sessionId": "session-1", "toolCall": tool});
    if let Some(meta) = meta {
        request["_meta"] = json!({"codex": meta});
    }
    request
}

#[test]
fn a_terminal_pointer_is_replaced_by_the_output_it_pointed_at() {
    let mut tools = HashMap::new();
    // The command itself: a pointer to a terminal this binding does not
    // implement, and nothing else. Dropping the pointer must not drop the call.
    let started = tool_call(&terminal_command("command-1"), &mut tools).unwrap();
    assert_eq!(started.kind(), &Some(ToolKind::Execute));
    assert_eq!(started.content().as_deref(), None);

    // Output streams in frames carrying nothing but the identifier and the data.
    let streamed = tool_call(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-1",
                "_meta":{"terminal_output_delta":{"data":"ok\n","terminal_id":"command-1"}}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(streamed.content().as_deref(), Some(&vec![text("ok\n")][..]));

    // The completion repeats the whole output. It was already carried, so it is
    // carried once, not twice.
    let completed = tool_call(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-1","status":"completed",
                "rawOutput":{"formatted_output":"ok\n","exit_code":0},
                "_meta":{"terminal_exit":{"exit_code":0,"terminal_id":"command-1"}}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(completed.content().as_deref(), None);
}

#[test]
fn output_a_tool_call_never_streamed_is_carried_from_its_completion() {
    let mut tools = HashMap::new();
    tool_call(&terminal_command("command-2"), &mut tools).unwrap();
    let completed = tool_call(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-2","status":"completed",
                "rawOutput":{"formatted_output":"only once\n","exit_code":0}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(completed.content().as_deref(), Some(&vec![text("only once\n")][..]));
    // Having carried it, the same aggregate arriving again does not repeat it.
    let repeated = tool_call(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-2",
                "rawOutput":{"formatted_output":"only once\n","exit_code":0}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(repeated.content().as_deref(), None);
}

#[test]
fn diffs_and_links_survive_translation_and_unknown_shapes_do_not_pass_as_empty() {
    let mut tools = HashMap::new();
    let edit = tool_call(
        &json!({"toolCallId":"file-1","kind":"edit","status":"pending","content":[
            {"type":"diff","path":"/a","oldText":null,"newText":"new"},
            {"type":"content","content":{"type":"resource_link","name":"shot.png","uri":"/a/shot.png"}}]}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        edit.content().as_deref(),
        Some(
            &vec![
                ToolContent::diff(FilePath::new("/a").unwrap(), None, String::from("new")),
                text("/a/shot.png"),
            ][..]
        )
    );
    for frame in [
        json!({"toolCallId":"file-2","content":[{"type":"audio","data":"..."}]}),
        json!({"toolCallId":"file-2","content":[{"type":"content","content":{"type":"image","data":"..."}}]}),
        json!({"toolCallId":"file-2","content":[{"type":"terminal"}]}),
        json!({"toolCallId":"file-2","content":"not a list"}),
        json!({"toolCallId":"file-2","rawOutput":{"formatted_output":7}}),
        json!({"toolCallId":"file-2","_meta":{"terminal_output":{"data":[]}}}),
    ] {
        assert!(tool_call(&frame, &mut HashMap::new()).is_err(), "{frame}");
    }
}

#[test]
fn provider_identity_is_retained_and_bounded_and_never_silently_replaced() {
    let mut tools = HashMap::new();
    tool_call(&terminal_command("command-3"), &mut tools).unwrap();
    // A later frame restating only the kind is not a second identity.
    tool_call(
        &json!({"toolCallId":"command-3","kind":"execute","status":"completed"}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        permission_input(
            &permission(json!({"toolCallId":"command-3","kind":"execute","status":"pending",
                               "rawInput":{"command":"npm test","cwd":"/workspace"}}), None),
            &tools
        )
        .unwrap()
        .name,
        "exec_command"
    );
    // A different name for the same call is a provider contradicting itself.
    assert_eq!(
        tool_call(
            &json!({"toolCallId":"command-3","name":"write_stdin","status":"completed"}),
            &mut tools
        ),
        Err(AgentError::Protocol("tool identity changed".into()))
    );
    // Names are bounded before retention, and the rejected name is not copied
    // into the error a teardown path would retain.
    let long = "n".repeat(129);
    assert_eq!(
        tool_call(
            &json!({"toolCallId":"command-4","name":long,"kind":"execute"}),
            &mut HashMap::new()
        ),
        Err(AgentError::Unsupported(
            "tool name is not a reviewable identity".into()
        ))
    );
    for name in [json!(""), json!("has space"), json!(7)] {
        assert!(tool_call(
            &json!({"toolCallId":"command-5","name":name,"kind":"execute"}),
            &mut HashMap::new()
        )
        .is_err());
    }
}

#[test]
fn retained_identities_are_bounded_and_cleared_between_executions() {
    let mut tools = HashMap::new();
    for index in 0..4096 {
        tool_call(
            &json!({"toolCallId":format!("call-{index}"),"kind":"execute"}),
            &mut tools,
        )
        .unwrap();
    }
    assert_eq!(
        tool_call(&json!({"toolCallId":"one-too-many","kind":"execute"}), &mut tools),
        Err(AgentError::Protocol("tool count limit exceeded".into()))
    );
    // A call already known keeps being accepted at the limit.
    assert!(tool_call(&json!({"toolCallId":"call-0","status":"completed"}), &mut tools).is_ok());
}

#[test]
fn an_approval_asked_with_no_arguments_is_still_reviewed_with_what_codex_said() {
    let mut tools = HashMap::new();
    // Codex asks for a file change with an identifier, a kind, and a status. The
    // facts it is asking about are in the request's own metadata.
    let request = permission(
        json!({"toolCallId":"file-change-1","kind":"edit","status":"pending"}),
        Some(json!({"params":{"itemId":"file-change-1","reason":"Modifying config file"}})),
    );
    let review = permission_input(&request, &tools).unwrap();
    assert_eq!(review.name, "edit");
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        json!({"params":{"itemId":"file-change-1","reason":"Modifying config file"}})
    );

    // Arguments on the tool call itself are preferred over that fallback.
    let arguments = json!({"command":"npm install","cwd":"/workspace"});
    let review = permission_input(
        &permission(
            json!({"toolCallId":"command-6","kind":"execute","status":"pending","rawInput":arguments}),
            Some(json!({"params":{"reason":"Installing dependencies"}})),
        ),
        &tools,
    )
    .unwrap();
    assert_eq!(review.name, "execute");
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        arguments
    );

    // An observed name outranks the kind, because it says more about the action.
    tool_call(&terminal_command("command-6"), &mut tools).unwrap();
    assert_eq!(
        permission_input(
            &permission(
                json!({"toolCallId":"command-6","kind":"execute","status":"pending","rawInput":arguments}),
                None
            ),
            &tools
        )
        .unwrap()
        .name,
        "exec_command"
    );
}

#[test]
fn a_permission_nothing_can_be_said_about_is_refused_rather_than_reviewed_empty() {
    let tools = HashMap::new();
    for request in [
        json!({"sessionId":"session-1"}),
        json!({"sessionId":"session-1","toolCall":{"kind":"edit"}}),
        // Named, but with nothing said about what it would do.
        permission(json!({"toolCallId":"file-2","kind":"edit"}), None),
        permission(json!({"toolCallId":"file-2","kind":"edit","rawInput":null}), None),
        permission(json!({"toolCallId":"file-2","kind":"edit","rawInput":"text"}), None),
        // Asking about something it will not name.
        permission(
            json!({"toolCallId":"file-2","rawInput":{"path":"/a"}}),
            None,
        ),
    ] {
        assert!(permission_input(&request, &tools).is_err(), "{request}");
    }
}
