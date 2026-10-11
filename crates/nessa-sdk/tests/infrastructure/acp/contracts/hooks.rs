//! Native-hook suppression as each integrated binding actually reports it.
//!
//! ```text
//! binding launch -> fixture checks the suppression request and writes hook-request
//!                -> open, prompt, restore, prompt
//!                -> negotiated fact stays the profile's checked answer
//! ```
//!
//! Arrows show the order these tests run. The fixture accepts the launch only
//! after it has seen that binding's request. A prompt then completes, the
//! session is restored, and a prompt completes again.
//!
//! Which events the upstream harness fires, and which outputs it accepts, are
//! properties of that process. These fixtures are not that process. They do
//! not emit hook events, and a missing event here is not evidence the real
//! harness stayed quiet. This module does not invent frames for those claims.
//! The survey names the rows that need a live provider.
//!
//! Codex's open, prompt, and restoration contract is
//! `codex_startup_and_restoration_continue_when_native_hook_suppression_is_unsupported`.
//! This module reads the same `hook-request` file for Claude and OpenCode.
use super::support::*;

fn hook_request(root: &TempDir) -> serde_json::Value {
    serde_json::from_str(
        &std::fs::read_to_string(root.path().join("hook-request"))
            .expect("fixture records the suppression request it accepted"),
    )
    .expect("hook-request is JSON")
}

async fn prompt_restores_and_prompts_again(
    binding: &impl AgentProvider,
    expected: NativeHookSuppressionCapability,
) {
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .expect("the reported suppression fact must not refuse the first open");
    let capabilities = opened.session.operation_capabilities();
    assert!(capabilities.negotiated());
    assert_eq!(capabilities.native_hook_suppression(), expected);
    assert_eq!(
        opened
            .session
            .execute(prompt("with the hook request"))
            .await
            .into_result()
            .expect("a prompt must run with the reported suppression fact"),
        ExecutionOutcome::Completed
    );
    let id = opened.session.id().clone();
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();

    let restored = binding
        .open(ProviderOpenRequest::without_startup_control(Some(id)))
        .await
        .expect("the reported suppression fact must not refuse restoration");
    let restored_capabilities = restored.session.operation_capabilities();
    assert!(restored_capabilities.negotiated());
    assert_eq!(restored_capabilities.native_hook_suppression(), expected);
    assert_eq!(
        restored
            .session
            .execute(prompt("restored with the hook request"))
            .await
            .into_result()
            .expect("a restored prompt must run with the reported suppression fact"),
        ExecutionOutcome::Completed
    );
    restored
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
}

#[tokio::test]
async fn claude_startup_and_restoration_continue_when_native_hook_suppression_is_unknown() {
    // The fixture accepts the launch only after `disableAllHooks` is true and
    // setting sources are empty. That request is not proof of the effective
    // setting, and this profile has no checked answer, so the negotiated fact
    // stays unknown. The open still completes.
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("echo", 16);
    prompt_restores_and_prompts_again(&binding, NativeHookSuppressionCapability::Unknown).await;
    let request = hook_request(&root);
    assert_eq!(request["disableAllHooks"], true);
    assert_eq!(request["settingSources"].as_array().map(Vec::len), Some(0));
    assert!(root.path().join("resume-observed").is_file());
    assert_gone(&root, "pid");
}

#[tokio::test]
async fn opencode_startup_and_restoration_continue_when_native_hook_suppression_is_unknown() {
    // The fixture accepts the launch only after external plugins are off, project
    // config is disabled, alternate config inputs are removed, and the home is
    // not the caller's. Default plugins inside the binary are outside that
    // request. The profile does not report the request as supported.
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_opencode_binding("echo", 16);
    prompt_restores_and_prompts_again(&binding, NativeHookSuppressionCapability::Unknown).await;
    let request = hook_request(&root);
    assert_eq!(request["pure"], "1");
    assert_eq!(request["disableProjectConfig"], "1");
    let absent: Vec<_> = request["absent"]
        .as_array()
        .expect("absent config inputs are a list")
        .iter()
        .filter_map(serde_json::Value::as_str)
        .collect();
    assert_eq!(
        absent,
        [
            "OPENCODE_CONFIG",
            "OPENCODE_CONFIG_CONTENT",
            "OPENCODE_CONFIG_DIR",
        ]
    );
    assert_eq!(request["privateHome"], true);
    assert!(root.path().join("resumed").is_file());
    assert_gone(&root, "pid");
}
