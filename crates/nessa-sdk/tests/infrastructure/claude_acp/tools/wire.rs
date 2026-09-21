//! Tool wire parsing and atomic sparse-update validation at the adapter boundary.
use super::*;
use crate::domain::agent_execution::tools::{FilePath, ToolContent};
use serde_json::json;

fn tool_call(
    value: &Value,
    names: &mut HashMap<String, String>,
) -> Result<ToolCallUpdate, AgentError> {
    super::tool_call(value, names, &[])
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
    assert!(tool_call(
        &json!({"toolCallId":"b","_meta":{"claudeCode":{"toolName":"Bash"}}}),
        &mut names
    )
    .is_err());
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
    assert_eq!(names.get("a").map(String::as_str), Some("Read"));
}

#[test]
fn provider_name_retention_is_bounded_by_name_identity_and_entry_limits() {
    let mut names = HashMap::new();
    let oversized = "Write".repeat(1024 * 1024);
    for id in ["first", "second"] {
        let error = tool_call(
            &json!({
                "toolCallId": id, "_meta": {"claudeCode": {"toolName": oversized}}
            }),
            &mut names,
        )
        .unwrap_err();
        // Inspect the immediate parser error, before report constructors bound it.
        let AgentError::Unsupported(message) = &error else {
            panic!("expected unsupported tool diagnostic");
        };
        assert!(
            message.len() <= 64,
            "diagnostic retained {} bytes",
            message.len()
        );
        assert_eq!(
            error,
            AgentError::Unsupported("tool is outside the configured tool profile".into())
        );
        assert!(names.is_empty());
    }
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
            .map(|(id, name)| id.len() + name.len())
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
    assert!(tool_call(
        &json!({"toolCallId": "x".repeat(257), "_meta": {"claudeCode": {"toolName": "Write"}}}),
        &mut names
    )
    .is_err());
    assert!(names.is_empty());
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
        assert!(tool_call(
            &json!({"toolCallId":"blocked","_meta":{"claudeCode":{"toolName":name}}}),
            &mut names
        )
        .is_err());
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
        assert_eq!(names.get(name).map(String::as_str), Some(name));
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
        assert!(matches!(
            tool_call(
                &json!({"toolCallId":name,"_meta":{"claudeCode":{"toolName":name}}}),
                &mut names
            ),
            Err(AgentError::Unsupported(_))
        ));
        assert!(tool_input(name, &json!({"command":"true"})).is_err());
    }
    // Denial reserves no provider name, whatever the caller sends.
    assert!(names.is_empty());
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
        names.get("future").map(String::as_str),
        Some("FutureNativeTool")
    );
    let future_args = json!({"some_field":"value"});
    assert_eq!(
        super::tool_input("FutureNativeTool", &future_args, &configured)
            .unwrap()
            .arguments_json,
        future_args.to_string()
    );
    // A denied tool is still refused, and reserves no provider name.
    assert!(matches!(
        super::tool_call(
            &json!({"toolCallId":"denied","_meta":{"claudeCode":{"toolName":"Bash"}}}),
            &mut names,
            &configured
        ),
        Err(AgentError::Unsupported(_))
    ));
    assert!(!names.contains_key("denied"));
}
