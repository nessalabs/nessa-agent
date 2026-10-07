//! A conversation list keeps, on the wire, whether its bound left any out.
//! An observation page keeps the same for one catalogue page, and its cursor
//! is decimal text with no leading zeros.
use super::{list_result, observe_result, ConversationList};
use nessa_protocol::conversation::view::{
    ConversationListEntry, ConversationObservation, ConversationObservationCursor,
};

#[test]
fn a_cut_list_says_so_on_the_wire() {
    let entry = ConversationListEntry {
        conversation_id: "00000000-0000-4000-8000-000000000001".to_owned(),
        title: Some("Flights".to_owned()),
        preview: None,
        created_at_ms: 1,
        updated_at_ms: 2,
        running: false,
        archived: true,
    };
    for complete in [true, false] {
        let wire = serde_json::to_value(list_result(ConversationList {
            conversations: vec![entry.clone()],
            complete,
        }))
        .unwrap();
        assert_eq!(wire["complete"], complete);
        assert_eq!(wire["conversations"][0]["archived"], true);
        assert_eq!(wire["conversations"][0]["updatedAtMs"], 2);
    }
}

#[test]
fn an_observation_page_keeps_complete_and_its_cursor_on_the_wire() {
    let entry = ConversationListEntry {
        conversation_id: "00000000-0000-4000-8000-000000000001".to_owned(),
        title: Some("Flights".to_owned()),
        preview: None,
        created_at_ms: 1,
        updated_at_ms: 2,
        running: false,
        archived: false,
    };
    let unfinished = serde_json::to_value(observe_result(ConversationObservation {
        conversations: vec![entry.clone()],
        complete: false,
        cursor: None,
    }))
    .unwrap();
    assert_eq!(unfinished["complete"], false);
    assert!(unfinished.get("cursor").is_none());
    let page = serde_json::to_value(observe_result(ConversationObservation {
        conversations: vec![entry.clone()],
        complete: false,
        cursor: Some(ConversationObservationCursor {
            incarnation: "catalogue".into(),
            boundary: 10,
            creation: 9,
            id: entry.conversation_id,
        }),
    }))
    .unwrap();
    assert_eq!(page["cursor"]["boundary"], "10");
    assert_eq!(page["cursor"]["creation"], "9");
    assert_eq!(page["cursor"]["id"], "00000000-0000-4000-8000-000000000001");
}
