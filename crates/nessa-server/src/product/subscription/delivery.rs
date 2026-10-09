//! Each subscription's one frame on its way to the connection's single writer.
//!
//! A subscription offers one frame at a time and waits for the writer to
//! write it (`offer`, then the returned receiver). The writer takes the
//! oldest offer (`take`), so one busy subscription cannot starve another, and
//! says when it is written (`SubscriptionFrame::written`). An offer the writer
//! has not taken can be withdrawn (`withdraw`, row S10) or retired with its
//! subscription (`retire`, row S16): a frame the writer took first is written
//! first; none is taken after.
//!
//! Only a terminal frame carries a deadline the writer keeps: missing it
//! closes the socket, as a watch's terminal notice does. A view frame's
//! deadline is its subscription's to keep, by withdrawing it.

use super::super::event_sequence::EventSequence;
use nessa_protocol::product::generated::{
    MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS, MAX_CONNECTION_LIST_SUBSCRIPTIONS,
};
use nessa_protocol::protocol::{EventFrame, OutgoingMessage};
use serde_json::Value;
use std::sync::{Arc, Mutex, PoisonError};
use tokio::{
    sync::{oneshot, Notify, OwnedSemaphorePermit},
    time::Instant,
};

/// What a subscription sends: its reply, or one of its events.
pub(in crate::product) enum Outgoing {
    Reply(OutgoingMessage),
    Event { name: &'static str, payload: Value },
}

struct Offered {
    outgoing: Outgoing,
    order: u64,
    terminal: Option<Instant>,
    written: oneshot::Sender<()>,
    // The subscribe request's socket slot, held until its reply is written.
    slot: Option<Arc<OwnedSemaphorePermit>>,
}

struct Slot {
    id: String,
    offered: Option<Offered>,
    // The terminal deadline of the frame the writer is writing, if it is one.
    in_flight: Option<Option<Instant>>,
}

struct State {
    slots: Vec<Slot>,
    order: u64,
    closed: bool,
}

pub(in crate::product) struct SubscriptionDeliveries {
    state: Mutex<State>,
    ready: Notify,
    sequence: Arc<EventSequence>,
}

/// One frame the writer took. Its request slot goes with it.
pub(in crate::product) struct SubscriptionFrame {
    pub id: String,
    pub message: OutgoingMessage,
    pub terminal: Option<Instant>,
    pub written: Written,
}

/// Told once the frame is written; dropped unwritten, its subscription learns
/// that it never will be.
pub(in crate::product) struct Written {
    sender: oneshot::Sender<()>,
    _slot: Option<Arc<OwnedSemaphorePermit>>,
}

impl Written {
    /// The writer wrote it: its subscription may read again.
    pub fn told(self) {
        let _ = self.sender.send(());
    }
}

impl SubscriptionDeliveries {
    /// Events continue `sequence`, the socket's one event numbering.
    pub fn new(sequence: Arc<EventSequence>) -> Self {
        Self {
            state: Mutex::new(State {
                slots: Vec::with_capacity(
                    MAX_CONNECTION_CONVERSATION_SUBSCRIPTIONS + MAX_CONNECTION_LIST_SUBSCRIPTIONS,
                ),
                order: 0,
                closed: false,
            }),
            ready: Notify::new(),
            sequence,
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// A place for subscription `id`. False once the connection closed.
    pub fn reserve(&self, id: &str) -> bool {
        let mut state = self.lock();
        if state.closed {
            return false;
        }
        state.slots.push(Slot {
            id: id.to_owned(),
            offered: None,
            in_flight: None,
        });
        true
    }

    /// Offer `id`'s next frame. The receiver resolves once the writer wrote
    /// it, and fails if it never will be: withdrawn, retired, or the socket
    /// gone. A subscription offers one frame at a time.
    pub fn offer(
        &self,
        id: &str,
        outgoing: Outgoing,
        terminal: Option<Instant>,
        slot: Option<Arc<OwnedSemaphorePermit>>,
    ) -> oneshot::Receiver<()> {
        let (written, receiver) = oneshot::channel();
        let mut state = self.lock();
        let order = state.order.wrapping_add(1);
        state.order = order;
        if let Some(place) = state.slots.iter_mut().find(|place| place.id == id) {
            place.offered = Some(Offered {
                outgoing,
                order,
                terminal,
                written,
                slot,
            });
        }
        drop(state);
        self.ready.notify_one();
        receiver
    }

    /// Take back `id`'s offer if the writer has not taken it. True when it
    /// was taken back: it will never be written.
    pub fn withdraw(&self, id: &str) -> bool {
        let mut state = self.lock();
        state
            .slots
            .iter_mut()
            .find(|place| place.id == id)
            .and_then(|place| place.offered.take())
            .is_some()
    }

    /// The oldest offer, for the writer to write now.
    pub fn take(&self) -> Option<SubscriptionFrame> {
        let mut state = self.lock();
        let position = state
            .slots
            .iter()
            .enumerate()
            .filter(|(_, place)| place.in_flight.is_none())
            .filter_map(|(position, place)| Some((position, place.offered.as_ref()?.order)))
            .min_by_key(|(_, order)| *order)
            .map(|(position, _)| position)?;
        let message = match &state.slots[position]
            .offered
            .as_ref()
            .expect("selected offer")
            .outgoing
        {
            Outgoing::Reply(_) => None,
            Outgoing::Event { name, payload } => Some(
                EventFrame::push(name, payload, self.sequence.next()?, 0)
                    .expect("a JSON value serializes"),
            ),
        };
        let place = &mut state.slots[position];
        let offered = place.offered.take().expect("selected offer");
        place.in_flight = Some(offered.terminal);
        let message = match (message, offered.outgoing) {
            (Some(event), _) => OutgoingMessage::Event(event),
            (None, Outgoing::Reply(reply)) => reply,
            (None, Outgoing::Event { .. }) => unreachable!("an event was framed above"),
        };
        Some(SubscriptionFrame {
            id: place.id.clone(),
            message,
            terminal: offered.terminal,
            written: Written {
                sender: offered.written,
                _slot: offered.slot,
            },
        })
    }

    /// The writer finished `id`'s frame. A terminal one ends its place.
    pub fn sent(&self, id: &str, terminal: bool) {
        let mut state = self.lock();
        if terminal {
            state.slots.retain(|place| place.id != id);
        } else if let Some(place) = state.slots.iter_mut().find(|place| place.id == id) {
            place.in_flight = None;
        }
        drop(state);
        self.ready.notify_one();
    }

    /// End `id` here: its offer, if not taken, is never written.
    pub fn retire(&self, id: &str) {
        self.lock().slots.retain(|place| place.id != id);
        self.ready.notify_one();
    }

    /// The earliest terminal frame's deadline, offered or being written.
    pub fn deadline(&self) -> Option<Instant> {
        self.lock()
            .slots
            .iter()
            .flat_map(|place| {
                [
                    place.offered.as_ref().and_then(|offered| offered.terminal),
                    place.in_flight.flatten(),
                ]
            })
            .flatten()
            .min()
    }

    /// Whether `id` still has a place: not yet ended, or its last frame not
    /// yet written.
    pub fn has(&self, id: &str) -> bool {
        self.lock().slots.iter().any(|place| place.id == id)
    }

    pub fn is_closed(&self) -> bool {
        self.lock().closed
    }

    /// The single writer consumes stored permits before checking `take`.
    pub async fn changed(&self) {
        self.ready.notified().await;
    }

    /// The connection is gone: nothing more is offered or written.
    pub fn close(&self) {
        let mut state = self.lock();
        state.closed = true;
        state.slots.clear();
        drop(state);
        self.ready.notify_one();
    }

    #[cfg(test)]
    pub fn places(&self) -> usize {
        self.lock().slots.len()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_protocol::protocol::ResponseFrame;

    fn deliveries() -> SubscriptionDeliveries {
        SubscriptionDeliveries::new(Arc::new(EventSequence::default()))
    }

    fn event(n: u64) -> Outgoing {
        Outgoing::Event {
            name: "conversation.view",
            payload: Value::from(n),
        }
    }

    #[test]
    fn the_oldest_offer_is_taken_first_and_one_at_a_time_per_subscription() {
        let deliveries = deliveries();
        assert!(deliveries.reserve("1"));
        assert!(deliveries.reserve("2"));
        let _first = deliveries.offer("2", event(1), None, None);
        let _second = deliveries.offer("1", event(2), None, None);
        let taken = deliveries.take().unwrap();
        assert_eq!(taken.id, "2");
        let OutgoingMessage::Event(frame) = &taken.message else {
            panic!("an event")
        };
        assert_eq!(frame.payload, Value::from(1));
        let next = deliveries.take().unwrap();
        assert_eq!(next.id, "1");
        let _third = deliveries.offer("2", event(3), None, None);
        assert!(
            deliveries.take().is_none(),
            "2 has a frame being written already"
        );
        deliveries.sent("2", false);
        assert_eq!(deliveries.take().unwrap().id, "2");
    }

    #[tokio::test]
    async fn a_withdrawn_offer_is_never_written_and_a_taken_one_cannot_be_withdrawn() {
        let deliveries = deliveries();
        assert!(deliveries.reserve("1"));
        let withdrawn = deliveries.offer("1", event(1), None, None);
        assert!(deliveries.withdraw("1"));
        assert!(withdrawn.await.is_err(), "never written");
        assert!(deliveries.take().is_none());
        let written = deliveries.offer("1", event(2), None, None);
        let frame = deliveries.take().unwrap();
        assert!(!deliveries.withdraw("1"));
        frame.written.told();
        assert!(written.await.is_ok());
    }

    #[test]
    fn a_retired_subscription_offers_nothing_more() {
        let deliveries = deliveries();
        assert!(deliveries.reserve("1"));
        let _offer = deliveries.offer("1", event(1), None, None);
        deliveries.retire("1");
        assert!(deliveries.take().is_none());
        assert_eq!(deliveries.places(), 0);
    }

    #[tokio::test(start_paused = true)]
    async fn only_a_terminal_frame_carries_a_deadline_through_its_write() {
        let deliveries = deliveries();
        assert!(deliveries.reserve("1"));
        let _view = deliveries.offer("1", event(1), None, None);
        assert_eq!(deliveries.deadline(), None);
        let frame = deliveries.take().unwrap();
        deliveries.sent(&frame.id, false);
        let deadline = Instant::now() + std::time::Duration::from_secs(1);
        let reply = OutgoingMessage::Response(ResponseFrame::failure("r", "x", "x"));
        let _end = deliveries.offer("1", Outgoing::Reply(reply), Some(deadline), None);
        assert_eq!(deliveries.deadline(), Some(deadline));
        let terminal = deliveries.take().unwrap();
        assert_eq!(terminal.terminal, Some(deadline));
        assert_eq!(deliveries.deadline(), Some(deadline), "while it is written");
        deliveries.sent(&terminal.id, true);
        assert_eq!(deliveries.deadline(), None);
        assert_eq!(deliveries.places(), 0);
    }

    #[test]
    fn events_continue_the_shared_socket_sequence() {
        let sequence = Arc::new(EventSequence::default());
        let deliveries = SubscriptionDeliveries::new(sequence.clone());
        assert!(deliveries.reserve("1"));
        let before = sequence.next().unwrap();
        let _offer = deliveries.offer("1", event(1), None, None);
        let OutgoingMessage::Event(frame) = deliveries.take().unwrap().message else {
            panic!("an event")
        };
        assert_eq!(frame.seq, before + 1);
    }
}
