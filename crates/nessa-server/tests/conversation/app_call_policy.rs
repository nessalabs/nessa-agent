//! What an MCP App may ask, answered by Cedar: one test per policy id, the
//! refusal each maps to, and an error refusing.
use super::*;
use crate::conversation::application::admit_app_question;
use crate::mcp_servers::domain::AppRefusal;

fn evaluator() -> CedarAppCallPolicyEvaluator {
    CedarAppCallPolicyEvaluator::new().expect("the checked-in bundle validates")
}

fn allowed(rules: &[&str]) -> ToolDecision {
    ToolDecision::Allow {
        rules: rules.iter().map(ToString::to_string).collect(),
    }
}

fn refused(rules: &[&str]) -> ToolDecision {
    ToolDecision::Refuse {
        rules: rules.iter().map(ToString::to_string).collect(),
    }
}

fn reach(server: &str) -> AppQuestion<'_> {
    AppQuestion::Reach { server }
}

fn call<'a>(server: &'a str, tool: &'a str, visible_to_app: bool) -> AppQuestion<'a> {
    AppQuestion::CallTool {
        server,
        tool,
        visible_to_app,
    }
}

#[test]
fn app_own_server_an_app_reaches_its_own_server() {
    assert_eq!(
        evaluator().evaluate("charts", reach("charts")).unwrap(),
        allowed(&["app.own_server"])
    );
    assert_eq!(
        admit_app_question(&evaluator(), "charts", reach("charts")),
        Ok(vec!["app.own_server".to_owned()])
    );
}

#[test]
fn app_other_server_an_app_never_reaches_another_server() {
    assert_eq!(
        evaluator().evaluate("charts", reach("files")).unwrap(),
        refused(&["app.other_server"])
    );
    assert_eq!(
        admit_app_question(&evaluator(), "charts", reach("files")),
        Err(AppRefusal::ServerMismatch)
    );
}

#[test]
fn app_visible_tool_an_app_calls_its_own_servers_tools_visible_to_it() {
    assert_eq!(
        evaluator()
            .evaluate("charts", call("charts", "rows", true))
            .unwrap(),
        allowed(&["app.visible_tool"])
    );
    // Hidden from apps: no policy lets it be called.
    assert_eq!(
        evaluator()
            .evaluate("charts", call("charts", "think", false))
            .unwrap(),
        refused(&[])
    );
    assert_eq!(
        admit_app_question(&evaluator(), "charts", call("charts", "think", false)),
        Err(AppRefusal::ToolNotForApp)
    );
}

#[test]
fn app_other_server_tool_another_servers_tool_is_refused_whatever_it_is() {
    for visible in [true, false] {
        assert_eq!(
            evaluator()
                .evaluate("charts", call("files", "rows", visible))
                .unwrap(),
            refused(&["app.other_server_tool"]),
            "{visible}"
        );
        assert_eq!(
            admit_app_question(&evaluator(), "charts", call("files", "rows", visible)),
            Err(AppRefusal::ServerMismatch),
            "{visible}"
        );
    }
}

#[test]
fn names_like_syntax_are_still_only_names() {
    for name in [r#"charts" || true || ""#, r"charts\", "*"] {
        assert_eq!(
            evaluator().evaluate("charts", reach(name)).unwrap(),
            refused(&["app.other_server"]),
            "{name}"
        );
        assert_eq!(
            evaluator().evaluate(name, reach(name)).unwrap(),
            allowed(&["app.own_server"]),
            "{name}"
        );
    }
    // A tool named like a server is still that server's tool.
    assert_eq!(
        evaluator()
            .evaluate("charts", call("charts", "charts", true))
            .unwrap(),
        allowed(&["app.visible_tool"])
    );
}

#[test]
fn a_policy_that_errors_makes_the_app_decision_unavailable_and_refused() {
    // Cedar skips a forbid whose condition errors; were that ignored, this
    // bundle would allow the call.
    let bundle = Bundle::from_sources(
        include_str!("../../src/conversation/infrastructure/tool_policy/cedar/schema.json"),
        &[
            r#"@id("all") permit(principal, action, resource);"#,
            r#"@id("overflow") forbid(principal, action, resource) when { 9223372036854775807 + 1 > 0 };"#,
        ],
    )
    .expect("validates: the overflow only shows when evaluated");
    let cedar = CedarAppCallPolicyEvaluator::with_bundle(bundle);
    assert_eq!(
        cedar.evaluate("charts", reach("charts")),
        Err(ToolPolicyUnavailable)
    );
    assert_eq!(
        cedar.evaluate("charts", call("charts", "rows", true)),
        Err(ToolPolicyUnavailable)
    );
    // Refused as the question it could not answer.
    assert_eq!(
        admit_app_question(&cedar, "charts", reach("charts")),
        Err(AppRefusal::ServerMismatch)
    );
    assert_eq!(
        admit_app_question(&cedar, "charts", call("charts", "rows", true)),
        Err(AppRefusal::ToolNotForApp)
    );
}

fn message<'a>(
    server: &'a str,
    approval_mode: &'a str,
    trusted_app: bool,
) -> UnreviewedMessage<'a> {
    UnreviewedMessage {
        server,
        approval_mode,
        trusted_app,
    }
}

#[test]
fn app_message_full_access_a_message_goes_unasked_with_full_access() {
    assert_eq!(
        evaluator()
            .message_unreviewed("huddle", message("huddle", "full", false))
            .unwrap(),
        allowed(&["app.message_full_access"])
    );
    for mode in ["ask", "auto"] {
        assert_eq!(
            evaluator()
                .message_unreviewed("huddle", message("huddle", mode, false))
                .unwrap(),
            refused(&[])
        );
    }
}

#[test]
fn app_message_trusted_app_a_message_goes_unasked_for_an_always_allowed_app() {
    assert_eq!(
        evaluator()
            .message_unreviewed("huddle", message("huddle", "ask", true))
            .unwrap(),
        allowed(&["app.message_trusted_app"])
    );
    // Never for another server's app, whatever was chosen.
    assert!(!crate::conversation::application::message_skips_review(
        &evaluator(),
        "charts",
        message("huddle", "full", true)
    ));
}
