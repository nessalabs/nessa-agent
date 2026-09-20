//! What this profile adds to the shared reading of a tool call: the identity it
//! keeps, and what a permission request is allowed to be reviewed on.
//!
//! The frames are the protocol's own shapes, which is what Opencode reports —
//! it has no tool extension of its own, unlike Codex's streamed terminals. The
//! shared translator's own tests cover the reading; these cover the retention,
//! the bound on it, and the refusal.
use super::*;
use serde_json::json;

fn call(id: &str, title: Option<&str>) -> Value {
    let mut value = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": id,
        "kind": "read",
        "status": "pending",
    });
    if let Some(title) = title {
        value["title"] = json!(title);
    }
    value
}

fn permission(tool: Value) -> Value {
    json!({"sessionId": "ses_1", "toolCall": tool})
}

#[test]
fn a_permission_request_is_named_by_the_call_it_belongs_to() {
    let mut tools = HashMap::new();
    tool_call(&call("read-1", Some("Read src/main.rs")), &mut tools).unwrap();

    // The request itself says only which call it is: the name comes from what
    // the announcement gave, which is the whole reason it is retained.
    let input = permission_input(
        &permission(json!({"toolCallId": "read-1", "rawInput": {"filePath": "src/main.rs"}})),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "Read src/main.rs");
    assert_eq!(input.arguments_json, r#"{"filePath":"src/main.rs"}"#);
}

#[test]
fn a_request_that_says_what_it_is_asking_about_is_taken_at_its_word() {
    let mut tools = HashMap::new();
    tool_call(&call("edit-1", Some("Edit one")), &mut tools).unwrap();

    // The request's own title wins over the retained one. A tool call that has
    // moved on to a second file would otherwise be reviewed under the first.
    let input = permission_input(
        &permission(json!({
            "toolCallId": "edit-1",
            "title": "Edit two",
            "rawInput": {"filePath": "two.rs"},
        })),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "Edit two");
}

#[test]
fn an_update_does_not_forget_the_name_the_announcement_gave() {
    let mut tools = HashMap::new();
    tool_call(&call("read-1", Some("Read src/main.rs")), &mut tools).unwrap();
    // An update carries only what changed, so a missing title is not a cleared
    // one. Read back through a permission request, because that is the only
    // thing the retention exists for.
    tool_call(
        &json!({"sessionUpdate": "tool_call_update", "toolCallId": "read-1", "status": "completed"}),
        &mut tools,
    )
    .unwrap();

    let input = permission_input(
        &permission(json!({"toolCallId":"read-1","rawInput":{}})),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "Read src/main.rs");
}

#[test]
fn a_call_that_was_never_named_is_reviewed_under_its_identifier() {
    let mut tools = HashMap::new();
    tool_call(&call("read-1", None), &mut tools).unwrap();

    // Its identifier, and nothing invented: a name made up here would put a
    // tool Opencode never ran into the audit record.
    let input = permission_input(
        &permission(json!({"toolCallId":"read-1","rawInput":{}})),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "read-1");
}

#[test]
fn a_request_with_no_arguments_is_refused_rather_than_reviewed_on_its_title() {
    let tools = HashMap::new();
    for tool in [
        json!({"toolCallId": "edit-1", "title": "Edit src/main.rs"}),
        // Present but not an object: not the action's arguments either.
        json!({"toolCallId": "edit-1", "rawInput": "src/main.rs"}),
    ] {
        let refused = permission_input(&permission(tool.clone()), &tools);
        assert!(refused.is_err(), "{tool} was reviewed on nothing");
    }
    // And a request naming no tool call at all is not a request about anything.
    assert!(permission_input(&json!({"sessionId": "ses_1"}), &tools).is_err());
}

#[test]
fn an_execution_cannot_decide_how_many_identities_to_keep() {
    let mut tools = HashMap::new();
    for index in 0..MAX_TOOLS {
        tool_call(&call(&format!("read-{index}"), Some("Read")), &mut tools).unwrap();
    }
    // Refused rather than evicted: a review that named the wrong tool call
    // would be worse than one that says it cannot name it.
    assert!(tool_call(&call("read-one-too-many", Some("Read")), &mut tools).is_err());
    // And the ones already there are still answerable.
    let input = permission_input(
        &permission(json!({"toolCallId":"read-0","rawInput":{}})),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "Read");
}

#[test]
fn a_name_long_enough_to_choose_this_maps_size_is_not_retained() {
    let mut tools = HashMap::new();
    let long = "R".repeat(MAX_NAME_BYTES + 1);
    tool_call(&call("read-1", Some(&long)), &mut tools).unwrap();

    let input = permission_input(
        &permission(json!({"toolCallId":"read-1","rawInput":{}})),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "read-1");
}
