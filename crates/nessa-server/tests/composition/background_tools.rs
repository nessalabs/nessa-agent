use super::*;

#[test]
fn background_matches_both_current_provider_inputs_and_rejects_contradictions() {
    let task = BackgroundTaskId::new("task-1".into()).unwrap();
    let command = BackgroundCommand::new("printf result".into(), 120).unwrap();
    let arguments = json!({"taskId":"task-1","command":"printf result"});
    let envelope = |arguments: Value| json!({"server":BACKGROUND_SERVER,"tool":BACKGROUND_START,"arguments":arguments});
    for valid in [
        arguments.clone(),
        envelope(arguments.clone()),
        envelope(json!({
            "taskId":"task-1","command":"printf result","timeoutSeconds":120
        })),
    ] {
        assert!(
            Arguments.matches(&valid.to_string(), &task, &command),
            "{valid}"
        );
    }
    for invalid in [
        json!({"server":"other","tool":BACKGROUND_START,"arguments":arguments}),
        json!({"server":BACKGROUND_SERVER,"tool":"cancel","arguments":arguments}),
        json!({"server":BACKGROUND_SERVER,"tool":BACKGROUND_START,"arguments":arguments,"extra":true}),
        envelope(json!({"taskId":"other","command":"printf result"})),
        envelope(json!({"taskId":"task-1","command":"other"})),
        envelope(json!({"taskId":"task-1","command":"printf result","timeoutSeconds":1})),
        envelope(json!({"taskId":"task-1","command":"printf result","extra":true})),
        envelope(Value::Null),
        envelope(envelope(arguments)),
    ] {
        assert!(
            !Arguments.matches(&invalid.to_string(), &task, &command),
            "{invalid}"
        );
    }
    for duplicate in [
        r#"{"taskId":"task-1","taskId":"task-1","command":"printf result"}"#,
        r#"{"server":"nessa-background","server":"other","tool":"start","arguments":{"taskId":"task-1","command":"printf result"}}"#,
        r#"{"server":"other","server":"nessa-background","tool":"start","arguments":{"taskId":"task-1","command":"printf result"}}"#,
    ] {
        assert!(
            !Arguments.matches(duplicate, &task, &command),
            "{duplicate}"
        );
    }
}

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
