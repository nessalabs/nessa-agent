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
        method: nessa_protocol::product::generated::ProductMethod::WorkspacePublish
            .as_str()
            .into(),
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

fn closed_panel() -> Value {
    json!({ "open": false, "tabs": [] })
}

fn two_panes() -> Value {
    json!({
        "arrangement": "columns",
        "sidebar": false,
        "sessionList": true,
        "showing": "panes",
        "panel": closed_panel(),
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
    let slot = windows.open(org.clone(), person.clone()).0;
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
    let slot = windows.open(org.clone(), person.clone()).0;
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
    let slot = windows.open(org, person).0;
    let mut kinds_disagree = two_panes();
    kinds_disagree["columns"][1]["panes"][0]["item"] =
        json!({ "kind": "draft", "conversationId": ID });
    let mut two_focused = two_panes();
    two_focused["columns"][0]["panes"][0]["focused"] = json!(true);
    let mut widget_missing = two_panes();
    widget_missing["showing"] = json!("widget");
    let mut no_columns = two_panes();
    no_columns["columns"] = json!([]);
    let mut no_panel = two_panes();
    no_panel.as_object_mut().unwrap().remove("panel");
    let mut front_missing = two_panes();
    front_missing["panel"] = json!({
        "open": true,
        "tabs": [{ "plugin": "subagents", "id": "chat" }]
    });
    let mut front_stranger = two_panes();
    front_stranger["panel"] = json!({
        "open": true,
        "tabs": [{ "plugin": "subagents", "id": "chat" }],
        "front": { "plugin": "mcp:huddle", "id": "call-1" }
    });
    let mut repeated = two_panes();
    repeated["panel"] = json!({
        "open": true,
        "tabs": [
            { "plugin": "subagents", "id": "chat" },
            { "plugin": "subagents", "id": "chat" }
        ],
        "front": { "plugin": "subagents", "id": "chat" }
    });
    let mut too_many = two_panes();
    too_many["panel"] = json!({
        "open": true,
        "tabs": (0..17)
            .map(|index| json!({ "plugin": "subagents", "id": index.to_string() }))
            .collect::<Vec<_>>(),
        "front": { "plugin": "subagents", "id": "0" }
    });
    for window in [
        kinds_disagree,
        two_focused,
        widget_missing,
        no_columns,
        no_panel,
        front_missing,
        front_stranger,
        repeated,
        too_many,
    ] {
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
fn a_published_panel_is_the_focused_chats_and_a_bad_one_changes_nothing() {
    let (windows, org, person) = owner();
    let slot = windows.open(org.clone(), person.clone()).0;
    let mut window = two_panes();
    window["panel"] = json!({
        "open": true,
        "tabs": [
            { "plugin": "subagents", "id": "chat" },
            { "plugin": "mcp:huddle", "id": "call-1" }
        ],
        "front": { "plugin": "mcp:huddle", "id": "call-1" }
    });
    publish(
        Some(&slot),
        frame(json!({ "windowId": WINDOW, "revision": 1, "window": window })),
    );
    let mut stranger = two_panes();
    stranger["panel"]["front"] = json!({ "plugin": "subagents", "id": "missing" });
    assert_eq!(
        code(publish(
            Some(&slot),
            frame(json!({ "windowId": WINDOW, "revision": 2, "window": stranger }))
        )),
        "invalid_request"
    );
    let held = &windows.of(&org, &person);
    assert_eq!(held.len(), 1);
    assert_eq!(held[0].revision, 1);
    let panel = held[0].window.panel();
    assert!(panel.open());
    assert_eq!(panel.tabs().len(), 2);
    assert_eq!(panel.front().unwrap().plugin(), "mcp:huddle");
    assert_eq!(panel.front().unwrap().id(), "call-1");
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

fn answer_frame(params: Value) -> RequestFrame {
    RequestFrame {
        kind: "req".into(),
        id: "r2".into(),
        method: "workspace.answer".into(),
        params,
    }
}

#[test]
fn a_window_answers_only_the_commands_sent_to_it_in_their_own_shape() {
    use crate::conversation::application::WindowAnswer;
    use crate::conversation::domain::{WindowAction, WindowSide};
    let (windows, org, person) = owner();
    let (slot, mut commands) = windows.open(org.clone(), person.clone());
    assert!(response(publish(
        Some(&slot),
        frame(json!({
            "windowId": WINDOW, "revision": 1, "window": two_panes()
        }))
    ))
    .error
    .is_none());
    let action = WindowAction::Show {
        side: WindowSide::Panel,
        open: true,
    };
    let mut waiting = windows.command(&org, &person, WINDOW, action).unwrap();
    let sent = commands.try_recv().unwrap();
    // The window is sent the change in the protocol's words.
    let wire = serde_json::to_value(command(&sent)).unwrap();
    assert_eq!(
        wire,
        json!({"windowId": WINDOW, "commandId": sent.id, "action": {"kind": "panel", "open": true}})
    );
    // Each outcome takes exactly its own fields.
    for bad in [
        json!({"windowId": WINDOW, "commandId": sent.id, "outcome": "applied"}),
        json!({"windowId": WINDOW, "commandId": sent.id, "outcome": "applied", "revision": 2, "code": "no_room"}),
        json!({"windowId": WINDOW, "commandId": sent.id, "outcome": "refused", "revision": 2}),
        json!({"windowId": WINDOW, "commandId": sent.id, "outcome": "refused", "code": "made_up"}),
        json!({"windowId": WINDOW, "commandId": sent.id, "outcome": "applied", "revision": 2, "extra": 1}),
        json!({"windowId": WINDOW, "commandId": sent.id, "outcome": "refused", "code": "no_room", "revision": null}),
        json!({"windowId": WINDOW.to_uppercase(), "commandId": sent.id, "outcome": "refused", "code": "no_room"}),
        json!({"windowId": WINDOW, "commandId": 1_u64 << 53, "outcome": "refused", "code": "no_room"}),
    ] {
        assert_eq!(
            code(answer(Some(&slot), answer_frame(bad.clone()))),
            "invalid_request",
            "{bad}"
        );
    }
    assert!(waiting.try_recv().is_err());
    let settled = |message| response(message).payload.unwrap()["settled"].clone();
    // Another command id, or another window, settles nothing.
    assert_eq!(
        settled(answer(
            Some(&slot),
            answer_frame(json!({
                "windowId": WINDOW, "commandId": sent.id + 1, "outcome": "unchanged", "revision": 1
            }))
        )),
        json!(false)
    );
    assert_eq!(
        settled(answer(
            Some(&slot),
            answer_frame(json!({
                "windowId": WINDOW, "commandId": sent.id, "outcome": "refused", "code": "no_chat_focused"
            }))
        )),
        json!(true)
    );
    assert_eq!(
        waiting.try_recv().unwrap(),
        WindowAnswer::Refused {
            code: "no_chat_focused".into()
        }
    );
}

#[test]
fn an_arrangement_names_exactly_the_fields_its_kind_takes() {
    use nessa_protocol::product::generated::WorkspaceArrangeAction;
    let decode = |value: Value| {
        serde_json::from_value::<WorkspaceArrangeAction>(value)
            .ok()
            .and_then(arrange_action)
    };
    for good in [
        json!({"kind": "sidebar", "open": false}),
        json!({"kind": "session_list", "open": true}),
        json!({"kind": "panel", "open": true}),
        json!({"kind": "panel_tab", "widget": {"plugin": "files", "id": "recent"}}),
        json!({"kind": "show", "view": "agents"}),
        json!({"kind": "open_conversation", "conversationId": ID}),
        json!({"kind": "open_conversation", "conversationId": ID, "place": "below"}),
        json!({"kind": "focus_pane", "conversationId": ID}),
    ] {
        assert!(decode(good.clone()).is_some(), "{good}");
    }
    for bad in [
        json!({"kind": "sidebar"}),
        json!({"kind": "sidebar", "open": true, "view": "panes"}),
        json!({"kind": "panel_tab", "widget": {"plugin": "", "id": "recent"}}),
        json!({"kind": "show"}),
        json!({"kind": "open_conversation"}),
        json!({"kind": "open_conversation", "conversationId": "not-an-id"}),
        json!({"kind": "focus_pane", "conversationId": ID, "place": "here"}),
        // Closing a pane is its own method, and severity.
        json!({"kind": "close_pane", "conversationId": ID}),
    ] {
        assert!(decode(bad.clone()).is_none(), "{bad}");
    }
}
