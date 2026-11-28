//! What an app's session admits, before any handler is asked.
use super::*;
use nessa_protocol::product::generated::action_for_method;
use serde_json::json;

fn frame(method: &str, params: Value) -> RequestFrame {
    let mut frame: RequestFrame =
        serde_json::from_value(json!({ "type": "req", "id": "1", "method": method, "params": {} }))
            .unwrap();
    frame.params = params;
    frame
}

#[test]
fn an_app_is_offered_only_conversation_methods_and_each_needs_one_capability() {
    for (method, _) in APP_METHODS {
        assert!(method.starts_with("conversation."), "{method}");
        // Each is a real Product method the conversation handler answers.
        assert_eq!(
            action_for_method(method),
            Some("conversation.write"),
            "{method}"
        );
    }
    for method in [
        "apps.installed",
        "apps.issueCredential",
        "credential.list",
        "conversation.delete",
        "conversation.answer",
        "conversation.setApprovalMode",
        "conversation.subscribe",
        "terminal.open",
        "workspace.publish",
    ] {
        assert_eq!(capability_for(method), None, "{method}");
    }
    assert_eq!(
        capability_for("conversation.send"),
        Some(AppCapability::Send)
    );
    assert_eq!(
        capability_for("conversation.list"),
        Some(AppCapability::List)
    );
}

#[test]
fn a_new_conversation_runs_in_auto_and_never_in_full() {
    let mut asked = frame(
        "conversation.create",
        json!({ "conversationId": "c", "requestId": "r" }),
    );
    assert_eq!(admit_shape(&mut asked), Ok(()));
    assert_eq!(asked.params["approvalMode"], "auto");

    let mut ask = frame(
        "conversation.create",
        json!({ "conversationId": "c", "requestId": "r", "approvalMode": "ask" }),
    );
    assert_eq!(admit_shape(&mut ask), Ok(()));
    assert_eq!(ask.params["approvalMode"], "ask");

    for refused in [
        json!({ "conversationId": "c", "requestId": "r", "approvalMode": "full" }),
        json!({ "conversationId": "c", "requestId": "r", "channelId": "owner-channel" }),
        json!({ "conversationId": "c", "requestId": "r", "environment": "box" }),
    ] {
        let mut asked = frame("conversation.create", refused.clone());
        assert_eq!(admit_shape(&mut asked), Err("forbidden"), "{refused}");
    }
}

#[test]
fn a_message_links_no_file_on_this_computer() {
    let base = json!({
        "conversationId": "c", "requestId": "r", "executionId": "e",
        "text": "hi", "attachments": [], "files": [],
    });
    assert_eq!(
        admit_shape(&mut frame("conversation.send", base.clone())),
        Ok(())
    );
    let mut linked = base.clone();
    linked["files"] = json!([{ "path": "/etc/passwd" }]);
    assert_eq!(
        admit_shape(&mut frame("conversation.send", linked)),
        Err("forbidden")
    );
    let mut uploaded = base;
    uploaded["uploads"] = json!([{ "uploadId": "u" }]);
    assert_eq!(
        admit_shape(&mut frame("conversation.steer", uploaded)),
        Err("forbidden")
    );
}
