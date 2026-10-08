use super::support::*;
use nessa_sdk::domain::agent_execution::sessions::{ExecutionSession, ExecutionSessionId};

fn session(execution: &ExecutionId) -> ExecutionSession {
    let mut session = ExecutionSession::new(ExecutionSessionId::new("session").unwrap());
    session.begin_execution(execution.clone()).unwrap();
    session
}

#[test]
fn paths_are_descriptions_not_filesystem_authority() {
    for invalid in ["", "a\0b"] {
        assert_eq!(FilePath::new(invalid), Err(ExecutionError::InvalidPath));
    }
    for valid in ["../outside", "/not-a-local-file", "C:\\remote\\file", " "] {
        assert_eq!(FilePath::new(valid).unwrap().as_str(), valid);
    }
}

#[test]
fn tool_observations_merge_without_losing_omitted_fields_or_crossing_identity() {
    let execution = ExecutionId::new("execution").unwrap();
    let original = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some("Read file".into()),
        Some(ToolKind::Read),
        Some(ToolStatus::Running),
        Some(vec![FileLocation::new(
            FilePath::new("a").unwrap(),
            Some(0),
        )]),
        Some(vec![ToolContent::text("old output")]),
    );
    let mut session = session(&execution);
    session.observe_tool(&execution, original.clone()).unwrap();
    let tool = session.tool(original.id()).unwrap();
    assert_eq!(tool.execution_id(), &execution);
    assert_eq!(tool.observation().title(), original.title());
    assert_eq!(tool.observation().kind(), original.kind());
    assert_eq!(tool.observation().status(), original.status());
    let location = &tool.observation().locations().as_ref().unwrap()[0];
    assert_eq!(location.path().as_str(), "a");
    assert_eq!(location.line(), Some(0));
    session.observe_tool(&execution, update("tool")).unwrap();
    let tool = session.tool(original.id()).unwrap();
    assert_eq!(tool.observation().title(), original.title());
    assert_eq!(tool.observation().kind(), original.kind());
    assert_eq!(tool.observation().status(), original.status());
    let snapshot = tool.observation().clone();
    assert_eq!(tool.id(), original.id());
    assert_eq!(
        session.observe_tool(&ExecutionId::new("other").unwrap(), update("tool")),
        Err(ExecutionError::DifferentExecution)
    );
    assert_eq!(
        session.tool(original.id()).unwrap().observation(),
        &snapshot
    );
    session.observe_tool(&execution, update("other")).unwrap();
    assert_eq!(
        session.tool(original.id()).unwrap().observation(),
        &snapshot
    );
    assert_ne!(
        session
            .tool(&ToolCallId::new("other").unwrap())
            .unwrap()
            .id(),
        original.id()
    );
    let cleared = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some(String::new()),
        Some(ToolKind::Other),
        Some(ToolStatus::Completed),
        Some(vec![]),
        Some(vec![]),
    );
    session.observe_tool(&execution, cleared.clone()).unwrap();
    let tool = session.tool(original.id()).unwrap();
    assert_eq!(tool.observation().title(), cleared.title());
    assert_eq!(tool.observation().kind(), cleared.kind());
    assert_eq!(tool.observation().status(), cleared.status());
    assert_eq!(tool.observation().locations(), cleared.locations());
    assert_eq!(tool.observation().content(), cleared.content());
    assert_eq!(snapshot.title().as_deref(), Some("Read file"));
    assert_ne!(&snapshot, tool.observation());
    assert_eq!(original.title().as_deref(), Some("Read file"));
}

