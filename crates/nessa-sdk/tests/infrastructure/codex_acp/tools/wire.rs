//! Codex's own tool frames reach the shared vocabulary without losing what they
//! carried, and a permission request is never reviewed without naming its tool.
use super::*;
use crate::domain::agent_execution::tools::{
    FilePath, ToolContent, ToolContentView, ToolKind, ToolObservation, ToolStatus,
    MAX_STRUCTURED_RESULT_BYTES,
};
use crate::infrastructure::mcp::STRUCTURED_RESULT_OMITTED;
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
        request["_meta"] = meta;
    }
    request
}

#[test]
fn a_terminal_pointer_is_replaced_by_the_output_it_pointed_at() {
    let mut tools = ObservedTools::default();
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

    // The completion repeats the whole output. Content is replaced rather than
    // added to, so carrying Codex's own aggregate puts the output on screen
    // once — not twice, and not only its last delta.
    let completed = tool_call(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-1","status":"completed",
                "rawOutput":{"formatted_output":"ok\n","exit_code":0},
                "_meta":{"terminal_exit":{"exit_code":0,"terminal_id":"command-1"}}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        completed.content().as_deref(),
        Some(&vec![text("ok\n")][..])
    );
}

/// A command that prints more than once, through the observation these updates
/// actually reach.
///
/// The mapper alone cannot show this: `ToolObservation::with_update` replaces an
/// observation's content instead of appending to it, so a frame carrying one
/// delta is a frame that discards every delta before it. Reading the mapper's
/// return values one at a time hides that entirely — each one looks right.
#[test]
fn every_streamed_chunk_survives_the_observation_that_replaces_content() {
    let mut tools = ObservedTools::default();
    let mut observed = ToolObservation::default();
    // The path a real update takes: the mapper's result applied to the
    // observation the session keeps, not read on its own.
    fn observe(
        observed: ToolObservation,
        frame: &Value,
        tools: &mut ObservedTools,
    ) -> ToolObservation {
        observed.with_update(tool_call(frame, tools).unwrap())
    }

    observed = observe(observed, &terminal_command("command-3"), &mut tools);
    for delta in ["compiling...\n", "running 2 tests\n", "2 tests passed\n"] {
        observed = observe(
            observed,
            &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-3",
                    "_meta":{"terminal_output_delta":{"data":delta,"terminal_id":"command-3"}}}),
            &mut tools,
        );
    }
    // Everything printed so far, in the order it was printed, and once each.
    assert_eq!(
        observed.content().as_deref(),
        Some(&vec![text("compiling...\nrunning 2 tests\n2 tests passed\n")][..])
    );

    // The completion's aggregate is the same transcript, so replacing the
    // accumulated snapshot with it leaves the output unchanged rather than
    // doubled.
    observed = observe(
        observed,
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-3","status":"completed",
                "rawOutput":{"formatted_output":"compiling...\nrunning 2 tests\n2 tests passed\n",
                             "exit_code":0}}),
        &mut tools,
    );
    assert_eq!(
        observed.content().as_deref(),
        Some(&vec![text("compiling...\nrunning 2 tests\n2 tests passed\n")][..])
    );
}

/// A command that never stops printing must not grow this adapter's memory
/// without limit, and what it keeps must stay valid text.
#[test]
fn a_command_that_outruns_the_window_keeps_its_most_recent_output() {
    let mut tools = ObservedTools::default();
    tool_call(&terminal_command("command-4"), &mut tools).unwrap();
    // Multi-byte, so a window that cut bytes rather than characters would panic
    // or retain a fragment of one.
    let chunk = "é".repeat(64 * 1024);
    let mut last = None;
    for _ in 0..40 {
        last = Some(
            tool_call(
                &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-4",
                        "_meta":{"terminal_output_delta":{"data":chunk,"terminal_id":"command-4"}}}),
                &mut tools,
            )
            .unwrap(),
        );
    }
    let content = last.unwrap();
    let ToolContentView::Text(kept) = content.content().as_deref().unwrap()[0].view() else {
        panic!("streamed output is text")
    };
    assert!(kept.len() <= MAX_STREAMED_OUTPUT_BYTES, "{}", kept.len());
    assert!(kept.len() > MAX_STREAMED_OUTPUT_BYTES - 4, "{}", kept.len());
    // What survived is the end of what was printed, still whole characters.
    assert!(kept.chars().all(|character| character == 'é'));
}

