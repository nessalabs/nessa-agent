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
        Some(ObservedTool::Reviewable { name, .. }) => Some(name.clone()),
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

        let mut extended = input;
        extended.as_object_mut().unwrap().insert(
            "future_schema_field".into(),
            json!({"nested":["opaque", 7]}),
        );
        let review = tool_input(name, &extended).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
            extended,
            "{name} must preserve additive provider fields for host review"
        );
    }
    // Both spellings are in the pinned SDK's current schema; conflicts cannot
    // be hidden from the host by choosing a projection precedence.
    assert!(tool_input("Grep", &json!({"pattern":"x","context":1,"-C":2})).is_err());
    for (name, input) in [
        ("Bash", json!({"command":"true"})),
        ("Write", json!({"file_path":"/a"})),
        ("Edit", json!({"file_path":"/a","old_string":"old"})),
        ("Glob", json!({"path":"/a"})),
        ("Grep", json!({"pattern":7})),
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
            &json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Write"}},"content":[{"type":"content","content":{}}]}),
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
        ObservedTool::Reviewable { name, .. } => name.len(),
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
        "TaskOutput",
        "TaskStop",
        "Monitor",
        "REPL",
        "EnterPlanMode",
        "ExitPlanMode",
        "Workflow",
        "CronCreate",
        "CronDelete",
        "CronList",
        "ScheduleWakeup",
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

    // The pinned SDK canonicalizes these historical spellings before applying
    // permission rules. They are not a second local compatibility contract:
    // raw names reaching this parser follow the same open review path as any
    // other bounded native name.
    for alias in ["BashOutput", "KillShell"] {
        assert!(enabled_name(alias, &[]));
        tool_call(
            &json!({"toolCallId":alias,"_meta":{"claudeCode":{"toolName":alias}}}),
            &mut names,
        )
        .unwrap();
        assert_eq!(reviewable(&names, alias).as_deref(), Some(alias));
    }
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

#[test]
fn a_full_entry_budget_forgets_a_declined_call_rather_than_refusing_its_frame() {
    // Refusing the frame here would hand a provider a way to end an execution
    // by calling denied tools often enough, which is the failure the decline
    // path exists to remove. A reviewable call still cannot be admitted without
    // a place to keep it: one that cannot be retained cannot be reviewed.
    let mut names = HashMap::new();
    for i in 0..4096 {
        tool_call(
            &json!({"toolCallId": format!("{i:0256}"), "_meta":{"claudeCode":{"toolName":"Write"}}}),
            &mut names,
        )
        .unwrap();
    }
    assert_eq!(names.len(), 4096);
    tool_call(
        &json!({"toolCallId":"denied-at-the-limit","_meta":{"claudeCode":{"toolName":"Bash"}}}),
        &mut names,
    )
    .unwrap();
    assert_eq!(names.len(), 4096, "a forgotten call must not take a place");
    assert!(!names.contains_key("denied-at-the-limit"));
    assert!(tool_call(
        &json!({"toolCallId":"reviewable-at-the-limit","_meta":{"claudeCode":{"toolName":"Read"}}}),
        &mut names
    )
    .is_err());
}

#[test]
fn a_call_cannot_change_between_reviewable_and_declined_under_one_identity() {
    // What a refused call is not: two different tools wearing one identity.
    // A declined name is not retained, so a flip between two declined names is
    // not detectable here — and does not need to be, because neither is
    // reviewed. A flip across that boundary is what would change the answer.
    let mut names = HashMap::new();
    tool_call(
        &json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Read"}}}),
        &mut names,
    )
    .unwrap();
    assert!(tool_call(
        &json!({"toolCallId":"a","_meta":{"claudeCode":{"toolName":"Bash"}}}),
        &mut names
    )
    .is_err());
    let mut names = HashMap::new();
    tool_call(
        &json!({"toolCallId":"b","_meta":{"claudeCode":{"toolName":"Bash"}}}),
        &mut names,
    )
    .unwrap();
    assert!(tool_call(
        &json!({"toolCallId":"b","_meta":{"claudeCode":{"toolName":"Read"}}}),
        &mut names
    )
    .is_err());
    // Declined to declined: accepted, and still declined.
    tool_call(
        &json!({"toolCallId":"b","_meta":{"claudeCode":{"toolName":"Artifact"}}}),
        &mut names,
    )
    .unwrap();
    assert_eq!(names.get("b"), Some(&ObservedTool::Declined));
}

/// A frame of the shape the pinned adapter (0.76.0, `tools.js:335-340`) sends
/// for an MCP tool, as recorded live in `fixtures/mcp_live_frames.json`: the
/// harness name as the title and in `_meta`, kind `other`, empty input and no
/// content until completion.
fn mcp_frame(id: &str, name: &str) -> Value {
    json!({"sessionUpdate":"tool_call","toolCallId":id,"title":name,"kind":"other",
        "status":"pending","rawInput":{},"content":[],
        "_meta":{"claudeCode":{"toolName":name}}})
}

#[test]
fn an_mcp_call_names_its_configured_server_and_tool() {
    let configured = vec!["mcp__nessa__".to_owned(), "mcp__charts-app__".to_owned()];
    let mut names = HashMap::new();
    for (name, server, tool) in [
        ("mcp__nessa__shell", "nessa", "shell"),
        ("mcp__charts-app__show__v2", "charts-app", "show__v2"),
    ] {
        let update = super::tool_call(&mcp_frame(name, name), &mut names, &configured).unwrap();
        let identity = update.mcp_tool().expect(name);
        assert_eq!((identity.server(), identity.tool()), (server, tool));
        assert_eq!(reviewable(&names, name).as_deref(), Some(name));
    }
    // The completion repeats the name, and the text result stays as it was.
    let done = json!({"toolCallId":"mcp__nessa__shell","status":"completed",
        "content":[{"type":"content","content":{"type":"text","text":"{\"ok\":true}"}}],
        "_meta":{"claudeCode":{"toolName":"mcp__nessa__shell"}}});
    let update = super::tool_call(&done, &mut names, &configured).unwrap();
    assert_eq!(update.mcp_tool().unwrap().server(), "nessa");
    assert_eq!(
        update.content(),
        &Some(vec![ToolContent::text("{\"ok\":true}")])
    );
}

#[test]
fn a_call_is_left_without_an_mcp_identity_where_none_can_be_named_exactly() {
    let mut names = HashMap::new();
    // Built-in tools, a frame naming nothing, an unconfigured server, and a
    // name two configured servers both fit.
    // A configured name holds no `__` and neither starts nor ends with `_`
    // (`StdioMcpServer::problem`), so no configuration gives two fitting
    // prefixes. These are handed in directly — `a` and `a_` both fit
    // `mcp__a___c` — to show the split withholds an identity rather than
    // guess, were two ever to fit.
    let ambiguous = vec!["mcp__a__".to_owned(), "mcp__a___".to_owned()];
    for (frame, prefixes) in [
        (mcp_frame("read", "Read"), vec!["mcp__nessa__".to_owned()]),
        (
            json!({"toolCallId":"bare","title":"mcp__nessa__shell"}),
            vec!["mcp__nessa__".to_owned()],
        ),
        (
            mcp_frame("other", "mcp__other__shell"),
            vec!["mcp__nessa__".to_owned()],
        ),
        (mcp_frame("both", "mcp__a___c"), ambiguous),
    ] {
        let update = super::tool_call(&frame, &mut names, &prefixes).unwrap();
        assert_eq!(update.mcp_tool(), None, "{frame}");
    }
    // The ambiguous call is still reviewable; only its identity is withheld.
    assert_eq!(reviewable(&names, "both").as_deref(), Some("mcp__a___c"));
}

/// Every frame the Claude adapter 0.76.0 sent for five real MCP calls and the
/// tool search before one, recorded through the gateway against
/// `scripts/mcp-test-server` (see the fixture's `recorded`).
#[test]
fn recorded_claude_mcp_calls_name_their_server_and_keep_their_text() {
    use crate::domain::agent_execution::tools::ToolStatus;
    let recorded: Value =
        serde_json::from_str(include_str!("fixtures/mcp_live_frames.json")).unwrap();
    let configured = vec!["mcp__mcptest__".to_owned()];
    let mut names = HashMap::new();
    let mut last = HashMap::new();
    for (name, frames) in recorded["calls"].as_object().unwrap() {
        for frame in frames.as_array().unwrap() {
            let update = super::tool_call(frame, &mut names, &configured).unwrap();
            // One call's frames are updates to one call: the gateway draws
            // them as one part (`nessa-protocol` `tool_parts.rs`).
            assert_eq!(update.id().as_str(), frames[0]["toolCallId"], "{name}");
            // Every frame names the tool, so every frame carries the same identity.
            let identity = update.mcp_tool().map(|tool| (tool.server(), tool.tool()));
            match name.strip_prefix("mcp__mcptest__") {
                Some(tool) => assert_eq!(identity, Some(("mcptest", tool)), "{name}"),
                None => assert_eq!(identity, None, "{name}"),
            }
            last.insert(name.clone(), update);
        }
    }
    // The dotted `rows.get` arrives in the harness's spelling.
    assert!(last.contains_key("mcp__mcptest__rows_get"));
    // The CLI replaces an MCP result's text with its structured result's JSON:
    // it reaches Nessa as text, indistinguishable from any other text.
    assert_eq!(
        last["mcp__mcptest__report_rows"].content(),
        &Some(vec![ToolContent::text(
            r#"{"rows":[{"id":1,"name":"alpha","value":10},{"id":2,"name":"beta","value":20}],"total":30}"#
        )])
    );
    assert_eq!(
        last["mcp__mcptest__link_resources"]
            .content()
            .as_ref()
            .unwrap()[1],
        ToolContent::text("[Resource link: rows.csv] file:///nessa-test/rows.csv")
    );
    assert_eq!(
        last["mcp__mcptest__always_fails"].status(),
        &Some(ToolStatus::Failed)
    );
}

fn permission(
    names: &HashMap<String, ObservedTool>,
    tool: Value,
) -> Result<ToolReviewInput, AgentError> {
    super::permission_input(&json!({"toolCall": tool}), names, &[])
}

/// Claude ACP 0.76.0 announces WebSearch, then a query update, then a permission
/// whose `toolCall` is only an update. The query stays reviewable when that
/// update omits both the name and `rawInput`.
#[test]
fn websearch_review_keeps_the_query_when_the_permission_update_omits_it() {
    let mut names = HashMap::new();
    let query = json!({"query": "Rust programming language official website"});
    tool_call(
        &json!({"toolCallId":"search-1","title":"Web search","kind":"fetch","status":"pending",
            "_meta":{"claudeCode":{"toolName":"WebSearch"}}}),
        &mut names,
    )
    .unwrap();
    tool_call(
        &json!({"toolCallId":"search-1","title":"Search \"Rust programming language official website\"",
            "kind":"fetch","rawInput":query,"_meta":{"claudeCode":{"toolName":"WebSearch"}}}),
        &mut names,
    )
    .unwrap();
    let review = permission(&names, json!({"toolCallId":"search-1","status":"pending"})).unwrap();
    assert_eq!(review.name, "WebSearch");
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        query
    );
}

