//! Tool wire parsing and atomic sparse-update validation at the adapter boundary.
use super::*;
use crate::domain::agent_execution::tools::{FilePath, ToolContent};
use serde_json::json;

fn tool_call(
    value: &Value,
    names: &mut HashMap<String, ObservedTool>,
) -> Result<ToolCallUpdate, AgentError> {
    super::tool_call(value, names, &[])
}

/// The name this binding retained for `id`, or `None` where it retained none.
fn reviewable(names: &HashMap<String, ObservedTool>, id: &str) -> Option<String> {
    match names.get(id) {
        Some(ObservedTool::Reviewable(name)) => Some(name.clone()),
        _ => None,
    }
}

fn tool_input(name: &str, value: &Value) -> Result<ToolReviewInput, AgentError> {
    super::tool_input(name, value, &[])
}

#[test]
fn validates_claude_schemas_and_preserves_complete_review_arguments() {
    for (name, input) in [
        (
            "Read",
            json!({"file_path":"/a","offset":2,"limit":5,"pages":"1-2"}),
        ),
        ("Write", json!({"file_path":"/a","content":"new"})),
        (
            "Edit",
            json!({"file_path":"/a","old_string":"old","new_string":"new"}),
        ),
        ("Glob", json!({"pattern":"*.rs"})),
        ("Grep", json!({"pattern":"x","context":2,"-o":true})),
    ] {
        let review = tool_input(name, &input).unwrap();
        assert_eq!(review.name, name);
        assert_eq!(
            serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
            input
        );
    }
    // Both spellings are in the pinned SDK's current schema; conflicts cannot
    // be hidden from the host by choosing a projection precedence.
    assert!(tool_input("Grep", &json!({"pattern":"x","context":1,"-C":2})).is_err());
    for (name, input) in [
        ("Bash", json!({"command":"true"})),
        (
            "Write",
            json!({"file_path":"/a","content":"x","command":"true"}),
        ),
        ("Read", json!({"file_path":"/a","offset":-1})),
        ("Write", json!({"file_path":"","content":"x"})),
        ("Read", json!({"file_path":"a\0b"})),
        ("Glob", json!({"pattern":"*","path":""})),
    ] {
        assert!(tool_input(name, &input).is_err());
    }
}

