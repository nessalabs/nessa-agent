//! Native-hook suppression as Claude and OpenCode report it.
//!
//! ```text
//! binding launch -> fixture accepts only after that profile's request
//!                -> open, prompt, restore, prompt
//!                -> negotiated fact stays unknown
//! ```
//!
//! Arrows show the order these tests run. The fixture refuses the launch when
//! the request is missing. These tests do not read that request back. They
//! check the fact the profile reports, and that an unknown fact does not
//! refuse the first open or the restored one.
//!
//! Codex's open, prompt, and restoration contract is
//! `codex_startup_and_restoration_continue_when_native_hook_suppression_is_unsupported`.
//! That contract is unsupported, not unknown, so it does not cover these two.
//!
//! Which events the upstream harness fires, and which outputs it accepts, are
//! properties of that process. These fixtures are not that process. They do
//! not emit hook events, and a missing event here is not evidence the real
//! harness stayed quiet.
use super::support::*;
use crate::domain::agent_execution::sessions::ExecutionSessionId;

async fn prompt_restores_and_prompts_again(binding: &impl AgentProvider) -> ExecutionSessionId {
    let opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .expect("an unknown suppression fact must not refuse the first open");
    let capabilities = opened.session.operation_capabilities();
    assert!(capabilities.negotiated());
    assert_eq!(
        capabilities.native_hook_suppression(),
        NativeHookSuppressionCapability::Unknown
    );
    assert_eq!(
        opened
            .session
            .execute(prompt("with the hook request"), Arc::new(|| Ok(())))
            .await
            .into_result()
            .expect("a prompt must run while suppression is unknown"),
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
        .open(ProviderOpenRequest::without_startup_control(Some(
            id.clone(),
        )))
        .await
        .expect("an unknown suppression fact must not refuse restoration");
    let restored_capabilities = restored.session.operation_capabilities();
    assert!(restored_capabilities.negotiated());
    assert_eq!(
        restored_capabilities.native_hook_suppression(),
        NativeHookSuppressionCapability::Unknown
    );
    assert_eq!(
        restored
            .session
            .execute(
                prompt("restored with the hook request"),
                Arc::new(|| Ok(()))
            )
            .await
            .into_result()
            .expect("a restored prompt must run while suppression is unknown"),
        ExecutionOutcome::Completed
    );
    restored
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    id
}

#[tokio::test]
async fn claude_startup_and_restoration_continue_when_native_hook_suppression_is_unknown() {
    // The fixture accepts the launch only after `disableAllHooks` is true and
    // setting sources are empty. That request is not proof of the effective
    // setting, and this profile has no checked answer, so the negotiated fact
    // stays unknown. The open still completes.
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("echo", 16);
    prompt_restores_and_prompts_again(&binding).await;
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
    let id = prompt_restores_and_prompts_again(&binding).await;
    assert_eq!(
        std::fs::read_to_string(root.path().join("resumed")).unwrap(),
        id.as_str()
    );
    assert_gone(&root, "pid");
}