/// The same review when the permission frame itself carries `name` and the query,
/// which is the shape captured from the pinned harness after the announcement.
#[test]
fn websearch_review_reads_a_self_contained_permission_request() {
    let mut names = HashMap::new();
    let query = json!({"query": "Rust programming language official website"});
    tool_call(
        &json!({"toolCallId":"search-1","kind":"fetch","status":"pending",
            "_meta":{"claudeCode":{"toolName":"WebSearch"}}}),
        &mut names,
    )
    .unwrap();
    let review = permission(
        &names,
        json!({"toolCallId":"search-1","name":"WebSearch","kind":"fetch","status":"pending",
            "title":"Search \"Rust programming language official website\"","rawInput":query}),
    )
    .unwrap();
    assert_eq!(review.name, "WebSearch");
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        query
    );
}

#[test]
fn an_unobserved_permission_stays_unreadable_even_when_it_names_a_tool() {
    assert!(permission(
        &HashMap::new(),
        json!({"toolCallId":"never-observed","name":"WebSearch",
            "rawInput":{"query":"Rust programming language official website"}})
    )
    .is_err());
}

/// ACP `name` is enough to remember the call when Claude metadata is absent.
#[test]
fn websearch_name_on_the_tool_call_is_retained_for_a_later_sparse_review() {
    let mut names = HashMap::new();
    let query = json!({"query": "Rust programming language official website"});
    tool_call(
        &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":query}),
        &mut names,
    )
    .unwrap();
    let review = permission(&names, json!({"toolCallId":"search-1"})).unwrap();
    assert_eq!(review.name, "WebSearch");
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        query
    );
}

