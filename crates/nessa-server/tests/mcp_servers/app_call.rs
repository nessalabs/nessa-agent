//! The rows of an MCP App's calls (#348, "An app's request" table) that are
//! not policy: one test each, over the pure rules. Which server and tools an
//! app may reach are Cedar's (`tests/conversation/app_call_policy.rs`).
use super::{
    admit_listed_call, known_app, listed_tool, AppCallAdmission, AppFacts, AppRefusal,
    MAX_APP_ARGUMENTS_BYTES,
};
use nessa_sdk::domain::agent_execution::tools::McpTool;
use nessa_sdk::domain::mcp_apps::{ListedTool, ToolHints, ToolUi, UiResourceUri, UiVisibility};

fn app(server: &str) -> AppFacts {
    AppFacts {
        server: server.into(),
        has_ui: true,
    }
}

/// A UI of its own, seen by whom `visibility` says.
fn drawn(visibility: UiVisibility) -> ToolUi {
    ToolUi::new(
        Some(UiResourceUri::new("ui://charts/a.html").unwrap()),
        visibility,
    )
}

/// No UI, seen by whom `visibility` says.
fn undrawn(visibility: UiVisibility) -> ToolUi {
    ToolUi::new(None, visibility)
}

/// `name` on `charts`, with what it declared in `_meta.ui`, and hints that
/// say it only reads unless `destructive`.
fn listed(name: &str, ui: ToolUi, destructive: bool) -> ListedTool {
    let hints = if destructive {
        ToolHints::default()
    } else {
        ToolHints::new(Some(true), None)
    };
    ListedTool::new(McpTool::new("charts", name).unwrap(), ui).with_hints(hints)
}

#[test]
fn a_listed_tool_is_sent_and_a_destructive_one_waits_for_approval() {
    let reads = listed("rows", drawn(UiVisibility::new(false, true)), false);
    assert_eq!(admit_listed_call(&reads, 10), Ok(AppCallAdmission::Send));
    // A tool with no `_meta.ui` at all, or no UI of its own (#412): the same.
    let plain = listed("plain", ToolUi::default(), false);
    assert_eq!(admit_listed_call(&plain, 10), Ok(AppCallAdmission::Send));
    let helper = listed("helper", undrawn(UiVisibility::new(false, true)), false);
    assert_eq!(admit_listed_call(&helper, 10), Ok(AppCallAdmission::Send));
    // Destructive, including a tool that says nothing of its effects: asks.
    let deletes = listed("delete", drawn(UiVisibility::BOTH), true);
    assert_eq!(
        admit_listed_call(&deletes, 10),
        Ok(AppCallAdmission::Approve)
    );
}

#[test]
fn a_tool_not_listed_is_refused() {
    assert_eq!(listed_tool(None), Err(AppRefusal::ToolNotForApp));
    let reads = listed("rows", ToolUi::default(), false);
    assert_eq!(listed_tool(Some(&reads)), Ok(&reads));
}

/// The flow tests in `tests/conversation/app_calls.rs` show a hidden tool
/// refused end to end; this one pins how its declared visibility is read.
#[test]
fn a_tools_visibility_is_read_as_the_policy_reads_it() {
    // What `app_calls` hands the policy as `visibleToApp`.
    for (ui, visible) in [
        (ToolUi::default(), true),
        (drawn(UiVisibility::BOTH), true),
        (drawn(UiVisibility::new(false, true)), true),
        (undrawn(UiVisibility::new(false, true)), true),
        (drawn(UiVisibility::new(true, false)), false),
        (undrawn(UiVisibility::new(true, false)), false),
        (undrawn(UiVisibility::new(false, false)), false),
    ] {
        let tool = listed("t", ui.clone(), false);
        assert_eq!(tool.ui().visibility().app(), visible, "{ui:?}");
    }
}

#[test]
fn no_app_or_one_without_a_ui_is_unknown() {
    let no_ui = AppFacts {
        server: "charts".into(),
        has_ui: false,
    };
    for app in [None, Some(&no_ui)] {
        assert_eq!(known_app(app), Err(AppRefusal::AppUnknown));
    }
    let charts = app("charts");
    assert_eq!(known_app(Some(&charts)), Ok(&charts));
}

#[test]
fn arguments_past_their_bound_are_refused_at_it_and_not_before() {
    let reads = listed("rows", ToolUi::default(), false);
    assert_eq!(
        admit_listed_call(&reads, MAX_APP_ARGUMENTS_BYTES),
        Ok(AppCallAdmission::Send)
    );
    assert_eq!(
        admit_listed_call(&reads, MAX_APP_ARGUMENTS_BYTES + 1),
        Err(AppRefusal::RequestTooLarge)
    );
}
