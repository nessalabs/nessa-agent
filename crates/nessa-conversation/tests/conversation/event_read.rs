//! Metadata projection and source-correlation regressions.
use super::*;
use nessa_sdk::application::agent_execution::{agents::AgentError, executions::ExecutionEvent};
use nessa_sdk::domain::agent_execution::{
    executions::ExecutionId,
    tools::{McpTool, ToolCallId, ToolCallUpdate},
};
use nessa_sync::replication::domain::Id;

fn conversation() -> ConversationId {
    ConversationId::new("00000000-0000-4000-8000-000000000001").unwrap()
}
fn cursor(id: &ConversationId, index: u64) -> CommittedEventCursor {
    CommittedEventCursor::new(
        conversation_session(id),
        Id::new("incarnation").unwrap(),
        1,
        Some(index),
    )
}

#[test]
fn projection_omits_sensitive_content_and_provider_finished_is_not_local_end() {
    let id = conversation();
    let execution = ExecutionId::new("turn").unwrap();
    let tool = ToolCallUpdate::new(
        ToolCallId::new("tool").unwrap(),
        Some("shell SECRET ARGUMENT".into()),
        None,
        Some(ToolStatus::Running),
        None,
        None,
    )
    .with_mcp_tool(McpTool::new("nessa", "shell").unwrap());
    let events = vec![
        CommittedEvent::change(
            cursor(&id, 1),
            SessionChange::ProviderObservation(ExecutionEvent::new(
                execution.clone(),
                ExecutionUpdate::Tool(tool),
            )),
        ),
        CommittedEvent::change(
            cursor(&id, 2),
            SessionChange::ProviderObservation(ExecutionEvent::new(
                execution,
                ExecutionUpdate::Finished(ExecutionOutcome::Completed),
            )),
        ),
    ];
    let projected = project_events(&id, &events).unwrap();
    assert_eq!(projected.len(), 1);
    assert_eq!(projected[0].kind, "tool.updated");
    assert_eq!(projected[0].name.as_deref(), Some("shell"));
    let json = serde_json::to_string(&projected).unwrap();
    assert!(!json.contains("SECRET"));
    assert!(!json.contains("arguments"));
}

#[test]
fn settlement_correction_updates_state_without_a_second_turn_end() {
    let id = conversation();
    let execution = ExecutionId::new("turn").unwrap();
    let events = vec![
        CommittedEvent::change(
            cursor(&id, 1),
            SessionChange::LocalSettlement {
                execution_id: execution.clone(),
                before: None,
                after: Ok(ExecutionOutcome::Completed),
                local_outcome: None,
            },
        ),
        CommittedEvent::change(
            cursor(&id, 2),
            SessionChange::LocalSettlement {
                execution_id: execution,
                before: Some(Ok(ExecutionOutcome::Completed)),
                after: Err(AgentError::Closed),
                local_outcome: Some(ExecutionOutcome::Completed),
            },
        ),
    ];
    let projected = project_events(&id, &events).unwrap();
    assert_eq!(projected[0].kind, "turn.ended");
    assert_eq!(projected[1].kind, "state.changed");
    assert_eq!(projected[1].status, Some("settlement_updated"));
}

#[test]
fn foreign_source_reply_and_history_gap_are_explicit_refusals() {
    let id = conversation();
    let foreign = ConversationId::new("00000000-0000-4000-8000-000000000002").unwrap();
    let foreign_event = CommittedEvent::gap(cursor(&foreign, 0));
    assert!(matches!(
        project_events(&id, &[foreign_event]),
        Err(ConversationEventError::Events(
            EventReadError::IdentityChanged
        ))
    ));
    let gap = CommittedEvent::gap(cursor(&id, 0));
    assert!(matches!(
        project_events(&id, &[gap]),
        Err(ConversationEventError::Gap)
    ));
}

mod progress_tests {
    use super::*;
    use nessa_sdk::domain::agent_execution::sessions::SessionId;

    fn cursor(incarnation: &str, position: u64, index: Option<u64>) -> CommittedEventCursor {
        CommittedEventCursor::new(
            SessionId::new("session").unwrap(),
            Id::new(incarnation).unwrap(),
            position,
            index,
        )
    }