#[test]
fn a_sparse_websearch_permission_without_an_observed_query_stays_unreadable() {
    let mut names = HashMap::new();
    tool_call(
        &json!({"toolCallId":"search-1","_meta":{"claudeCode":{"toolName":"WebSearch"}}}),
        &mut names,
    )
    .unwrap();
    assert!(permission(&names, json!({"toolCallId":"search-1"})).is_err());
    assert!(permission(
        &names,
        json!({"toolCallId":"search-1","rawInput":"not an object"})
    )
    .is_err());
}

/// An object that does not fit the retention budget is not the previous query.
///
/// Other calls can fill the budget after a query was cached. The next object
/// for that call then cannot be retained. Keeping the old query would let a
/// sparse permission approve input the provider has already replaced. The cache
/// is cleared instead, whether or not this update repeats the tool name.
#[test]
fn an_unretained_query_replacement_clears_the_cached_input() {
    let max = super::MAX_RETAINED_INPUT_BYTES;
    let old = object_with_json_len(32);
    let filler = object_with_json_len(max - 32);
    let replacement = object_with_json_len(33);
    for named in [true, false] {
        let mut names = HashMap::new();
        tool_call(
            &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":old}),
            &mut names,
        )
        .unwrap();
        tool_call(
            &json!({"toolCallId":"filler","name":"WebFetch","rawInput":filler}),
            &mut names,
        )
        .unwrap();
        // Omitting the object leaves the cached query, even with a full budget.
        tool_call(
            &json!({"toolCallId":"search-1","status":"pending"}),
            &mut names,
        )
        .unwrap();
        let kept = permission(&names, json!({"toolCallId":"search-1"})).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&kept.arguments_json).unwrap(),
            old,
            "named={named}"
        );
        let update = if named {
            json!({"toolCallId":"search-1","_meta":{"claudeCode":{"toolName":"WebSearch"}},
                "rawInput":replacement})
        } else {
            json!({"toolCallId":"search-1","rawInput":replacement})
        };
        tool_call(&update, &mut names).unwrap();
        assert_no_cached_query(&names, &format!("named={named}"));
        let review = permission(
            &names,
            json!({"toolCallId":"search-1","rawInput":replacement}),
        )
        .unwrap();
        assert_eq!(review.name, "WebSearch", "named={named}");
        assert_eq!(
            serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
            replacement,
            "named={named}"
        );
    }
}

