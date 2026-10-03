use super::super::{
    generated::{
        product_event, ConversationChanged, ConversationWatchEnded, MAX_CONNECTION_CHANGE_WATCHES,
    },
    passive_read::deadlines::RECORD_SEND_TIMEOUT,
    socket::CHALLENGE_EVENT_SEQUENCE,
};
use super::owner::ProductWatchPermit;
use crate::product_contract::generated::ChangeWatchEndReason;
use crate::protocol::{EventFrame, OutgoingMessage};
use std::sync::{Arc, Mutex, PoisonError};
use tokio::{sync::Notify, time::Instant};

#[derive(Clone, Copy)]
pub(in crate::product) enum Notice {
    Changed,
    Ended(ChangeWatchEndReason),
}

#[derive(Clone, Copy)]
struct Pending {
    notice: Notice,
    deadline: Instant,
    authorized: bool,
}
struct TargetDelivery {
    id: String,
    owner: Arc<ProductWatchPermit>,
    // Set once the registration acknowledgement is physically written; no
    // notice is sent before. This is ordering only: no reply deadline lives
    // here, for a registration or a retirement.
    activated: bool,
    pending: Option<Pending>,
    in_flight: Option<Instant>,
    terminal: bool,
    terminal_sent: bool,
    retiring: bool,
}
struct State {
    targets: Vec<TargetDelivery>,
    sequence: u64,
    closed: bool,
}

/// Fixed per-target positions are shared with the physical writer, never a bus.
pub(in crate::product) struct WatchDeliveries {
    state: Mutex<State>,
    ready: Notify,
}

/// The original watch charge and deadline survive movement into a physical send.
pub(in crate::product) struct WatchFrame {
    pub id: String,
    pub message: OutgoingMessage,
    pub deadline: Instant,
    pub terminal: bool,
    pub(in crate::product) _owner: Arc<ProductWatchPermit>,
}

impl WatchDeliveries {
    pub fn new() -> Self {
        Self {
            state: Mutex::new(State {
                targets: Vec::with_capacity(MAX_CONNECTION_CHANGE_WATCHES),
                // Continue the socket's one event sequence after the challenge.
                sequence: CHALLENGE_EVENT_SEQUENCE,
                closed: false,
            }),
            ready: Notify::new(),
        }
    }