#[test]
fn sparse_tool_updates_preserve_allocations_and_move_replacements() {
    let execution = ExecutionId::new("execution").unwrap();
    let mut title = String::with_capacity(128);
    title.push_str("Read file");
    let original_title = title.as_ptr();
    let original_content = vec![ToolContent::text("large output".repeat(1024))];
    let original_content_ptr = original_content.as_ptr();
    let id = ToolCallId::new("tool").unwrap();
    let mut session = session(&execution);
    session
        .observe_tool(
            &execution,
            ToolCallUpdate::new(
                ToolCallId::new("tool").unwrap(),
                Some(title),
                None,
                None,
                None,
                Some(original_content),
            ),
        )
        .unwrap();
    let tool = session.tool(&id).unwrap();
    let retained = tool.payload_bytes();
    let status = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        Some(ToolStatus::Running),
        None,
        None,
    );
    assert_eq!(tool.payload_bytes_after(&status), retained);
    session.observe_tool(&execution, status).unwrap();
    let tool = session.tool(&id).unwrap();
    assert_eq!(
        tool.observation().title().as_ref().unwrap().as_ptr(),
        original_title
    );
    assert_eq!(
        tool.observation().content().as_ref().unwrap().as_ptr(),
        original_content_ptr
    );
    assert_eq!(tool.payload_bytes(), retained);

    let replacement = vec![ToolContent::text("replacement")];
    let replacement_ptr = replacement.as_ptr();
    let patch = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        None,
        Some(replacement),
    );
    let predicted = tool.payload_bytes_after(&patch);
    session.observe_tool(&execution, patch).unwrap();
    let tool = session.tool(&id).unwrap();
    assert_eq!(
        tool.observation().content().as_ref().unwrap().as_ptr(),
        replacement_ptr
    );
    assert_eq!(tool.payload_bytes(), predicted);
    assert!(predicted < retained);

    let clear = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some(String::new()),
        None,
        None,
        Some(vec![]),
        Some(vec![]),
    );
    assert_eq!(
        tool.payload_bytes_after(&clear),
        execution.as_str().len() + 4
    );
    session.observe_tool(&execution, clear).unwrap();
    let tool = session.tool(&id).unwrap();
    assert_eq!(
        tool.payload_bytes(),
        tool.execution_id().as_str().len() + tool.id().as_str().len()
    );
}

#[test]
fn tool_payload_accounting_includes_spare_capacity_empty_entries_and_diffs() {
    let mut title = String::with_capacity(1024);
    title.push('x');
    let mut locations = Vec::with_capacity(8);
    locations.push(FileLocation::new(FilePath::new("path").unwrap(), None));
    let mut content = Vec::with_capacity(16);
    content.push(ToolContent::text(String::new()));
    content.push(ToolContent::diff(
        FilePath::new("diff").unwrap(),
        Some("old".into()),
        "new",
    ));
    let expected = title.capacity()
        + locations.capacity() * std::mem::size_of::<FileLocation>()
        + 4
        + content.capacity() * std::mem::size_of::<ToolContent>()
        + 4
        + 3
        + 3;
    let observation = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some(title),
        None,
        None,
        Some(locations),
        Some(content),
    );
    assert_eq!(observation.payload_bytes(), expected);
}

#[test]
fn absent_tool_content_has_no_payload_allocation() {
    let observation = update("tool");
    assert_eq!(observation.payload_bytes(), 0);
    let execution = ExecutionId::new("execution").unwrap();
    let mut session = session(&execution);
    session.observe_tool(&execution, observation).unwrap();
    let tool = session.tool(&ToolCallId::new("tool").unwrap()).unwrap();
    assert_eq!(
        tool.payload_bytes(),
        tool.execution_id().as_str().len() + tool.id().as_str().len()
    );
    assert_eq!(
        tool.payload_bytes_after(&update("tool")),
        tool.execution_id().as_str().len() + 4
    );
}