fn object_with_json_len(bytes: usize) -> Value {
    let overhead = serde_json::to_string(&json!({"q":""})).unwrap().len();
    assert!(bytes >= overhead);
    let value = json!({"q": "x".repeat(bytes - overhead)});
    assert_eq!(serde_json::to_string(&value).unwrap().len(), bytes);
    value
}

/// An object of `bytes` JSON whose text is shorter in characters than in bytes.
///
/// The retained-input budget charges UTF-8 bytes. A character count would
/// under-charge this value and free too little room for the next one.
fn object_with_multibyte_json_len(bytes: usize, mark: &str) -> Value {
    let overhead = serde_json::to_string(&json!({"q":""})).unwrap().len();
    let content_bytes = bytes.checked_sub(overhead).expect("object is at least {}");
    assert!(mark.len() >= 2 && content_bytes >= mark.len());
    let mut text = String::from(mark);
    text.push_str(&"x".repeat(content_bytes - mark.len()));
    let value = json!({"q": text});
    assert!(text.chars().count() < text.len());
    assert_eq!(serde_json::to_string(&value).unwrap().len(), bytes);
    value
}

fn query_update(named: bool, raw_input: Value) -> Value {
    if named {
        json!({"toolCallId":"search-1","_meta":{"claudeCode":{"toolName":"WebSearch"}},
            "rawInput": raw_input})
    } else {
        json!({"toolCallId":"search-1","rawInput": raw_input})
    }
}

