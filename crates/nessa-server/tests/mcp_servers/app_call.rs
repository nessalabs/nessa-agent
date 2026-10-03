//! The policy rows of an MCP App's calls (#348, "An app's request" table):
//! one test each, over the pure rules.
use super::{
    admit_app, admit_tool_call, AppCallAdmission, AppFacts, AppRefusal, MAX_APP_ARGUMENTS_BYTES,
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
fn an_apps_own_tool_for_it_is_sent_and_a_destructive_one_waits_for_approval() {
    let reads = listed("rows", drawn(UiVisibility::new(false, true)), false);
    assert_eq!(
        admit_tool_call(Some(&app("charts")), "charts", Some(&reads), 10),
        Ok(AppCallAdmission::Send)
    );
    // A tool with no `_meta.ui` at all is for an app too: the spec's default.
    let plain = listed("plain", ToolUi::default(), false);
    assert_eq!(
        admit_tool_call(Some(&app("charts")), "charts", Some(&plain), 10),
        Ok(AppCallAdmission::Send)
    );
    // So is one that names apps and has no UI of its own (#412).
    let helper = listed("helper", undrawn(UiVisibility::new(false, true)), false);
    assert_eq!(
        admit_tool_call(Some(&app("charts")), "charts", Some(&helper), 10),
        Ok(AppCallAdmission::Send)
    );
    // Destructive, including a tool that says nothing of its effects: asks.
    let deletes = listed("delete", drawn(UiVisibility::BOTH), true);
    assert_eq!(
        admit_tool_call(Some(&app("charts")), "charts", Some(&deletes), 10),
        Ok(AppCallAdmission::Approve)
    );
}

#[test]
fn a_tool_hidden_from_apps_or_not_listed_is_refused() {
    let model_only = listed("think", drawn(UiVisibility::new(true, false)), false);
    assert_eq!(
        admit_tool_call(Some(&app("charts")), "charts", Some(&model_only), 10),
        Err(AppRefusal::ToolNotForApp)
    );
    // Whether or not it has a UI of its own (#412), and for no one at all.
    for visibility in [
        UiVisibility::new(true, false),
        UiVisibility::new(false, false),
    ] {
        let undrawn = listed("think", undrawn(visibility), false);
        assert_eq!(
            admit_tool_call(Some(&app("charts")), "charts", Some(&undrawn), 10),
            Err(AppRefusal::ToolNotForApp),
            "{visibility:?}"
        );
    }
    assert_eq!(
        admit_tool_call(Some(&app("charts")), "charts", None, 10),
        Err(AppRefusal::ToolNotForApp)
    );
}

#[test]
fn another_servers_tool_is_refused_whatever_it_is() {
    let reads = listed("rows", ToolUi::default(), false);
    assert_eq!(
        admit_tool_call(Some(&app("charts")), "files", Some(&reads), 10),
        Err(AppRefusal::ServerMismatch)
    );
    assert_eq!(
        admit_app(Some(&app("charts")), "files"),
        Err(AppRefusal::ServerMismatch)
    );
}

#[test]
fn no_app_or_one_without_a_ui_is_unknown() {
    let reads = listed("rows", ToolUi::default(), false);
    let no_ui = AppFacts {
        server: "charts".into(),
        has_ui: false,
    };
    for app in [None, Some(&no_ui)] {
        assert_eq!(
            admit_tool_call(app, "charts", Some(&reads), 10),
            Err(AppRefusal::AppUnknown)
        );
        assert_eq!(admit_app(app, "charts"), Err(AppRefusal::AppUnknown));
    }
    assert_eq!(admit_app(Some(&app("charts")), "charts"), Ok(()));
}

#[test]
fn arguments_past_their_bound_are_refused_at_it_and_not_before() {
    let reads = listed("rows", ToolUi::default(), false);
    assert_eq!(
        admit_tool_call(
            Some(&app("charts")),
            "charts",
            Some(&reads),
            MAX_APP_ARGUMENTS_BYTES
        ),
        Ok(AppCallAdmission::Send)
    );
    assert_eq!(
        admit_tool_call(
            Some(&app("charts")),
            "charts",
            Some(&reads),
            MAX_APP_ARGUMENTS_BYTES + 1
        ),
        Err(AppRefusal::RequestTooLarge)
    );
}
