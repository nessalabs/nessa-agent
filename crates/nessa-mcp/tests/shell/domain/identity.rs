use super::*;

#[test]
fn provider_correlation_rejects_empty_and_oversized_textual_identity() {
    assert!(ToolInvocation::provider("", ToolRequestId::Unsigned(1)).is_err());
    assert!(ToolInvocation::provider("connection", ToolRequestId::Text("".into())).is_err());
    assert!(ToolInvocation::provider(
        "connection",
        ToolRequestId::Text("x".repeat(257).into_boxed_str()),
    )
    .is_err());

    let boundary = ToolInvocation::provider(
        "connection",
        ToolRequestId::Text("x".repeat(256).into_boxed_str()),
    )
    .unwrap();
    assert_eq!(
        boundary.request_id(),
        &ToolRequestId::Text("x".repeat(256).into_boxed_str())
    );
}

#[test]
fn provider_correlation_preserves_signed_and_unsigned_json_rpc_identity() {
    for request_id in [ToolRequestId::Signed(-1), ToolRequestId::Unsigned(u64::MAX)] {
        let invocation = ToolInvocation::provider("connection", request_id.clone()).unwrap();
        assert_eq!(invocation.connection_id(), "connection");
        assert_eq!(invocation.request_id(), &request_id);
        assert_eq!(
            invocation.initiator(),
            ToolInitiator::ConfiguredProviderToolCall
        );
    }
}