fn assert_cached_query(names: &HashMap<String, ObservedTool>, expected: &Value, label: &str) {
    let review = permission(names, json!({"toolCallId":"search-1"})).unwrap();
    assert_eq!(review.name, "WebSearch", "{label}");
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        *expected,
        "{label}"
    );
}

/// A later object that fits is the query a sparse permission shows.
///
/// The budget is already full. The replacement fits only because this call's
/// own cached bytes are released first, and those bytes are counted in UTF-8
/// rather than characters. One extra byte clears the cache. Freeing another
/// call's input afterwards does not bring the old query back; only a new
/// object that fits does.
#[test]
fn a_fitting_replacement_is_the_query_a_sparse_permission_shows() {
    let max = super::MAX_RETAINED_INPUT_BYTES;
    let old = object_with_multibyte_json_len(32, "é");
    let filler = object_with_json_len(max - 32);
    let replacement = object_with_multibyte_json_len(32, "ü");
    let over = object_with_json_len(33);
    assert_ne!(old, replacement);
    for named in [true, false] {
        let label = format!("named={named}");
        let mut names = HashMap::new();
        tool_call(
            &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":old}),
            &mut names,
        )
        .unwrap();
        tool_call(
            &json!({"toolCallId":"filler","name":"WebFetch","rawInput":filler}),
            &mut names,
        )
        .unwrap();
        tool_call(&query_update(named, replacement.clone()), &mut names).unwrap();
        assert_cached_query(&names, &replacement, &label);
        tool_call(&query_update(named, over.clone()), &mut names).unwrap();
        assert_no_cached_query(&names, &label);
        tool_call(&json!({"toolCallId":"filler","rawInput":{}}), &mut names).unwrap();
        assert_no_cached_query(&names, &format!("{label}: budget released"));
        tool_call(&query_update(named, replacement.clone()), &mut names).unwrap();
        assert_cached_query(&names, &replacement, &format!("{label}: resent"));
    }
}

/// A supplied value that is not an object is not the cached query. Null and a
/// missing field are omissions, so they leave the query in place.
#[test]
fn a_non_object_input_clears_the_cached_query() {
    for named in [true, false] {
        for raw_input in [
            json!("next query"),
            json!(["next query"]),
            json!(1),
            json!(true),
        ] {
            let label = format!("named={named} input={raw_input}");
            let mut names = HashMap::new();
            let old = json!({"query": "old"});
            tool_call(
                &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":old}),
                &mut names,
            )
            .unwrap();
            tool_call(&query_update(named, Value::Null), &mut names).unwrap();
            assert_cached_query(&names, &old, &format!("{label}: null"));
            tool_call(
                &json!({"toolCallId":"search-1","status":"pending"}),
                &mut names,
            )
            .unwrap();
            assert_cached_query(&names, &old, &format!("{label}: omitted"));
            tool_call(&query_update(named, raw_input), &mut names).unwrap();
            assert_no_cached_query(&names, &label);
        }
    }
}

