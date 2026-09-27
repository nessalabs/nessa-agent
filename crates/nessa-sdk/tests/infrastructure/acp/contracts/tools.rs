//! Claude tool observations retain supported content while bounding opaque variants.
use super::support::*;
use crate::domain::agent_execution::tools::ToolContent;
use crate::infrastructure::acp::tools::wire::UNSUPPORTED_TOOL_CONTENT;

#[tokio::test]
async fn unsupported_tool_content_is_visible_and_does_not_end_the_session() {
    let _process_slot = process_test_slot().await;
    let (root, binding) = test_acp_binding("unsupported-tool-content", 32);
    let mut opened = binding
        .open(ProviderOpenRequest::without_startup_control(None))
        .await
        .unwrap();

    for input in ["first", "second"] {
        let running = start(&opened, input).await;
        let mut contents = Vec::new();
        let mut continued = false;
        loop {
            match next(&mut opened).await {
                ExecutionUpdate::Tool(update) => {
                    if let Some(content) = update.content() {
                        contents.extend(content.iter().cloned());
                    }
                }
                ExecutionUpdate::Message(chunk) => {
                    assert_eq!(chunk, MessageChunk::text(format!("continued:{input}")));
                    continued = true;
                }
                ExecutionUpdate::Finished(ExecutionOutcome::Completed) => break,
                update => panic!("unexpected update: {update:?}"),
            }
        }
        assert_eq!(running.await.unwrap(), Ok(ExecutionOutcome::Completed));
        assert!(continued);
        assert_eq!(contents.first(), Some(&ToolContent::text("before")));
        assert_eq!(contents.last(), Some(&ToolContent::text("after")));
        assert_eq!(
            contents
                .iter()
                .filter(|content| **content == ToolContent::text(UNSUPPORTED_TOOL_CONTENT))
                .count(),
            15
        );
        assert_eq!(contents.len(), 17);
        assert!(contents.iter().all(|content| {
            content != &ToolContent::text("i".repeat(3000))
                && content != &ToolContent::text("z".repeat(180))
        }));
    }

    opened
        .session
        .shutdown(SessionCloseRequest::Explicit(close_action()))
        .await
        .into_result()
        .unwrap();
    assert_gone(&root, "pid");
}
