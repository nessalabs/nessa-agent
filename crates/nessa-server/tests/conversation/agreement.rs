//! Numbers this context enforces and the published protocol schema states
//! again. Each is valid on its own; these check that they describe the same
//! gateway, so changing one without the other fails here.
use crate::conversation::{
    application::MAX_LISTED_CONVERSATIONS,
    domain::{ConversationPreview, ConversationTitle, LATEST_TIME_MS},
};
use serde_json::Value;

fn schema() -> Value {
    serde_json::from_str(include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../protocol/product/v1.json"
    )))
    .expect("the product schema is valid JSON")
}

#[test]
fn the_list_schema_states_the_bounds_the_summary_rules_keep() {
    let schema = schema();
    let summary = &schema["$defs"]["ConversationSummary"]["properties"];
    assert_eq!(
        summary["preview"]["x-utf8MaxBytes"].as_u64(),
        Some(ConversationPreview::MAX_BYTES as u64)
    );
    // A title is bounded in characters; at four bytes each it still fits.
    let title_bytes = summary["title"]["x-utf8MaxBytes"].as_u64().unwrap();
    assert!(ConversationTitle::MAX_CHARS as u64 * 4 <= title_bytes);
    // Every time a row carries is one the domain can hold, and no later.
    for field in ["createdAtMs", "updatedAtMs"] {
        assert_eq!(
            summary[field]["maximum"].as_u64(),
            Some(LATEST_TIME_MS),
            "{field}"
        );
    }
    assert_eq!(
        schema["$defs"]["ConversationListResult"]["properties"]["conversations"]["maxItems"]
            .as_u64(),
        Some(MAX_LISTED_CONVERSATIONS as u64)
    );
}

#[test]
fn a_read_states_the_same_title_bound_as_the_list() {
    let schema = schema();
    let view = &schema["$defs"]["ConversationView"];
    let summary = &schema["$defs"]["ConversationSummary"]["properties"];
    // One title, shown by both: the same bound, and required on a read so a
    // conversation with no title yet says `null` rather than leaving it out.
    assert_eq!(
        view["properties"]["title"]["x-utf8MaxBytes"],
        summary["title"]["x-utf8MaxBytes"]
    );
    let title_bytes = view["properties"]["title"]["x-utf8MaxBytes"]
        .as_u64()
        .unwrap();
    assert!(ConversationTitle::MAX_CHARS as u64 * 4 <= title_bytes);
    assert!(view["required"]
        .as_array()
        .unwrap()
        .contains(&Value::from("title")));
}