/// A finished call no longer offers its query. An active update still does.
///
/// The terminal frame repeats an object. Ignoring the status and keeping that
/// object would leave the old query approvable after the call has finished.
#[test]
fn a_finished_call_drops_its_cached_input() {
    let old = json!({"query": "old"});
    let repeated = json!({"query": "old again"});
    for status in ["completed", "failed"] {
        for named in [true, false] {
            let label = format!("status={status} named={named}");
            let mut names = HashMap::new();
            tool_call(
                &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":old}),
                &mut names,
            )
            .unwrap();
            for active in ["pending", "in_progress"] {
                let update = if named {
                    json!({"toolCallId":"search-1","_meta":{"claudeCode":{"toolName":"WebSearch"}},
                        "status":active})
                } else {
                    json!({"toolCallId":"search-1","status":active})
                };
                tool_call(&update, &mut names).unwrap();
                assert_cached_query(&names, &old, &format!("{label} {active}"));
            }
            let mut finished = query_update(named, repeated.clone());
            finished
                .as_object_mut()
                .unwrap()
                .insert("status".into(), json!(status));
            tool_call(&finished, &mut names).unwrap();
            assert_no_cached_query(&names, &label);
            let review =
                permission(&names, json!({"toolCallId":"search-1","rawInput":old})).unwrap();
            assert_eq!(
                serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
                old,
                "{label}: the permission frame's object"
            );
        }
    }
    let mut names = HashMap::new();
    tool_call(
        &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":old}),
        &mut names,
    )
    .unwrap();
    assert!(tool_call(
        &json!({"toolCallId":"search-1","name":"WebFetch","status":"completed",
            "rawInput":{"url":"https://example.com"}}),
        &mut names
    )
    .is_err());
    assert_cached_query(&names, &old, "rejected finished rename");
}

/// A permission the review declines still applies its input rule.
///
/// A non-object `rawInput` is declined before the shared runtime records the
/// frame as a tool update. The previous query must not stay approvable. A
/// frame for a call that was never observed must not create that call.
#[test]
fn a_declined_non_object_permission_drops_the_cached_query() {
    let mut names = HashMap::new();
    let old = json!({"query": "old"});
    tool_call(
        &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":old}),
        &mut names,
    )
    .unwrap();
    let request = json!({"toolCall":{"toolCallId":"search-1","name":"WebSearch",
        "rawInput":"not an object"}});
    assert!(matches!(
        permission(
            &names,
            json!({"toolCallId":"search-1","name":"WebSearch","rawInput":"not an object"})
        ),
        Err(AgentError::Protocol(_))
    ));
    super::note_declined_permission(&request, &mut names, &[]);
    assert_no_cached_query(&names, "non-object permission");
    super::note_declined_permission(
        &json!({"toolCall":{"toolCallId":"never-observed","name":"WebSearch","rawInput":"x"}}),
        &mut names,
        &[],
    );
    assert!(
        !names.contains_key("never-observed"),
        "a declined frame admitted a call"
    );
}

/// A rejected identity change is not an accepted input update.
#[test]
fn a_rejected_identity_change_keeps_the_cached_query() {
    let mut names = HashMap::new();
    let old = json!({"query": "old"});
    tool_call(
        &json!({"toolCallId":"search-1","name":"WebSearch","rawInput":old}),
        &mut names,
    )
    .unwrap();
    assert!(tool_call(
        &json!({"toolCallId":"search-1","name":"WebFetch","rawInput":{"url":"https://example.com"}}),
        &mut names
    )
    .is_err());
    assert_cached_query(&names, &old, "rejected rename");
}

fn assert_no_cached_query(names: &HashMap<String, ObservedTool>, label: &str) {
    assert!(
        matches!(
            names.get("search-1"),
            Some(ObservedTool::Reviewable {
                arguments_json: None,
                ..
            })
        ),
        "{label}"
    );
    assert!(
        matches!(
            permission(names, json!({"toolCallId":"search-1"})),
            Err(AgentError::Protocol(_))
        ),
        "{label}: the previous query stayed approvable"
    );
}

#[test]
fn a_permission_name_that_disagrees_with_the_observed_call_is_rejected() {
    let mut names = HashMap::new();
    tool_call(
        &json!({"toolCallId":"search-1","_meta":{"claudeCode":{"toolName":"WebSearch"}},
            "rawInput":{"query":"Rust programming language official website"}}),
        &mut names,
    )
    .unwrap();
    assert!(permission(
        &names,
        json!({"toolCallId":"search-1","name":"WebFetch","rawInput":{"url":"https://example.com"}})
    )
    .is_err());
}