#[test]
fn tool_payload_includes_large_retained_identities_before_and_after_updates() {
    let execution = ExecutionId::new("e".repeat(ExecutionId::MAX_BYTES)).unwrap();
    let id = ToolCallId::new("t".repeat(1024 * 1024)).unwrap();
    let first = ToolCallUpdate::new(id.clone(), None, None, None, None, None);
    let expected = ExecutionId::MAX_BYTES + 1024 * 1024;
    assert_eq!(
        ToolCall::initial_payload_bytes(&execution, &first),
        expected
    );
    let mut session = session(&execution);
    session.observe_tool(&execution, first).unwrap();
    let tool = session.tool(&id).unwrap();
    assert_eq!(tool.payload_bytes(), expected);
    let patch = ToolCallUpdate::new(id.clone(), Some("title".into()), None, None, None, None);
    assert_eq!(tool.payload_bytes_after(&patch), expected + 5);
    session.observe_tool(&execution, patch).unwrap();
    let tool = session.tool(&id).unwrap();
    assert_eq!(tool.payload_bytes(), expected + 5);
}

#[test]
fn tool_content_is_compact_exact_and_distinguishes_unknown_from_empty_prior_text() {
    let reserved = |text: &str| {
        let mut value = String::with_capacity(8192);
        value.push_str(text);
        value
    };
    for text in ["", " ", "é"] {
        let value = ToolContent::text(reserved(text));
        assert_eq!(value.view(), ToolContentView::Text(text));
        assert_eq!(value.payload_bytes(), text.len());
        assert_eq!(value.clone(), value);
    }
    let path = FilePath::new(reserved("file")).unwrap();
    let unknown = ToolContent::diff(path.clone(), None, reserved(""));
    let empty = ToolContent::diff(path.clone(), Some(reserved("")), reserved(""));
    assert_ne!(unknown, empty);
    assert_eq!(
        unknown.view(),
        ToolContentView::Diff {
            path: &path,
            old: None,
            new: ""
        }
    );
    assert_eq!(
        empty.view(),
        ToolContentView::Diff {
            path: &path,
            old: Some(""),
            new: ""
        }
    );
    // Cloning FilePath may compact it; use the owned path to prove accounting
    // includes capacity still retained by an immutable path value.
    let changed = ToolContent::diff(path, Some(reserved("é")), reserved("new"));
    assert_eq!(changed.payload_bytes(), 8192 + "é".len() + 3);
    let update = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        None,
        None,
        None,
        None,
        Some(vec![changed]),
    );
    assert_eq!(
        update.payload_bytes(),
        std::mem::size_of::<ToolContent>() + 8192 + 2 + 3
    );
}

#[test]
fn mcp_tool_names_are_bounded_single_line_and_kept_exactly() {
    let tool = McpTool::new("nessa-apps", "show_chart.v2").unwrap();
    assert_eq!(
        (tool.server(), tool.tool()),
        ("nessa-apps", "show_chart.v2")
    );
    assert_eq!(
        tool.payload_bytes(),
        "nessa-apps".len() + "show_chart.v2".len()
    );
    let longest = "é".repeat(MAX_MCP_NAME_BYTES / 2);
    let kept = McpTool::new(longest.clone(), longest.clone()).unwrap();
    assert_eq!(
        (kept.server(), kept.tool()),
        (longest.as_str(), longest.as_str())
    );
    let too_long = format!("{longest}x");
    for (server, tool, expected) in [
        ("", "tool", ExecutionError::EmptyValue("MCP server name")),
        ("server", "", ExecutionError::EmptyValue("MCP tool name")),
        (
            too_long.as_str(),
            "tool",
            ExecutionError::ValueTooLong {
                field: "MCP server name",
                max_bytes: MAX_MCP_NAME_BYTES,
            },
        ),
        (
            "server",
            too_long.as_str(),
            ExecutionError::ValueTooLong {
                field: "MCP tool name",
                max_bytes: MAX_MCP_NAME_BYTES,
            },
        ),
        (
            "two words",
            "tool",
            ExecutionError::InvalidMcpToolName("MCP server name"),
        ),
        (
            "server",
            "a\nb",
            ExecutionError::InvalidMcpToolName("MCP tool name"),
        ),
        (
            "server",
            "a\u{7}",
            ExecutionError::InvalidMcpToolName("MCP tool name"),
        ),
    ] {
        assert_eq!(McpTool::new(server, tool), Err(expected));
    }
}

