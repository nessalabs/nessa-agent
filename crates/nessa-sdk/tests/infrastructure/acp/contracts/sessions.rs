use super::support::*;
#[tokio::test]
async fn streams_two_prompts_with_one_immutable_session_and_repeated_close() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("echo", 16);
    let mut opened = binding.open(None).await.unwrap();
    assert_eq!(
        opened.session.capabilities().model().model_id(),
        "exact-fixture-model"
    );
    // Resolve both calls before reading any events: old output must retain its
    // execution identity and terminal boundary even while a new prompt runs.
    for input in ["first", "second"] {
        assert_eq!(
            opened
                .session
                .execute(prompt(input))
                .await
                .into_result()
                .unwrap(),
            ExecutionOutcome::Completed
        );
    }
    for input in ["first", "second"] {
        let event = opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
            .unwrap()
            .unwrap();
        assert_eq!(event.execution_id().as_str(), input);
        assert_eq!(
            event.update().clone(),
            ExecutionUpdate::Message(MessageChunk::text(input))
        );
        let terminal = opened
            .events
            .next()
            .await
            .map_err(|failure| failure.into_error())
            .unwrap()
            .unwrap();
        assert_eq!(terminal.execution_id().as_str(), input);
        assert_eq!(
            terminal.update().clone(),
            ExecutionUpdate::Finished(ExecutionOutcome::Completed)
        );
    }
    let first = opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_eq!(
        opened
            .session
            .shutdown(SessionCloseRequest::Explicit(close_action()))
            .await
            .into_result()
            .unwrap(),
        first
    );
    assert_eq!(
        opened.session.execute(prompt("late")).await.into_result(),
        Ok(ExecutionOutcome::Completed)
    );
    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
