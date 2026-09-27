use super::support::*;

#[tokio::test]
async fn system_instructions_are_configured_once_and_each_execution_sends_only_new_user_input() {
    let _process_slot = process_test_slot().await;
    use crate::domain::agent_execution::prompts::SystemPromptBuilder;
    let (root, binding) = test_acp_binding("system-prompt", 16);
    assert!(binding.system_prompt().is_none());
    let system_prompt = SystemPromptBuilder::new()
        .text(
            PromptSource::new(PromptSourceKind::Core, "nessa/system").unwrap(),
            "Core instructions.",
        )
        .text(
            PromptSource::new(PromptSourceKind::Plugin, "reviewer").unwrap(),
            "\nPlugin instructions.",
        )
        .build()
        .unwrap();
    let binding = binding.with_system_prompt(system_prompt.clone());
    assert_eq!(binding.system_prompt(), Some(&system_prompt));
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();
    let session_id = opened.session.id().clone();
    for message in ["first user message", "second user message"] {
        assert_eq!(
            opened
                .session
                .execute(prompt(message))
                .await
                .into_result()
                .unwrap(),
            ExecutionOutcome::Completed
        );
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Message(MessageChunk::text(message))
        );
        assert_eq!(
            next(&mut opened).await,
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        );
        assert_eq!(opened.session.id(), &session_id);
    }
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
