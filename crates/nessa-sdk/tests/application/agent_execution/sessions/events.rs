//! Structural evidence accepted from custom semantic-event adapters.
use super::*;
use crate::application::agent_execution::providers::ProviderIdentity;
use crate::domain::agent_execution::sessions::ProviderContext;

fn cursor(
    session: &str,
    incarnation: &str,
    position: u64,
    index: Option<u64>,
) -> CommittedEventCursor {
    CommittedEventCursor::new(
        SessionId::new(session).unwrap(),
        Id::new(incarnation).unwrap(),
        position,
        index,
    )
}
fn gap(session: &str, incarnation: &str, position: u64) -> CommittedEvent {
    CommittedEvent::gap(cursor(session, incarnation, position, Some(0)))
}
fn opened(at: CommittedEventCursor) -> CommittedEvent {
    CommittedEvent::change(
        at.clone(),
        SessionChange::Opened {
            id: at.session().clone(),
            provider: ProviderIdentity::new("fixture", "model", "workspace").unwrap(),
            context: ProviderContext::Absent,
        },
    )
}
fn refused(events: Vec<CommittedEvent>, next: CommittedEventCursor, head: CommittedEventCursor) {
    assert!(matches!(
        CommittedEventPage::new(events, next, head),
        Err(EventReadError::InvalidCursor)
    ));
}

#[test]
fn page_constructor_accepts_ordered_correlated_changes() {
    // One unit may contain a readable opening beside its typed unreadable gap.
    let last = cursor("session", "incarnation", 2, Some(1));
    let page = CommittedEventPage::new(
        vec![
            gap("session", "incarnation", 1),
            gap("session", "incarnation", 2),
            opened(last.clone()),
        ],
        last.clone(),
        cursor("session", "incarnation", 3, None),
    )
    .unwrap();
    assert_eq!(page.events().len(), 3);
    assert_eq!(page.next(), &last);
    assert_eq!(page.captured_head().position(), 3);
    // The enclosing boundary may equal the last location under this page contract.
    let same_position = cursor("session", "incarnation", 2, None);
    assert!(CommittedEventPage::new(vec![opened(last.clone())], last, same_position).is_ok());
}

#[test]
fn page_constructor_rejects_contradictory_adapter_output() {
    let next = cursor("session", "incarnation", 2, Some(0));
    let head = cursor("session", "incarnation", 3, None);
    refused(vec![], next.clone(), head.clone());
    refused(
        (0..257)
            .map(|position| gap("session", "incarnation", position))
            .collect(),
        cursor("session", "incarnation", 256, Some(0)),
        cursor("session", "incarnation", 257, None),
    );
    refused(
        vec![
            gap("other", "incarnation", 1),
            gap("session", "incarnation", 2),
        ],
        next.clone(),
        head.clone(),
    );
    refused(
        vec![gap("session", "old", 1), gap("session", "incarnation", 2)],
        next.clone(),
        head.clone(),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        cursor("other", "incarnation", 2, Some(0)),
        head.clone(),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        cursor("session", "old", 2, Some(0)),
        head.clone(),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        cursor("session", "incarnation", 1, Some(0)),
        head.clone(),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        cursor("session", "incarnation", 3, Some(0)),
        head.clone(),
    );
    refused(
        vec![
            gap("session", "incarnation", 2),
            gap("session", "incarnation", 1),
        ],
        cursor("session", "incarnation", 1, Some(0)),
        head.clone(),
    );
    refused(
        vec![
            gap("session", "incarnation", 2),
            gap("session", "incarnation", 2),
        ],
        next.clone(),
        head.clone(),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        next.clone(),
        cursor("other", "incarnation", 3, None),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        next.clone(),
        cursor("session", "other", 3, None),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        next.clone(),
        cursor("session", "incarnation", 3, Some(0)),
    );
    refused(
        vec![gap("session", "incarnation", 2)],
        next.clone(),
        cursor("session", "incarnation", 1, None),
    );
    let boundary = cursor("session", "incarnation", 2, None);
    refused(
        vec![CommittedEvent::gap(boundary.clone())],
        boundary,
        head.clone(),
    );
    let wrong_gap_index = cursor("session", "incarnation", 2, Some(1));
    refused(
        vec![CommittedEvent::gap(wrong_gap_index.clone())],
        wrong_gap_index,
        head.clone(),
    );
    refused(vec![opened(next.clone())], next, head);
}
