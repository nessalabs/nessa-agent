//! `workspace.publish` holds what the window says as the connection's
//! window, newest revision first, and refuses anything that is not a window.
use super::*;
use crate::conversation::application::WorkspaceWindows;
use nessa_auth::domain::{OrganizationId, PrincipalId};
use nessa_protocol::protocol::ResponseFrame;
use serde_json::{json, Value};
use std::sync::Arc;

const ID: &str = "0b3b8f0e-6f43-4a51-9d55-0e7e4f7f8a11";
const WINDOW: &str = "7d9f2a4e-1b3c-4d5e-8f60-718293a4b5c6";

fn frame(params: Value) -> RequestFrame {
    RequestFrame {
        kind: "req".into(),
        id: "r1".into(),
        method: METHOD.into(),
        params,
    }
}

fn response(message: OutgoingMessage) -> ResponseFrame {
    match message {
        OutgoingMessage::Response(response) => response,
        OutgoingMessage::Event(_) => panic!("expected a response"),
    }
}

fn code(message: OutgoingMessage) -> String {
    response(message).error.expect("a refusal").code
}

fn two_panes() -> Value {
    json!({
        "arrangement": "columns",
        "sidebar": false,
        "sessionList": true,
        "showing": "panes",
        "columns": [
            { "widthPercent": 50, "panes": [
                { "heightPercent": 100, "focused": false,
                  "item": { "kind": "conversation", "conversationId": ID } } ] },
            { "widthPercent": 50, "panes": [
                { "heightPercent": 100, "focused": true, "item": { "kind": "draft" } } ] }
        ]
    })
}

fn owner() -> (Arc<WorkspaceWindows>, OrganizationId, PrincipalId) {
    (
        WorkspaceWindows::new(),
        OrganizationId::new("org").unwrap(),
        PrincipalId::new("person").unwrap(),
    )
}

#[test]
fn a_published_window_is_held_until_withdrawn() {
    let (windows, org, person) = owner();
    let slot = windows.open(org.clone(), person.clone());
    let answer = response(publish(
        Some(&slot),
        frame(json!({ "windowId": WINDOW, "revision": 1, "window": two_panes() })),
    ));
    assert_eq!(
        answer.payload,
        Some(json!({ "revision": 1, "applied": true }))
    );
    let held = windows.of(&org, &person);
    assert_eq!(held.len(), 1);
    let places = held[0].window.places();
    assert_eq!(places[0].position, "left");
    assert_eq!(
        places[0].pane.content(),
        &PaneContent::Conversation(ConversationId::new(ID).unwrap())
    );
    assert_eq!(places[1].position, "right");

    publish(
        Some(&slot),
        frame(json!({ "windowId": WINDOW, "revision": 2 })),
    );
    assert!(windows.of(&org, &person).is_empty());
}

#[test]
fn an_older_revision_changes_nothing() {
    let (windows, org, person) = owner();
    let slot = windows.open(org.clone(), person.clone());
    publish(
        Some(&slot),
        frame(json!({ "windowId": WINDOW, "revision": 5, "window": two_panes() })),
    );
    let answer = response(publish(
        Some(&slot),
        frame(json!({ "windowId": WINDOW, "revision": 4 })),
    ));
    assert_eq!(
        answer.payload,
        Some(json!({ "revision": 5, "applied": false }))
    );
    assert_eq!(windows.of(&org, &person).len(), 1);
}

#[test]
fn what_is_not_a_window_is_refused() {
    let (windows, org, person) = owner();
    let slot = windows.open(org, person);
    let mut kinds_disagree = two_panes();
    kinds_disagree["columns"][1]["panes"][0]["item"] =
        json!({ "kind": "draft", "conversationId": ID });
    let mut two_focused = two_panes();
    two_focused["columns"][0]["panes"][0]["focused"] = json!(true);
    let mut widget_missing = two_panes();
    widget_missing["showing"] = json!("widget");
    let mut no_columns = two_panes();
    no_columns["columns"] = json!([]);
    for window in [kinds_disagree, two_focused, widget_missing, no_columns] {
        assert_eq!(
            code(publish(
                Some(&slot),
                frame(json!({ "windowId": WINDOW, "revision": 1, "window": window }))
            )),
            "invalid_request"
        );
    }
    for params in [
        json!({ "windowId": WINDOW, "revision": "one" }),
        json!({ "windowId": WINDOW, "revision": 1u64 << 53 }),
        json!({ "windowId": "not a window", "revision": 1 }),
        json!({ "windowId": WINDOW.to_uppercase(), "revision": 1 }),
        json!({ "windowId": WINDOW.replace('-', ""), "revision": 1 }),
        json!({ "revision": 1 }),
    ] {
        assert_eq!(code(publish(Some(&slot), frame(params))), "invalid_request");
    }
}

#[test]
fn a_gateway_without_conversations_holds_no_window() {
    assert_eq!(
        code(publish(
            None,
            frame(json!({ "windowId": WINDOW, "revision": 1 }))
        )),
        "conversations_not_configured"
    );
}