    pub fn reserve(&self, id: String, owner: Arc<ProductWatchPermit>) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if state.closed || state.targets.len() >= MAX_CONNECTION_CHANGE_WATCHES {
            return false;
        }
        state.targets.push(TargetDelivery {
            id,
            owner,
            activated: false,
            pending: None,
            in_flight: None,
            terminal: false,
            terminal_sent: false,
            retiring: false,
        });
        drop(state);
        self.ready.notify_one();
        true
    }

    /// Publish the accepted opaque token only after actual producer installation.
    pub fn installed(&self, pending_id: &str, accepted_id: String) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(target) = state
            .targets
            .iter_mut()
            .find(|target| target.id == pending_id)
        {
            target.id = accepted_id;
        }
        drop(state);
        self.ready.notify_one();
    }

    pub fn activate(&self, id: &str) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(target) = state.targets.iter_mut().find(|target| target.id == id) {
            target.activated = true;
        }
        drop(state);
        self.ready.notify_one();
    }

    /// Returns true only when the connection must start one current admission.
    pub fn notice(&self, id: &str, notice: Notice) -> bool {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let Some(target) = state.targets.iter_mut().find(|target| target.id == id) else {
            return false;
        };
        if target.retiring || target.terminal {
            return false;
        }
        if matches!(notice, Notice::Ended(_)) {
            target.terminal = true;
        }
        let fresh = if let Some(pending) = target.pending {
            if matches!(notice, Notice::Ended(_)) {
                target.pending = Some(Pending { notice, ..pending });
            }
            false
        } else {
            target.pending = Some(Pending {
                notice,
                deadline: Instant::now() + RECORD_SEND_TIMEOUT,
                authorized: false,
            });
            true
        };
        drop(state);
        self.ready.notify_one();
        fresh
    }

    pub fn authorize(&self, id: &str) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(target) = state.targets.iter_mut().find(|target| target.id == id) {
            if let Some(pending) = target.pending {
                target.pending = Some(Pending {
                    authorized: true,
                    ..pending
                });
            }
        }
        drop(state);
        self.ready.notify_one();
    }

    /// Whether this watch is retiring: unwatched, failed to install, or its
    /// terminal notice sent. The one owner of that fact; the connection asks.
    /// A watch no longer held here has retired.
    pub fn retiring(&self, id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .targets
            .iter()
            .find(|target| target.id == id)
            .is_none_or(|target| target.retiring || target.terminal_sent)
    }

    pub fn in_flight(&self, id: &str) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .targets
            .iter()
            .any(|target| target.id == id && target.in_flight.is_some())
    }

    pub fn is_closed(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .closed
    }

    pub fn deadline(&self) -> Option<Instant> {
        let state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state
            .targets
            .iter()
            .flat_map(|target| {
                [
                    target.pending.as_ref().map(|pending| pending.deadline),
                    target.in_flight,
                ]
            })
            .flatten()
            .min()
    }

    /// A single physical writer consumes stored Notify permits before checking state.
    pub async fn changed(&self) {
        self.ready.notified().await;
    }

    pub fn take(&self) -> Option<WatchFrame> {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        let position = state.targets.iter().position(|target| {
            !target.retiring
                && target.activated
                && target.in_flight.is_none()
                && target
                    .pending
                    .as_ref()
                    .is_some_and(|pending| pending.authorized && pending.deadline > Instant::now())
        })?;
        let sequence = state.sequence.checked_add(1)?;
        state.sequence = sequence;
        let target = &mut state.targets[position];
        let pending = target.pending.take().expect("selected pending notice");
        let event = match pending.notice {
            Notice::Changed => EventFrame::push(
                product_event::CONVERSATION_CHANGED,
                &ConversationChanged {
                    watch_id: target.id.clone(),
                },
                sequence,
                0,
            ),
            Notice::Ended(reason) => EventFrame::push(
                product_event::CONVERSATION_WATCH_ENDED,
                &ConversationWatchEnded {
                    watch_id: target.id.clone(),
                    reason,
                },
                sequence,
                0,
            ),
        }
        .expect("fixed generated notice serializes");
        target.in_flight = Some(pending.deadline);
        Some(WatchFrame {
            id: target.id.clone(),
            message: OutgoingMessage::Event(event),
            deadline: pending.deadline,
            terminal: matches!(pending.notice, Notice::Ended(_)),
            _owner: target.owner.clone(),
        })
    }

    pub fn sent(&self, id: &str, terminal: bool) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(target) = state.targets.iter_mut().find(|target| target.id == id) {
            target.in_flight = None;
            target.terminal_sent |= terminal;
        }
        drop(state);
        self.ready.notify_one();
    }

    /// Stop this watch's notices: drop any unsent one and select none again.
    /// The unwatch reply is an ordinary reply with its own deadline; nothing
    /// here bounds the writer for a retired watch (rows U1, U4).
    pub fn retire(&self, id: &str) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(target) = state.targets.iter_mut().find(|target| target.id == id) {
            target.retiring = true;
            target.pending = None;
        }
        drop(state);
        self.ready.notify_one();
    }

    // Called after refusal or physical unwatch/end acknowledgement completion.
    pub fn remove(&self, id: &str) {
        self.state
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .targets
            .retain(|target| target.id != id);
        self.ready.notify_one();
    }

    pub fn close(&self) {
        let mut state = self.state.lock().unwrap_or_else(PoisonError::into_inner);
        state.closed = true;
        state.targets.clear();
        drop(state);
        self.ready.notify_one();
    }
}

#[cfg(test)]
mod tests {
    use super::super::owner::WatchOwners;
    use super::*;

    fn reserve(deliveries: &WatchDeliveries, slots: &Arc<WatchOwners>) {
        let owner = Arc::new(
            slots
                .try_acquire(&super::super::owner::tests::principal("a"))
                .unwrap(),
        );
        assert!(deliveries.reserve("watch".into(), owner));
    }

