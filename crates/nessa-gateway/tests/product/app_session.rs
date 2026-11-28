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

/// The app dispatcher routes only conversation methods; which of them an
/// app may call is the `app_call.*` policies' (tested in `nessa-auth`).
#[test]
fn an_app_is_routed_only_conversation_methods() {
    for method in APP_METHODS {
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
        assert!(!APP_METHODS.contains(&method), "{method}");
    }
}

/// A request's asks are named for policy (`app_call.shape`, which refuses
/// each of them, is tested in `nessa-auth`); only decoding and the `auto`
/// default are decided here.
#[test]
fn a_new_conversation_runs_in_auto_and_names_what_else_it_asks_for() {
    let mut asked = frame(
        "conversation.create",
        json!({ "conversationId": "c", "requestId": "r" }),
    );
    assert_eq!(asks(&mut asked), Ok(vec![]));
    assert_eq!(asked.params["approvalMode"], "auto");

    let mut ask = frame(
        "conversation.create",
        json!({ "conversationId": "c", "requestId": "r", "approvalMode": "ask" }),
    );
    assert_eq!(asks(&mut ask), Ok(vec![]));
    assert_eq!(ask.params["approvalMode"], "ask");

    for (params, named) in [
        (
            json!({ "conversationId": "c", "requestId": "r", "approvalMode": "full" }),
            vec![AppCallAsk::FullAccess],
        ),
        (
            json!({ "conversationId": "c", "requestId": "r", "channelId": "owner-channel" }),
            vec![AppCallAsk::Channel],
        ),
        (
            json!({ "conversationId": "c", "requestId": "r", "environment": "box" }),
            vec![AppCallAsk::Environment],
        ),
    ] {
        let mut asked = frame("conversation.create", params.clone());
        assert_eq!(asks(&mut asked), Ok(named), "{params}");
    }
    // Params that do not decode are not the policy's to judge.
    let mut broken = frame("conversation.create", json!({ "approvalMode": 7 }));
    assert_eq!(asks(&mut broken), Err("invalid_request"));
}

#[test]
fn a_message_that_links_files_says_so() {
    let base = json!({
        "conversationId": "c", "requestId": "r", "executionId": "e",
        "text": "hi", "attachments": [], "files": [],
    });
    assert_eq!(
        asks(&mut frame("conversation.send", base.clone())),
        Ok(vec![])
    );
    let mut linked = base.clone();
    linked["files"] = json!([{ "path": "/etc/passwd" }]);
    assert_eq!(
        asks(&mut frame("conversation.send", linked)),
        Ok(vec![AppCallAsk::LinkedFiles])
    );
    let mut uploaded = base;
    uploaded["uploads"] = json!([{ "uploadId": "u" }]);
    assert_eq!(
        asks(&mut frame("conversation.steer", uploaded)),
        Ok(vec![AppCallAsk::LinkedFiles])
    );
}