#[test]
fn an_mcp_identity_is_observed_once_carried_through_sparse_updates_and_counted() {
    let execution = ExecutionId::new("execution").unwrap();
    let id = ToolCallId::new("tool").unwrap();
    let mcp = McpTool::new("charts", "show").unwrap();
    let named = update("tool").with_mcp_tool(mcp.clone());
    assert_eq!(named.mcp_tool(), Some(&mcp));
    assert_eq!(update("tool").mcp_tool(), None);
    // The names, and the box that holds them apart from the observation.
    let retained = std::mem::size_of::<McpTool>() + mcp.payload_bytes();
    assert_eq!(named.payload_bytes(), retained);
    let mut session = session(&execution);
    session.observe_tool(&execution, named.clone()).unwrap();
    // A later update that names nothing keeps what was observed.
    let status = ToolCallUpdate::new(
        id.clone(),
        None,
        None,
        Some(ToolStatus::Completed),
        None,
        None,
    );
    let tool = session.tool(&id).unwrap();
    assert_eq!(tool.payload_bytes_after(&status), tool.payload_bytes());
    session.observe_tool(&execution, status.clone()).unwrap();
    let tool = session.tool(&id).unwrap();
    assert_eq!(tool.observation().mcp_tool(), Some(&mcp));
    assert_eq!(
        tool.payload_bytes(),
        execution.as_str().len() + id.as_str().len() + retained
    );
    // Arguments are counted with the identity: on the update, on the tool,
    // and after a later update that replaces them or leaves them.
    let json = r#"{"city":"Oslo"}"#;
    let arguments = McpCallArguments::new(json).unwrap();
    assert_eq!(
        format!("{arguments:?}"),
        format!("McpCallArguments({json:?})")
    );
    let with_arguments = named.clone().with_mcp_arguments(arguments);
    let argument_bytes = std::mem::size_of::<McpCallArguments>() + json.len();
    assert_eq!(with_arguments.payload_bytes(), retained + argument_bytes);
    session.observe_tool(&execution, with_arguments).unwrap();
    let tool = session.tool(&id).unwrap();
    let held = execution.as_str().len() + id.as_str().len() + retained + argument_bytes;
    assert_eq!(tool.payload_bytes(), held);
    assert_eq!(tool.payload_bytes_after(&status), held);
    // Naming the tool again keeps the arguments just counted. Both the
    // identity branch and the argument branch of the prediction have to run:
    // each is a retained allocation.
    assert_eq!(tool.payload_bytes_after(&named), held);
    let replaced = "{}";
    let replacement = update("tool").with_mcp_arguments(McpCallArguments::new(replaced).unwrap());
    assert_eq!(
        tool.payload_bytes_after(&replacement),
        execution.as_str().len()
            + id.as_str().len()
            + retained
            + std::mem::size_of::<McpCallArguments>()
            + replaced.len()
    );
    // A tool no update named stays without one.
    session.observe_tool(&execution, update("plain")).unwrap();
    let plain = session.tool(&ToolCallId::new("plain").unwrap()).unwrap();
    assert_eq!(plain.observation().mcp_tool(), None);
}

#[test]
fn a_structured_result_is_bounded_exact_json_text_beside_the_result_text() {
    let json = r#"{"rows":[1,2],"label":"é"}"#;
    let structured = ToolContent::structured(json).unwrap();
    assert_eq!(structured.view(), ToolContentView::Structured(json));
    assert_eq!(structured.payload_bytes(), json.len());
    assert_ne!(structured, ToolContent::text(json));
    // A JSON string exactly at the bound, and one byte past it.
    let exact = format!("\"{}\"", "a".repeat(MAX_STRUCTURED_RESULT_BYTES - 2));
    assert!(ToolContent::structured(exact.clone()).is_ok());
    assert_eq!(
        ToolContent::structured(format!("{exact} ")),
        Err(ExecutionError::ValueTooLong {
            field: "structured tool result",
            max_bytes: MAX_STRUCTURED_RESULT_BYTES,
        })
    );
    let replaced =
        update("tool").with_content(vec![ToolContent::text("rows: 2"), structured.clone()]);
    assert_eq!(
        replaced.content(),
        &Some(vec![ToolContent::text("rows: 2"), structured])
    );
}

