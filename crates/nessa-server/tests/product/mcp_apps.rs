//! What an MCP App's wire commands take from the caller, and what a server's
//! error becomes on the wire (#348).
use super::{app, remote_message, McpAppReference};

fn reference(execution: &str, tool: &str, instance: &str) -> McpAppReference {
    McpAppReference {
        execution_id: execution.into(),
        tool_id: tool.into(),
        instance_id: instance.into(),
    }
}

const MOUNT: &str = "6f1d6c0e-8f8c-4a52-9b8e-1f6c3d2a4b5c";

#[test]
fn an_app_is_named_by_bounded_identities_and_the_hosts_lowercase_uuid() {
    let named = app(reference("e", "t", MOUNT)).unwrap();
    assert_eq!(named.instance_id, MOUNT);
    let long = "x".repeat(257);
    for refused in [
        reference("", "t", MOUNT),
        reference("e", "", MOUNT),
        reference(&long, "t", MOUNT),
        reference("e", &long, MOUNT),
        // Only the schema's form: lowercase, hyphenated, 36 characters.
        reference("e", "t", &MOUNT.to_uppercase()),
        reference("e", "t", &MOUNT.replace('-', "")),
        reference("e", "t", &format!("{{{MOUNT}}}")),
        reference("e", "t", "not-a-mount"),
        reference("e", "t", ""),
    ] {
        assert!(app(refused).is_err());
    }
    // Exactly at the identity bound is taken.
    assert!(app(reference(&"x".repeat(256), &"y".repeat(256), MOUNT)).is_ok());
}

#[test]
fn a_servers_message_is_bounded_in_characters_with_control_characters_as_spaces() {
    assert_eq!(remote_message("bad\nparams\t!"), "bad params !");
    let long = "é".repeat(600);
    let bounded = remote_message(&long);
    assert_eq!(bounded.chars().count(), 512);
    assert!(bounded.chars().all(|character| character == 'é'));
    assert_eq!(remote_message(&"a".repeat(512)), "a".repeat(512));
}