    fn page(incarnation: &str, position: u64) -> EventRead {
        use nessa_sdk::application::agent_execution::sessions::{
            CommittedEvent, CommittedEventPage,
        };
        let next = cursor(incarnation, position, Some(0));
        EventRead::Page(
            CommittedEventPage::new(
                vec![CommittedEvent::gap(next.clone())],
                next,
                cursor(incarnation, position, None),
            )
            .unwrap(),
        )
    }

    /// E15: page constructors check one page; the application owns cross-read correlation.
    #[test]
    fn event_progress_refuses_repeated_backwards_and_replaced_pages() {
        let start = EventStart::After(cursor("store", 2, None));
        for invalid in [
            page("store", 1),
            page("store", 2),
            page("replacement", 3),
            EventRead::Idle(cursor("store", 1, None)),
            EventRead::Idle(cursor("replacement", 3, None)),
        ] {
            assert_eq!(
                EventProgress::new(&start).accept(&invalid),
                Err(EventReadError::InvalidCursor)
            );
        }
        let mut progress = EventProgress::new(&start);
        progress.accept(&page("store", 3)).unwrap();
        assert_eq!(
            progress.accept(&page("store", 3)),
            Err(EventReadError::InvalidCursor)
        );
        progress
            .accept(&EventRead::Idle(cursor("store", 3, None)))
            .unwrap();
        progress
            .accept(&EventRead::Idle(cursor("store", 3, None)))
            .unwrap();
        assert_eq!(
            progress.accept(&page("store", 3)),
            Err(EventReadError::InvalidCursor)
        );
        progress.accept(&page("store", 4)).unwrap();
    }

    /// E15: Preparing preserves the start; Now establishes continuity with either output kind.
    #[test]
    fn event_progress_correlates_beginning_now_and_idle_starts() {
        let mut beginning = EventProgress::new(&EventStart::Beginning);
        beginning.accept(&EventRead::Preparing).unwrap();
        beginning.accept(&page("store", 1)).unwrap();
        assert_eq!(
            beginning.accept(&EventRead::Idle(cursor("replacement", 1, None))),
            Err(EventReadError::InvalidCursor)
        );
        let mut now = EventProgress::new(&EventStart::Now);
        now.accept(&EventRead::Preparing).unwrap();
        now.accept(&page("store", 1)).unwrap();
        assert_eq!(
            now.accept(&page("replacement", 2)),
            Err(EventReadError::InvalidCursor)
        );
        now.accept(&EventRead::Idle(cursor("store", 2, None)))
            .unwrap();
        assert_eq!(
            now.accept(&page("store", 1)),
            Err(EventReadError::InvalidCursor)
        );
        now.accept(&page("store", 3)).unwrap();
    }

    /// E15: an explicit start remains authoritative after Preparing and rejection.
    #[test]
    fn event_progress_preserves_after_across_preparing_and_foreign_session_replies() {
        let start = cursor("store", 2, None);
        let mut progress = EventProgress::new(&EventStart::After(start.clone()));
        progress.accept(&EventRead::Preparing).unwrap();
        let foreign = CommittedEventCursor::new(
            SessionId::new("another-session").unwrap(),
            Id::new("store").unwrap(),
            3,
            None,
        );
        assert_eq!(
            progress.accept(&EventRead::Idle(foreign)),
            Err(EventReadError::InvalidCursor)
        );
        assert_eq!(
            progress.accept(&EventRead::Idle(cursor("store", 1, None))),
            Err(EventReadError::InvalidCursor)
        );
        progress.accept(&EventRead::Idle(start)).unwrap();
        progress.accept(&page("store", 3)).unwrap();
    }

    /// E15: an initial Now Idle establishes the same continuity as an initial page.
    #[test]
    fn event_progress_now_idle_establishes_continuity() {
        let mut now = EventProgress::new(&EventStart::Now);
        now.accept(&EventRead::Idle(cursor("store", 2, None)))
            .unwrap();
        assert_eq!(
            now.accept(&page("replacement", 3)),
            Err(EventReadError::InvalidCursor)
        );
        assert_eq!(
            now.accept(&page("store", 1)),
            Err(EventReadError::InvalidCursor)
        );
        now.accept(&page("store", 3)).unwrap();
    }
}
