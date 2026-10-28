//! A conversation list keeps, on the wire, whether its bound left any out.
//! An observation page keeps the same for one catalogue page, and its cursor
//! is decimal text with no leading zeros. A list longer than one frame is
//! cut to the frame and says so.
use super::{fitted_list, list_result, observe_result, success, ConversationList};
use nessa_protocol::conversation::view::{
    ConversationListEntry, ConversationObservation, ConversationObservationCursor,
};
use nessa_protocol::protocol::MAX_PAYLOAD_BYTES;

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
        channel_id: None,
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
        channel_id: None,
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

#[test]
fn a_list_longer_than_a_frame_is_cut_and_says_so() {
    let entry = |n: usize| ConversationListEntry {
        conversation_id: format!("00000000-0000-4000-8000-{n:012}"),
        title: Some("t".repeat(200)),
        preview: Some("p".repeat(400)),
        created_at_ms: 1,
        updated_at_ms: 2,
        running: false,
        archived: false,
        channel_id: None,
    };
    let long = list_result(ConversationList {
        conversations: (0..10_000).map(entry).collect(),
        complete: true,
    });
    let cut = fitted_list(long, "request");
    assert!(!cut.complete, "a cut list says it is incomplete");
    assert!(cut.conversations.len() < 10_000);
    let wire = success("request", &cut).to_wire_text().unwrap();
    assert!(
        wire.len() <= MAX_PAYLOAD_BYTES as usize,
        "{} bytes",
        wire.len()
    );
    // Newest first, kept in order: the rows kept are the list's first ones.
    assert_eq!(
        cut.conversations[0].conversation_id,
        entry(0).conversation_id
    );

    // One more row would not have fit.
    let more = list_result(ConversationList {
        conversations: (0..=cut.conversations.len()).map(entry).collect(),
        complete: false,
    });
    let wire = success("request", &more).to_wire_text().unwrap();
    assert!(wire.len() > MAX_PAYLOAD_BYTES as usize);

    // A list that fits is left whole.
    let short = list_result(ConversationList {
        conversations: (0..3).map(entry).collect(),
        complete: true,
    });
    let kept = fitted_list(short, "request");
    assert!(kept.complete);
    assert_eq!(kept.conversations.len(), 3);
}
