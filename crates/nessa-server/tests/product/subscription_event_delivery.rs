//! E5: force untaken and already-writing event offers across their deadline.
use super::*;
use crate::product::event_sequence::EventSequence;
use std::task::Poll;

#[tokio::test(start_paused = true)]
async fn untaken_event_offer_ends_lagging_with_last_written_cursor() {
    let deliveries = Arc::new(SubscriptionDeliveries::new(Arc::new(
        EventSequence::default(),
    )));
    assert!(deliveries.reserve("1"));
    let mut offered = Box::pin(offer_event(
        &deliveries,
        "1",
        json!({}),
        Some("applied-before"),
    ));
    assert!(matches!(futures_util::poll!(&mut offered), Poll::Pending));
    tokio::time::advance(DELIVERY_TIMEOUT).await;
    assert!(!offered.await);
    let frame = deliveries.take().unwrap();
    let OutgoingMessage::Event(frame) = frame.message else {
        panic!("terminal event")
    };
    assert_eq!(frame.payload["reason"], "lagging");
    assert_eq!(frame.payload["lastEventCursor"], "applied-before");
}

#[tokio::test(start_paused = true)]
async fn event_offer_already_taken_keeps_its_physical_write_owner() {
    let deliveries = Arc::new(SubscriptionDeliveries::new(Arc::new(
        EventSequence::default(),
    )));
    assert!(deliveries.reserve("1"));
    let mut offered = Box::pin(offer_event(&deliveries, "1", json!({}), None));
    assert!(matches!(futures_util::poll!(&mut offered), Poll::Pending));
    let frame = deliveries.take().unwrap();
    tokio::time::advance(DELIVERY_TIMEOUT * 2).await;
    assert!(matches!(futures_util::poll!(&mut offered), Poll::Pending));
    frame.written.told();
    assert!(offered.await);
    deliveries.sent("1", false);
    assert!(deliveries.take().is_none());
}

fn cursor(incarnation: &str, position: u64, index: Option<u64>) -> CommittedEventCursor {
    CommittedEventCursor::new(
        nessa_sdk::domain::agent_execution::sessions::SessionId::new("session").unwrap(),
        nessa_sync::replication::domain::Id::new(incarnation).unwrap(),
        position,
        index,
    )
}

fn page(incarnation: &str, position: u64) -> EventRead {
    use nessa_sdk::application::agent_execution::sessions::{CommittedEvent, CommittedEventPage};
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

/// E15: page constructors check one page; the follower owns correlation across reads.
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
