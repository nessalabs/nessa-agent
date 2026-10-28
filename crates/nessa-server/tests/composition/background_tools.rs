use super::*;
#[test]
fn background_identity_schema_and_domain_consume_the_product_contract() {
    let product: Value =
        serde_json::from_str(include_str!("../../../../protocol/product/v1.json")).unwrap();
    let identity = &product["$defs"]["ConversationTool"]["properties"]["backgroundTaskId"];
    for tool in BackgroundTools::new().tools() {
        let offered = &tool["inputSchema"]["properties"]["taskId"];
        for key in ["minLength", "maxLength", "pattern"] {
            assert_eq!(offered[key], identity[key]);
        }
    }
    let limit = identity["maxLength"].as_u64().unwrap() as usize;
    assert!(BackgroundTaskId::new("a".repeat(limit)).is_ok());
    assert!(BackgroundTaskId::new("a".repeat(limit + 1)).is_err());
    for invalid in ["", "a/b", "é", "a\n"] {
        assert!(BackgroundTaskId::new(invalid).is_err());
    }
}
