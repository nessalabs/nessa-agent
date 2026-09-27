//! A conversation list keeps, on the wire, whether its bound left any out.
use super::{list_result, ConversationList};
use crate::conversation::application::ConversationListEntry;

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
