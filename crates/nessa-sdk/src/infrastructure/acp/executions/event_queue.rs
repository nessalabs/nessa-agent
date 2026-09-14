//! Bounds retained decoded events across every process generation of one attachment.
//! A dequeue transfers ownership to the consumer and releases its queue charge.
use crate::application::agent_execution::executions::ExecutionEvent;
use std::{mem::size_of, sync::Arc};
use tokio::sync::{mpsc, OwnedSemaphorePermit, Semaphore};

const MAX_QUEUED_BYTES: usize = 32 * 1024 * 1024;

pub(in crate::infrastructure::acp) struct EventQueueBudget {
    permits: Arc<Semaphore>,
}
impl EventQueueBudget {
    pub(crate) fn new() -> Self {
        Self {
            permits: Arc::new(Semaphore::new(MAX_QUEUED_BYTES)),
        }
    }
    pub(crate) fn channel(&self, capacity: usize) -> (EventSender, EventReceiver) {
        let (sender, receiver) = mpsc::channel(capacity);
        (
            EventSender {
                sender,
                permits: self.permits.clone(),
            },
            EventReceiver { receiver },
        )
    }
}

pub(in crate::infrastructure::acp) struct EventSender {
    sender: mpsc::Sender<QueuedEvent>,
    permits: Arc<Semaphore>,
}
pub(in crate::infrastructure::acp) struct EventReceiver {
    receiver: mpsc::Receiver<QueuedEvent>,
}
struct QueuedEvent {
    event: ExecutionEvent,
    charge: OwnedSemaphorePermit,
}
#[derive(Debug, PartialEq, Eq)]
pub(in crate::infrastructure::acp) enum QueueError {
    Closed,
    Full,
}
impl EventSender {
    pub(crate) fn try_send(&self, event: ExecutionEvent) -> Result<(), QueueError> {
        if self.sender.is_closed() {
            return Err(QueueError::Closed);
        }
        let bytes = u32::try_from(event_bytes(&event)).map_err(|_| QueueError::Full)?;
        let charge = self
            .permits
            .clone()
            .try_acquire_many_owned(bytes)
            .map_err(|_| QueueError::Full)?;
        self.sender
            .try_send(QueuedEvent { event, charge })
            .map_err(|error| match error {
                mpsc::error::TrySendError::Closed(_) => QueueError::Closed,
                mpsc::error::TrySendError::Full(_) => QueueError::Full,
            })
    }
    pub(crate) fn is_closed(&self) -> bool {
        self.sender.is_closed()
    }
    pub(crate) async fn closed(&self) {
        self.sender.closed().await;
    }
}
impl EventReceiver {
    pub(crate) async fn recv(&mut self) -> Option<ExecutionEvent> {
        let QueuedEvent { event, charge } = self.receiver.recv().await?;
        // The consumer now owns the payload; its retained copies are outside the queue budget.
        drop(charge);
        Some(event)
    }
}

// Charge fixed event storage and retained payload allocations, including capacities
// exposed by the owned value types. Shared immutable cancellation requests are
// conservatively charged in full for each queued event. Channel/allocator overhead,
// transient JSON decoding, domain state, and consumer/audit copies are separate.
fn event_bytes(event: &ExecutionEvent) -> usize {
    event
        .retained_bytes()
        .saturating_add(size_of::<OwnedSemaphorePermit>())
}

#[cfg(test)]
#[path = "../../../../tests/infrastructure/acp/executions/event_queue.rs"]
mod tests;