#[test]
fn a_live_tool_call_keeps_its_first_mcp_identity_and_refuses_another() {
    let execution = ExecutionId::new("execution").unwrap();
    let id = ToolCallId::new("tool").unwrap();
    let first = McpTool::new("charts", "show").unwrap();
    let mut session = session(&execution);
    session
        .observe_tool(&execution, update("tool").with_mcp_tool(first.clone()))
        .unwrap();
    let before = session.tool(&id).unwrap().observation().clone();
    for other in [
        McpTool::new("other", "show").unwrap(),
        McpTool::new("charts", "hide").unwrap(),
    ] {
        let conflicting = ToolCallUpdate::new(
            id.clone(),
            Some("changed".into()),
            None,
            Some(ToolStatus::Completed),
            None,
            None,
        )
        .with_mcp_tool(other);
        assert_eq!(
            session.observe_tool(&execution, conflicting),
            Err(ExecutionError::DifferentMcpTool)
        );
        // Refused whole: nothing of the update was applied.
        assert_eq!(session.tool(&id).unwrap().observation(), &before);
    }
    // The same identity again, or none, is accepted.
    session
        .observe_tool(&execution, update("tool").with_mcp_tool(first.clone()))
        .unwrap();
    session.observe_tool(&execution, update("tool")).unwrap();
    assert_eq!(
        session.tool(&id).unwrap().observation().mcp_tool(),
        Some(&first)
    );
}

#[test]
fn a_structured_result_must_be_one_json_value() {
    for valid in [
        "{}",
        "[]",
        " {\"a\": [1, -2.5e+3, true, false, null, \"\\u00e9\\n\", {}]} ",
        "\"text\"",
        "0",
        "-0.0E-1",
        "null",
        "[[[[]]]]",
        "{\"a\":{\"b\":[{}]}}",
        "[ ]",
        "{ }",
        "{ \"a\" : 1 , \"b\" : [ 2 ] }",
        "\"\\\" \\\\ \\/ \\b \\f \\r \\t \\uABcd\"",
        "1E+2",
        "10.25",
    ] {
        assert!(ToolContent::structured(valid).is_ok(), "{valid}");
    }
    for invalid in [
        "",
        " ",
        "{",
        "}",
        "[1,]",
        "[,1]",
        "{\"a\":}",
        "{\"a\" 1}",
        "{\"a\":1,}",
        "{,}",
        "{1:2}",
        "[1 2]",
        "{} {}",
        "01",
        "1.",
        ".1",
        "1e",
        "-",
        "+1",
        "tru",
        "nul",
        "\"open",
        "\"bad \\x escape\"",
        "\"\\u12\"",
        "\"tab\tinside\"",
        "NaN",
        "[1]]",
        "{\"a\":1}}",
        "not json",
        "\"\\u00\"",
        "\"\\u",
        "\"\\uZZZZ\"",
        "\"\\",
        "-a",
        "1e+",
        "1.e1",
        "[1}",
        "{\"a\":1]",
        "{\"a\"",
        "{\"a\":1 \"b\":2}",
        "{\"a\":[}",
        "[{]",
        "{\"a\":1,\"b\"}",
    ] {
        assert_eq!(
            ToolContent::structured(invalid),
            Err(ExecutionError::InvalidStructuredResult),
            "{invalid:?}"
        );
    }
    // Deep nesting is checked without recursion.
    let deep = format!("{}{}", "[".repeat(30_000), "]".repeat(30_000));
    assert!(ToolContent::structured(deep).is_ok());
    let unbalanced = format!("{}{}", "[".repeat(30_000), "]".repeat(29_999));
    assert_eq!(
        ToolContent::structured(unbalanced),
        Err(ExecutionError::InvalidStructuredResult)
    );
}
