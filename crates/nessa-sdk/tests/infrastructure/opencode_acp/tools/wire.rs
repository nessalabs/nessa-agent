//! What this profile adds to the shared reading of a tool call: the identity it
//! keeps, and what a permission request is allowed to be reviewed on.
//!
//! The frames are the protocol's own shapes, which is what Opencode is assumed
//! to report — it has no tool extension of its own, unlike Codex's streamed
//! terminals. Assumed rather than observed: a tool call only happens during a
//! model turn, and none could be run where this was written. The shared
//! translator's own tests cover the reading; these cover the retention, the
//! bound on it, and the refusals.
use super::*;
use serde_json::json;

fn call(id: &str, kind: Option<&str>) -> Value {
    let mut value = json!({
        "sessionUpdate": "tool_call",
        "toolCallId": id,
        "status": "pending",
        // Display text, present on every frame here because it is present on
        // real ones — and never the thing under test, which is the point.
        "title": "Read src/main.rs",
    });
    if let Some(kind) = kind {
        value["kind"] = json!(kind);
    }
    value
}

fn permission(tool: Value) -> Value {
    json!({"sessionId": "ses_1", "toolCall": tool})
}

#[test]
fn a_permission_request_is_named_by_the_call_it_belongs_to() {
    let mut tools = HashMap::new();
    tool_call(&call("read-1", Some("read")), &mut tools).unwrap();

    // The request itself says only which call it is: the name comes from what
    // the announcement gave, which is the whole reason it is retained.
    let input = permission_input(
        &permission(json!({"toolCallId": "read-1", "rawInput": {"filePath": "src/main.rs"}})),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "read");
    assert_eq!(input.arguments_json, r#"{"filePath":"src/main.rs"}"#);
}

/// The identity is the kind, and a title cannot displace it.
///
/// This is the whole reason the kind is what is kept. A title is display text
/// Opencode composes, and it can be derived from arguments the model supplied,
/// so a request to edit something may carry a title that says it is reading
/// something else. `name` is what a person approves under and what the audit
/// record keeps, so it has to be the one thing here the provider cannot choose
/// the meaning of.
#[test]
fn a_request_is_reviewed_under_what_it_does_not_under_what_it_calls_itself() {
    let mut tools = HashMap::new();
    tool_call(&call("edit-1", Some("edit")), &mut tools).unwrap();

    let input = permission_input(
        &permission(json!({
            "toolCallId": "edit-1",
            "kind": "edit",
            "title": "Read the README\nand nothing else",
            "rawInput": {"filePath": "/etc/passwd"},
        })),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "edit");
    // Not the title, and in particular nothing that could be mistaken for one:
    // no newline, no sentence, nothing the request chose the wording of.
    assert!(!input.name.contains('\n'));
    assert!(!input.name.contains("README"));
    // What it is being done to is in the arguments, which is where a reviewer
    // reads it and where it cannot be dressed up as something else.
    assert!(input.arguments_json.contains("/etc/passwd"));
}

/// A kind arriving on a permission request is validated by the same rule as one
/// arriving on a tool call, and by the same code. A request is not a safer place
/// for a value to come from.
#[test]
fn a_kind_the_protocol_does_not_define_is_refused_wherever_it_arrives() {
    let mut tools = HashMap::new();
    assert!(tool_call(&call("x-1", Some("exfiltrate")), &mut tools).is_err());

    // And on the request, even when a good kind was retained for that call: the
    // frame in hand is the one being trusted, so it is the one being checked.
    tool_call(&call("edit-1", Some("edit")), &mut tools).unwrap();
    assert!(permission_input(
        &permission(json!({
            "toolCallId": "edit-1",
            "kind": "exfiltrate",
            "rawInput": {"filePath": "two.rs"},
        })),
        &tools,
    )
    .is_err());
}

#[test]
fn an_update_does_not_forget_the_kind_the_announcement_gave() {
    let mut tools = HashMap::new();
    tool_call(&call("read-1", Some("read")), &mut tools).unwrap();
    // An update carries only what changed, so a missing kind is not a cleared
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
    assert_eq!(input.name, "read");
}

/// A request nothing can name is refused, rather than named by its identifier.
///
/// `tc_01H9` tells the person approving it nothing about what they are
/// approving, so an approval recorded under it is a record that nobody could
/// have known what was approved. Refusing fails the execution, loudly, which is
/// the outcome that can be noticed and fixed.
#[test]
fn a_call_that_was_never_named_is_refused_rather_than_reviewed_under_its_identifier() {
    let mut tools = HashMap::new();
    tool_call(&call("read-1", None), &mut tools).unwrap();

    let refused = permission_input(
        &permission(json!({"toolCallId":"read-1","rawInput":{}})),
        &tools,
    );
    assert!(refused.is_err());
}

#[test]
fn a_request_with_no_arguments_is_refused_rather_than_reviewed_on_its_name_alone() {
    let tools = HashMap::new();
    for tool in [
        json!({"toolCallId": "edit-1", "kind": "edit", "title": "Edit src/main.rs"}),
        // Present but not an object: not the action's arguments either.
        json!({"toolCallId": "edit-1", "kind": "edit", "rawInput": "src/main.rs"}),
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
        tool_call(&call(&format!("read-{index}"), Some("read")), &mut tools).unwrap();
    }
    // Refused rather than evicted: a review that named the wrong tool call
    // would be worse than one that says it cannot name it.
    assert!(tool_call(&call("read-one-too-many", Some("read")), &mut tools).is_err());
    // And the ones already there are still answerable.
    let input = permission_input(
        &permission(json!({"toolCallId":"read-0","rawInput":{}})),
        &tools,
    )
    .unwrap();
    assert_eq!(input.name, "read");
}