    #[test]
    fn dirty_cannot_activate_before_ack_and_terminal_keeps_original_pending_deadline() {
        let deliveries = WatchDeliveries::new();
        let slots = Arc::new(WatchOwners::new(1, 1));
        reserve(&deliveries, &slots);
        assert!(deliveries.notice("watch", Notice::Changed));
        deliveries.authorize("watch");
        assert!(deliveries.take().is_none());
        let original = deliveries.deadline();
        assert!(!deliveries.notice("watch", Notice::Ended(ChangeWatchEndReason::Closed)));
        assert_eq!(deliveries.deadline(), original);
        deliveries.activate("watch");
        let frame = deliveries.take().unwrap();
        assert!(frame.terminal);
        assert!(!deliveries.notice("watch", Notice::Changed));
        assert!(deliveries.take().is_none());
        deliveries.remove("watch");
        assert_eq!(slots.available_permits(), 0);
        drop(frame);
        assert_eq!(slots.available_permits(), 1);
    }

    #[tokio::test(start_paused = true)]
    async fn selection_requires_current_authority_and_the_original_deadline() {
        let deliveries = WatchDeliveries::new();
        let slots = Arc::new(WatchOwners::new(1, 1));
        reserve(&deliveries, &slots);
        deliveries.activate("watch");
        assert!(deliveries.notice("watch", Notice::Changed));
        let original = deliveries.deadline().unwrap();
        assert!(deliveries.take().is_none());
        deliveries.authorize("watch");
        let frame = deliveries.take().unwrap();
        assert_eq!(frame.deadline, original);
        deliveries.sent("watch", false);
        drop(frame);
        assert!(deliveries.notice("watch", Notice::Changed));
        let original = deliveries.deadline().unwrap();
        tokio::time::advance(RECORD_SEND_TIMEOUT).await;
        deliveries.authorize("watch");
        assert_eq!(Instant::now(), original);
        assert_eq!(deliveries.deadline(), Some(original));
        assert!(deliveries.take().is_none());
        assert_eq!(slots.available_permits(), 0);
        deliveries.close();
        assert_eq!(slots.available_permits(), 1);
    }

    /// Row B1: notices that coalesce into one pending position later cannot
    /// extend its deadline, so a steady stream of commits cannot keep a slow
    /// receiver's notice alive past the first one's delivery deadline.
    #[tokio::test(start_paused = true)]
    async fn coalesced_notices_keep_the_first_pending_deadline_as_time_passes() {
        let deliveries = WatchDeliveries::new();
        let slots = Arc::new(WatchOwners::new(1, 1));
        reserve(&deliveries, &slots);
        deliveries.activate("watch");
        assert!(deliveries.notice("watch", Notice::Changed));
        let original = deliveries.deadline().unwrap();
        tokio::time::advance(RECORD_SEND_TIMEOUT / 2).await;
        assert!(!deliveries.notice("watch", Notice::Changed));
        assert_eq!(deliveries.deadline(), Some(original));
        tokio::time::advance(RECORD_SEND_TIMEOUT / 4).await;
        assert!(!deliveries.notice("watch", Notice::Ended(ChangeWatchEndReason::Closed)));
        assert_eq!(deliveries.deadline(), Some(original));
        deliveries.authorize("watch");
        assert_eq!(deliveries.take().unwrap().deadline, original);
    }

    #[test]
    fn in_flight_and_one_coalesced_pending_retain_the_same_original_charge() {
        let deliveries = WatchDeliveries::new();
        let slots = Arc::new(WatchOwners::new(1, 1));
        reserve(&deliveries, &slots);
        deliveries.activate("watch");
        assert!(deliveries.notice("watch", Notice::Changed));
        deliveries.authorize("watch");
        let first = deliveries.take().unwrap();
        assert!(deliveries.notice("watch", Notice::Changed));
        assert!(!deliveries.notice("watch", Notice::Changed));
        assert!(!deliveries.notice(
            "watch",
            Notice::Ended(ChangeWatchEndReason::NotificationFailed)
        ));
        deliveries.authorize("watch");
        assert!(deliveries.take().is_none());
        assert_eq!(slots.available_permits(), 0);
        deliveries.sent("watch", false);
        assert!(!first.terminal);
        drop(first);
        let second = deliveries.take().unwrap();
        assert!(second.terminal);
        assert!(deliveries.take().is_none());
        deliveries.retire("watch");
        assert_eq!(deliveries.deadline(), Some(second.deadline));
        deliveries.close();
        assert_eq!(slots.available_permits(), 0);
        drop(second);
        assert_eq!(slots.available_permits(), 1);
    }
}