#[test]
fn sparse_tool_updates_keep_omission_distinct_from_empty_and_preserve_diffs() {
    let mut names = HashMap::new();
    let tool = tool_call(&json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Write"}},"content":[{"type":"diff","path":"/a","oldText":null,"newText":"new"}]}),&mut names).unwrap();
    assert_eq!(
        tool.content().clone(),
        Some(vec![ToolContent::diff(
            FilePath::new("/a").unwrap(),
            None,
            "new"
        )])
    );
    let patch = tool_call(&json!({"toolCallId":"a","content":[]}), &mut names).unwrap();
    assert_eq!(patch.content().clone(), Some(vec![]));
    assert_eq!(patch.locations().clone(), None);
    assert!(tool_call(&json!({"toolCallId":" "}), &mut names).is_err());
    assert!(tool_call(
        &json!({"toolCallId":"a", "locations":[{"path":""}]}),
        &mut names
    )
    .is_err());
    assert!(tool_call(
        &json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Edit"}}}),
        &mut names
    )
    .is_err());
    // A denied tool is observed rather than refused: the call happened, and the
    // person watching should see it. What it does not get is a review.
    tool_call(
        &json!({"toolCallId":"b","_meta":{"claudeCode":{"toolName":"Bash"}}}),
        &mut names,
    )
    .unwrap();
    assert_eq!(names.get("b"), Some(&ObservedTool::Declined));
}
#[test]
fn invalid_content_does_not_reserve_a_provider_tool_name() {
    let mut names = HashMap::new();
    assert!(matches!(
        tool_call(
            &json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Write"}},"content":[{"type":"content","content":{"type":"image"}}]}),
            &mut names
        ),
        Err(AgentError::Protocol(_))
    ));
    assert!(names.is_empty());
    tool_call(
        &json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Read"}},"content":[]}),
        &mut names,
    )
    .unwrap();
    assert_eq!(reviewable(&names, "a").as_deref(), Some("Read"));
}

#[test]
fn provider_name_retention_is_bounded_by_name_identity_and_entry_limits() {
    let mut names = HashMap::new();
    let oversized = "Write".repeat(1024 * 1024);
    // A name too large to be a name is refused as one. The frame is still
    // observed — refusing the frame is what ended whole executions — but
    // nothing of that name is retained, so a caller cannot spend this map's
    // budget by choosing a long enough tool name.
    for id in ["first", "second"] {
        tool_call(
            &json!({
                "toolCallId": id, "_meta": {"claudeCode": {"toolName": oversized}}
            }),
            &mut names,
        )
        .unwrap();
        assert_eq!(names.get(id), Some(&ObservedTool::Declined));
        assert_eq!(reviewable(&names, id), None);
    }
    assert_eq!(
        names
            .iter()
            .map(|(id, observed)| id.len() + observed_bytes(observed))
            .sum::<usize>(),
        "first".len() + "second".len()
    );
    names.clear();
    for i in 0..4096 {
        let id = format!("{i:0256}");
        tool_call(
            &json!({
                "toolCallId": id, "_meta": {"claudeCode": {"toolName": "Write"}}
            }),
            &mut names,
        )
        .unwrap();
    }
    assert_eq!(names.len(), 4096);
    assert_eq!(
        names
            .iter()
            .map(|(id, observed)| id.len() + observed_bytes(observed))
            .sum::<usize>(),
        4096 * (256 + 5)
    );
    let existing = format!("{:0256}", 0);
    tool_call(
        &json!({"toolCallId": existing, "_meta": {"claudeCode": {"toolName": "Write"}}}),
        &mut names,
    )
    .unwrap();
    assert!(tool_call(
        &json!({"toolCallId": "overflow", "_meta": {"claudeCode": {"toolName": "Read"}}}),
        &mut names
    )
    .is_err());
    assert_eq!(names.len(), 4096);
    names.clear();
    // An identity too large to be an identity is still refused outright: there
    // is nothing to place the observation under.
    assert!(tool_call(
        &json!({"toolCallId": "x".repeat(257), "_meta": {"claudeCode": {"toolName": "Write"}}}),
        &mut names
    )
    .is_err());
    assert!(names.is_empty());
}

/// Name bytes this binding retained for one observed call.
fn observed_bytes(observed: &ObservedTool) -> usize {
    match observed {
        ObservedTool::Reviewable(name) => name.len(),
        ObservedTool::Declined => 0,
    }
}

#[test]
fn enabled_native_inputs_are_preserved_and_unmanaged_shell_is_rejected() {
    let mut names = HashMap::new();
    for (name, kind, args) in [
        (
            "WebSearch",
            "fetch",
            json!({"query":"Rust Shepherd process supervision", "allowed_domains":["github.com"]}),
        ),
        (
            "WebFetch",
            "fetch",
            json!({"url":"https://example.com", "prompt":"Summarize"}),
        ),
        (
            "Agent",
            "think",
            json!({"prompt":"Read project documentation", "description":"Inspect docs"}),
        ),
    ] {
        tool_call(
            &json!({"toolCallId":name,"kind":kind,"_meta":{"claudeCode":{"toolName":name}}}),
            &mut names,
        )
        .unwrap();
        assert_eq!(
            tool_input(name, &args).unwrap().arguments_json,
            args.to_string()
        );
    }
    for name in DISALLOWED_TOOLS {
        tool_call(
            &json!({"toolCallId":"blocked","_meta":{"claudeCode":{"toolName":name}}}),
            &mut names,
        )
        .unwrap();
        assert_eq!(names.get("blocked"), Some(&ObservedTool::Declined));
        assert!(tool_input(name, &json!({"command":"true"})).is_err());
    }
    assert!(tool_input("WebSearch", &json!("not an object")).is_err());
}

#[test]
fn deferred_schema_loading_is_admitted_before_the_tool_it_loads() {
    // The pinned harness defers tool schemas: a turn that needs WebSearch first
    // calls ToolSearch to load it. Refusing the loader refused the whole turn.
    let mut names = HashMap::new();
    for (name, kind) in [("ToolSearch", "other"), ("WebSearch", "fetch")] {
        tool_call(
            &json!({"toolCallId":name,"kind":kind,"_meta":{"claudeCode":{"toolName":name}}}),
            &mut names,
        )
        .unwrap();
        assert_eq!(reviewable(&names, name).as_deref(), Some(name));
    }
    let args = json!({"query":"select:WebSearch","max_results":5});
    let review = tool_input("ToolSearch", &args).unwrap();
    assert_eq!(review.name, "ToolSearch");
    assert_eq!(review.arguments_json, args.to_string());
}

#[test]
fn execution_and_escaping_tools_are_denied_even_though_admission_is_open() {
    // Admission is open, so this list is the whole boundary. Each name here is
    // a way out of what Nessa owns: shell execution outside Shepherd, the
    // permission mode itself, work that outlives its execution, and effects on
    // services beyond this machine. A permission prompt is not ownership.
    let mut names = HashMap::new();
    for name in [
        "Bash",
        "BashOutput",
        "KillShell",
        "Monitor",
        "REPL",
        "EnterPlanMode",
        "ExitPlanMode",
        "Workflow",
        "CronCreate",
        "CronDelete",
        "CronList",
        "EnterWorktree",
        "ExitWorktree",
        "Artifact",
        "PushNotification",
        "RemoteTrigger",
        "SendFeedback",
    ] {
        assert!(
            DISALLOWED_TOOLS.contains(&name),
            "{name} must stay denied once admission is open"
        );
        assert!(!enabled_name(name, &[]), "denied tool {name} was admitted");
        // The call is observed so it can be shown, and refused where the
        // refusal can be answered: the review. Ending the execution over it is
        // what left a caller staring at silence.
        tool_call(
            &json!({"toolCallId":name,"_meta":{"claudeCode":{"toolName":name}}}),
            &mut names,
        )
        .unwrap();
        assert_eq!(names.get(name as &str), Some(&ObservedTool::Declined));
        assert!(matches!(
            tool_input(name, &json!({"command":"true"})),
            Err(AgentError::Unsupported(_) | AgentError::Protocol(_))
        ));
    }
    // Denial reserves no provider name, whatever the caller sends.
    assert!(names
        .values()
        .all(|observed| *observed == ObservedTool::Declined));
    assert_eq!(names.len(), DISALLOWED_TOOLS.len());
}

#[test]
fn admission_reviews_every_harness_tool_except_denials_and_unconfigured_namespaces() {
    // Which built-ins the pinned harness offers is its fact, reached through
    // deferred schema loading. Nessa reviews them all rather than refusing the
    // ones it has not heard of, which used to fail the whole execution.
    for name in [
        "Read",
        "Write",
        "Edit",
        "Glob",
        "Grep",
        "NotebookEdit",
        "WebSearch",
        "WebFetch",
        "Agent",
        "Task",
        "TodoWrite",
        "Skill",
        "ToolSearch",
        "FutureNativeTool",
    ] {
        assert!(enabled_name(name, &[]), "reviewable harness tool {name}");
    }
    for name in DISALLOWED_TOOLS {
        assert!(!enabled_name(name, &[]), "denied native tool {name}");
    }
    for name in ["", " Read", "Read!", &"T".repeat(129)] {
        assert!(!enabled_name(name, &[]), "unbounded tool name {name:?}");
    }
    // An MCP name must belong to a configured server rather than merely look
    // like one, whatever the harness offers.
    for name in ["mcp__nessa__shell", "mcp__nessa__", "mcp__other__shell"] {
        assert!(!enabled_name(name, &[]), "unconfigured tool {name}");
    }
    let configured = vec!["mcp__nessa__".to_owned()];
    assert!(enabled_name("mcp__nessa__shell", &configured));
    assert!(!enabled_name("mcp__nessa__", &configured));
    assert!(!enabled_name("mcp__other__shell", &configured));

    let mut names = HashMap::new();
    let call = json!({
        "toolCallId":"mcp",
        "kind":"other",
        "_meta":{"claudeCode":{"toolName":"mcp__nessa__shell"}}
    });
    super::tool_call(&call, &mut names, &configured).unwrap();
    let args = json!({"command":"printf '%s' ' a\\b '\n", "timeoutSeconds":5});
    assert_eq!(
        super::tool_input("mcp__nessa__shell", &args, &configured)
            .unwrap()
            .arguments_json,
        args.to_string()
    );

    // A built-in Nessa has never heard of is reviewed with its input preserved,
    // not refused: refusing it ended the turn with nothing to show the caller.
    let future = json!({
        "toolCallId":"future",
        "kind":"other",
        "_meta":{"claudeCode":{"toolName":"FutureNativeTool"}}
    });
    super::tool_call(&future, &mut names, &configured).unwrap();
    assert_eq!(
        reviewable(&names, "future").as_deref(),
        Some("FutureNativeTool")
    );
    let future_args = json!({"some_field":"value"});
    assert_eq!(
        super::tool_input("FutureNativeTool", &future_args, &configured)
            .unwrap()
            .arguments_json,
        future_args.to_string()
    );
    // A denied tool is observed without reserving a provider name; the review
    // is where it is refused.
    super::tool_call(
        &json!({"toolCallId":"denied","_meta":{"claudeCode":{"toolName":"Bash"}}}),
        &mut names,
        &configured,
    )
    .unwrap();
    assert_eq!(names.get("denied"), Some(&ObservedTool::Declined));
    assert_eq!(reviewable(&names, "denied"), None);
}