#[test]
fn output_a_tool_call_never_streamed_is_carried_from_its_completion() {
    let mut tools = ObservedTools::default();
    tool_call(&terminal_command("command-2"), &mut tools).unwrap();
    let completed = tool_call(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-2","status":"completed",
                "rawOutput":{"formatted_output":"only once\n","exit_code":0}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        completed.content().as_deref(),
        Some(&vec![text("only once\n")][..])
    );
    // The same aggregate arriving again replaces the content it already set, so
    // the output stands once however many times Codex restates it.
    let repeated = tool_call(
        &json!({"sessionUpdate":"tool_call_update","toolCallId":"command-2",
                "rawOutput":{"formatted_output":"only once\n","exit_code":0}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        repeated.content().as_deref(),
        Some(&vec![text("only once\n")][..])
    );
}

#[test]
fn diffs_and_links_survive_translation_and_unknown_shapes_do_not_pass_as_empty() {
    let mut tools = ObservedTools::default();
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
        assert!(
            tool_call(&frame, &mut ObservedTools::default()).is_err(),
            "{frame}"
        );
    }
}

#[test]
fn provider_identity_is_retained_and_bounded_and_never_silently_replaced() {
    let mut tools = ObservedTools::default();
    tool_call(&terminal_command("command-3"), &mut tools).unwrap();
    // A later frame restating only the kind is not a second identity.
    tool_call(
        &json!({"toolCallId":"command-3","kind":"execute","status":"completed"}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        permission_input(
            &permission(
                json!({"toolCallId":"command-3","kind":"execute","status":"pending",
                               "rawInput":{"command":"npm test","cwd":"/workspace"}}),
                None
            ),
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
            &mut ObservedTools::default()
        ),
        Err(AgentError::Unsupported(
            "tool name is not a reviewable identity".into()
        ))
    );
    for name in [json!(""), json!("has space"), json!(7)] {
        assert!(tool_call(
            &json!({"toolCallId":"command-5","name":name,"kind":"execute"}),
            &mut ObservedTools::default()
        )
        .is_err());
    }
}

#[test]
fn retained_identities_are_bounded_and_cleared_between_executions() {
    let mut tools = ObservedTools::default();
    for index in 0..4096 {
        tool_call(
            &json!({"toolCallId":format!("call-{index}"),"kind":"execute"}),
            &mut tools,
        )
        .unwrap();
    }
    assert_eq!(
        tool_call(
            &json!({"toolCallId":"one-too-many","kind":"execute"}),
            &mut tools
        ),
        Err(AgentError::Protocol("tool count limit exceeded".into()))
    );
    // A call already known keeps being accepted at the limit.
    assert!(tool_call(
        &json!({"toolCallId":"call-0","status":"completed"}),
        &mut tools
    )
    .is_ok());
}

#[test]
fn an_approval_asked_with_no_arguments_is_still_reviewed_with_what_codex_said() {
    let mut tools = ObservedTools::default();
    // Captured pinned-adapter shape: the edit locations are on the tool call.
    let request = permission(
        json!({"toolCallId":"file-change-1","kind":"edit","status":"pending","locations":[{"path":"/workspace/config.txt"}]}),
        Some(json!({"permission":{"version":1,"title":"Make edits?"}})),
    );
    let review = permission_input(&request, &tools).unwrap();
    assert_eq!(review.name, "edit");
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        json!({"toolCall":request["toolCall"],"metadata":request["_meta"]})
    );

    // Arguments on the tool call itself are preferred over that fallback.
    let arguments = json!({"command":"npm test","cwd":"/workspace"});
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
    let tools = ObservedTools::default();
    for request in [
        json!({"sessionId":"session-1"}),
        json!({"sessionId":"session-1","toolCall":{"kind":"edit"}}),
        // Named, but with nothing said about what it would do.
        permission(
            json!({"toolCallId":"file-2","kind":"edit","locations":[]}),
            None,
        ),
        permission(
            json!({"toolCallId":"file-2","kind":"edit","locations":[{"path":""}]}),
            None,
        ),
        permission(
            json!({"toolCallId":"file-2","kind":"execute","locations":[{"path":"/a"}]}),
            None,
        ),
        permission(json!({"toolCallId":"file-2","kind":"edit"}), None),
        permission(
            json!({"toolCallId":"file-2","kind":"edit","rawInput":null}),
            None,
        ),
        permission(
            json!({"toolCallId":"file-2","kind":"edit","rawInput":"text"}),
            None,
        ),
        // Asking about something it will not name.
        permission(
            json!({"toolCallId":"file-2","rawInput":{"path":"/a"}}),
            None,
        ),
    ] {
        assert!(permission_input(&request, &tools).is_err(), "{request}");
    }
}

#[test]
fn sparse_mcp_review_uses_only_the_original_input_for_its_tool_id() {
    let mut tools = ObservedTools::default();
    let input = json!({"server":"probe","tool":"record_probe","arguments":{"marker":"one"}});
    let frame = json!({"toolCallId":"mcp-1","kind":"execute","title":"mcp.probe.record_probe","rawInput":input});
    tool_call(&frame, &mut tools).unwrap();
    let charged = tools.input_bytes;
    tool_call(&frame, &mut tools).unwrap();
    tool_call(
        &json!({"toolCallId":"mcp-1","rawInput":null,"status":"pending"}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(tools.input_bytes, charged);
    let request = permission(
        json!({"toolCallId":"mcp-1","kind":"execute"}),
        Some(json!({"is_mcp_tool_approval":true})),
    );
    let review = permission_input(&request, &tools).unwrap();
    assert_eq!(
        serde_json::from_str::<Value>(&review.arguments_json).unwrap(),
        input
    );
    let changed =
        json!({"server":"probe","tool":"record_probe","arguments":{"marker":"different"}});
    for incoming in [json!("malformed"), changed] {
        assert!(matches!(
            tool_call(
                &json!({"toolCallId":"mcp-1","rawInput":incoming}),
                &mut tools
            ),
            Err(AgentError::Protocol(_))
        ));
        assert!(matches!(
            permission_input(
                &permission(
                    json!({"toolCallId":"mcp-1","kind":"execute","rawInput":incoming}),
                    None
                ),
                &tools
            ),
            Err(AgentError::Protocol(_))
        ));
        assert_eq!(tools.input_bytes, charged);
        assert_eq!(permission_input(&request, &tools).unwrap(), review);
    }
    assert!(permission_input(
        &permission(json!({"toolCallId":"other","kind":"execute"}), None),
        &tools
    )
    .is_err());
    tools.clear();
    assert_eq!(tools.input_bytes, 0);
    assert!(permission_input(&request, &tools).is_err());
    tool_call(&frame, &mut tools).unwrap();
    assert_eq!(permission_input(&request, &tools).unwrap(), review);
}

#[test]
fn original_input_cache_checks_the_aggregate_encoded_bound_before_retaining() {
    let mut tools = ObservedTools::default();
    let first = json!({"text":"é".repeat(1024)});
    tool_call(
        &json!({"toolCallId":"one","kind":"execute","rawInput":first}),
        &mut tools,
    )
    .unwrap();
    let overhead = json!({"text":""}).to_string().len();
    let remaining = MAX_RETAINED_INPUT_BYTES - tools.input_bytes - overhead;
    let exact = json!({"text":"x".repeat(remaining)});
    let frame = json!({"toolCallId":"two","kind":"execute","rawInput":exact});
    tool_call(&frame, &mut tools).unwrap();
    assert_eq!(tools.input_bytes, MAX_RETAINED_INPUT_BYTES);
    tool_call(&frame, &mut tools).unwrap();
    let error = tool_call(
        &json!({"toolCallId":"three","kind":"execute","rawInput":{}}),
        &mut tools,
    )
    .unwrap_err();
    assert_eq!(
        error,
        AgentError::Protocol("tool input retention limit exceeded".into())
    );
    assert!(!tools.entries.contains_key("three"));
    tools.clear();
    tool_call(&frame, &mut tools).unwrap();
    assert_eq!(tools.input_bytes, exact.to_string().len());
}

#[test]
fn rejected_observations_do_not_retain_input_or_charge_its_budget() {
    let mut tools = ObservedTools::default();
    for frame in [
        json!({"toolCallId":"bad","rawInput":"malformed"}),
        json!({"toolCallId":"bad","kind":"invalid","rawInput":{"text":"one"}}),
        json!({"toolCallId":"bad","name":7,"rawInput":{"text":"one"}}),
    ] {
        assert!(matches!(
            tool_call(&frame, &mut tools),
            Err(AgentError::Protocol(_))
        ));
        assert!(tools.entries.is_empty());
        assert_eq!(tools.input_bytes, 0);
    }
    tool_call(
        &json!({"toolCallId":"bad","kind":"execute","rawInput":{"text":"valid"}}),
        &mut tools,
    )
    .unwrap();
    assert_eq!(tools.input_bytes, json!({"text":"valid"}).to_string().len());
}

/// The announcement codex-acp 1.12.0 sends for an MCP call
/// (`index.js:23035-23045`, and as recorded live in `fixtures/mcp_live_frames.json`):
/// dotted title, `execute`, `in_progress`, exact names in `rawInput`, and the
/// MCP marker in `_meta`.
fn mcp_call(id: &str) -> Value {
    json!({"sessionUpdate":"tool_call","toolCallId":id,"title":"mcp.charts.app.show","kind":"execute","status":"in_progress",
        "rawInput":{"server":"charts.app","tool":"show","arguments":{"n":2}},
        "_meta":{"is_mcp_tool_call":true}})
}

/// Its completion (`index.js:25123-25130`), exactly: `rawInput` again, and
/// the MCP result only in `rawOutput` — with no ACP content and no MCP marker,
/// which only the announcement carries.
fn mcp_done(id: &str, output: Value) -> Value {
    json!({"sessionUpdate":"tool_call_update","toolCallId":id,"status":"completed",
        "rawInput":{"server":"charts.app","tool":"show","arguments":{"n":2}},
        "rawOutput":output})
}

/// A session replay (`index.js:33968`): one announcement carrying the marker,
/// the input and the result together.
fn mcp_replayed(id: &str, output: Value) -> Value {
    let mut frame = mcp_call(id);
    frame["status"] = json!("completed");
    frame["rawOutput"] = output;
    frame
}

#[test]
fn an_mcp_call_names_its_server_and_tool_from_raw_input_never_the_dotted_title() {
    let mut tools = ObservedTools::default();
    let update = tool_call(&mcp_call("mcp-1"), &mut tools).unwrap();
    let identity = update.mcp_tool().unwrap();
    assert_eq!((identity.server(), identity.tool()), ("charts.app", "show"));
    assert_eq!(update.content(), &None);
    // Without the marker, or with names the domain will not keep, the call is
    // shown as before and names nothing.
    let mut unmarked = mcp_call("mcp-2");
    unmarked["_meta"] = json!({});
    let mut spaced = mcp_call("mcp-3");
    spaced["rawInput"]["server"] = json!("two words");
    let mut missing = mcp_call("mcp-4");
    missing["rawInput"] = json!({"arguments":{}});
    for frame in [unmarked, spaced, missing, terminal_command("shell")] {
        let update = tool_call(&frame, &mut tools).unwrap();
        assert_eq!(update.mcp_tool(), None, "{frame}");
    }
}

#[test]
fn an_mcp_result_keeps_its_text_blocks_structured_result_and_error() {
    let mut tools = ObservedTools::default();
    tool_call(&mcp_call("mcp-1"), &mut tools).unwrap();
    let structured = json!({"rows":[{"x":1,"y":"é"}]});
    let output = json!({"result":{"content":[
            {"type":"text","text":"Two rows."},
            {"type":"resource_link","uri":"file:///rows.csv","name":"rows"},
            {"type":"resource","resource":{"uri":"ui://charts/a","text":"<p>embedded</p>"}},
            {"type":"resource","resource":{"uri":"file:///b.bin","blob":"AAAA"}},
            {"type":"image","data":"AAAA","mimeType":"image/png"}
        ],"structuredContent":structured,"_meta":{"ui":{"note":"not kept"}}},"error":null});
    let update = tool_call(&mcp_done("mcp-1", output), &mut tools).unwrap();
    assert_eq!(update.mcp_tool().unwrap().tool(), "show");
    assert_eq!(
        update.content(),
        &Some(vec![
            text("Two rows."),
            text("file:///rows.csv"),
            text("<p>embedded</p>"),
            text(UNSUPPORTED_TOOL_CONTENT),
            text(UNSUPPORTED_TOOL_CONTENT),
            ToolContent::structured(structured.to_string()).unwrap(),
        ])
    );
    // Empty text, and an empty error, are results: kept exactly, not refused.
    let empty = tool_call(
        &mcp_done(
            "mcp-1",
            json!({"result":{"content":[{"type":"text","text":""}],"structuredContent":{}},
                "error":{"message":""}}),
        ),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        empty.content(),
        &Some(vec![
            text(""),
            ToolContent::structured("{}").unwrap(),
            text("")
        ])
    );
    let failed = tool_call(
        &mcp_done(
            "mcp-1",
            json!({"result":null,"error":{"message":"server went away"}}),
        ),
        &mut tools,
    )
    .unwrap();
    assert_eq!(failed.content(), &Some(vec![text("server went away")]));
    let replayed = tool_call(
        &mcp_replayed(
            "mcp-9",
            json!({"result":{"content":[{"type":"text","text":"again"}]}}),
        ),
        &mut tools,
    )
    .unwrap();
    assert_eq!(replayed.mcp_tool().unwrap().server(), "charts.app");
    assert_eq!(replayed.content(), &Some(vec![text("again")]));
    // The same output on a call never marked as MCP is not read as an MCP result.
    let unmarked = tool_call(
        &mcp_done(
            "shell-1",
            json!({"result":{"content":[{"type":"text","text":"no"}]}}),
        ),
        &mut tools,
    )
    .unwrap();
    assert_eq!(unmarked.content(), &None);
    assert_eq!(unmarked.mcp_tool(), None);
    // No output yet leaves the observed content as it was.
    let pending = tool_call(&mcp_done("mcp-1", Value::Null), &mut tools).unwrap();
    assert_eq!(pending.content(), &None);
}

#[test]
fn an_oversize_structured_result_is_said_rather_than_kept_or_refused() {
    let mut tools = ObservedTools::default();
    for id in ["mcp-1", "mcp-2"] {
        tool_call(&mcp_call(id), &mut tools).unwrap();
    }
    let exact = "a".repeat(MAX_STRUCTURED_RESULT_BYTES - 2);
    let kept = tool_call(
        &mcp_done(
            "mcp-1",
            json!({"result":{"content":[],"structuredContent":exact}}),
        ),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        kept.content(),
        &Some(vec![
            ToolContent::structured(format!("\"{exact}\"")).unwrap()
        ])
    );
    let over = tool_call(
        &mcp_done(
            "mcp-2",
            json!({"result":{"content":[{"type":"text","text":"big"}],
            "structuredContent":format!("{exact}a")}}),
        ),
        &mut tools,
    )
    .unwrap();
    assert_eq!(
        over.content(),
        &Some(vec![text("big"), text(STRUCTURED_RESULT_OMITTED)])
    );
}

#[test]
fn a_malformed_mcp_result_refuses_the_frame_before_anything_is_retained() {
    let mut tools = ObservedTools::default();
    for output in [
        json!("text"),
        json!({"result":"text"}),
        json!({"result":{"content":"text"}}),
        json!({"result":{"content":[{"text":"no type"}]}}),
        json!({"result":{"content":[{"type":"text"}]}}),
        json!({"result":{"content":[{"type":"resource"}]}}),
        json!({"result":{"content":[{"type":"resource","resource":{"text":7}}]}}),
        json!({"error":{"code":1}}),
    ] {
        assert!(
            matches!(
                tool_call(&mcp_replayed("bad", output.clone()), &mut tools),
                Err(AgentError::Protocol(_))
            ),
            "{output}"
        );
        assert!(tools.entries.is_empty());
    }
}

/// Every frame codex-acp 1.12.0 sent for five real MCP calls, recorded through
/// the gateway against `scripts/mcp-test-server` (see the fixture's `recorded`).
#[test]
fn recorded_codex_mcp_calls_carry_identity_text_and_structured_results() {
    let recorded: Value =
        serde_json::from_str(include_str!("fixtures/mcp_live_frames.json")).unwrap();
    let mut tools = ObservedTools::default();
    let mut last = HashMap::new();
    for (tool, frames) in recorded["calls"].as_object().unwrap() {
        let mut named = Vec::new();
        for frame in frames.as_array().unwrap() {
            let update = tool_call(frame, &mut tools).unwrap();
            if let Some(identity) = update.mcp_tool() {
                named.push((identity.server().to_owned(), identity.tool().to_owned()));
            }
            last.insert(tool.clone(), update);
        }
        // Named exactly, and the same on every frame that names it.
        assert!(!named.is_empty(), "{tool}");
        assert!(
            named
                .iter()
                .all(|pair| pair == &("mcptest".to_owned(), tool.clone())),
            "{tool}: {named:?}"
        );
    }
    let content = |tool: &str| last[tool].content().clone().unwrap();
    assert_eq!(
        content("report_rows"),
        vec![
            text("Two rows: alpha (10), beta (20)."),
            ToolContent::structured(
                r#"{"rows":[{"id":1,"name":"alpha","value":10},{"id":2,"name":"beta","value":20}],"total":30}"#
            )
            .unwrap(),
        ]
    );
    assert_eq!(
        content("link_resources"),
        vec![
            text("One link and one embedded resource follow."),
            text("file:///nessa-test/rows.csv"),
            text("Embedded note from nessa-test."),
        ]
    );
    assert_eq!(
        content("rows.get"),
        vec![
            text("Row 2: beta"),
            ToolContent::structured(r#"{"id":2,"name":"beta","value":20}"#).unwrap(),
        ]
    );
    // Codex reports an MCP error result as a failed call, its text kept.
    assert_eq!(last["always_fails"].status(), &Some(ToolStatus::Failed));
    assert_eq!(
        content("always_fails"),
        vec![text("This tool always fails, on purpose.")]
    );
    assert_eq!(content("show_chart")[0], text("Chart of two rows."));
}
