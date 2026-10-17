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
