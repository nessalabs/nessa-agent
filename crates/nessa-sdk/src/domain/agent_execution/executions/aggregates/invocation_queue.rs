//! Pending invocation admission and ordering, without provider or runtime effects.
#![deny(missing_docs)]

use std::collections::{HashSet, VecDeque};

use crate::domain::agent_execution::executions::{
    ExecutionId, InvocationKind, QueueMutation, QueueOrderChange, QueueOrderError, SchedulingError,
};

/// Owns bounded pending work and remembers every successfully admitted identity.
///
/// Steering work precedes ordinary work; each kind has its own deque, initially FIFO.
/// Explicit order changes may rearrange inputs within each priority class.
/// Dispatch checks at most two deque fronts, independent of pending capacity.
/// The capacity bounds pending work only. Identity history grows for the queue's
/// lifetime, including after dispatch or drain, so the owning agent must retain
/// this queue for its entire lifetime. Running work belongs to the application.
/// No provider, persistence, audit delivery, or cancellation effects occur here.
/// Callers serialize access through exclusive mutable ownership.
/// The live queue cannot be cloned into a second dispatch authority.
/// ```compile_fail
/// use nessa_sdk::domain::agent_execution::executions::InvocationQueue;
/// let queue = InvocationQueue::new(1).unwrap();
/// let duplicate = queue.clone();
/// ```
#[derive(Debug)]
pub struct InvocationQueue {
    capacity: usize,
    steering: VecDeque<ExecutionId>,
    ordinary: VecDeque<ExecutionId>,
    seen: HashSet<ExecutionId>,
    seen_bytes: usize,
}

/// Moved touched membership state for a semantic replay transaction.
pub(crate) struct InvocationQueueUndo(QueueUndo);
enum QueueUndo {
    Admitted(ExecutionId),
    Removed {
        id: ExecutionId,
        kind: InvocationKind,
        position: usize,
    },
    Order {
        steering: VecDeque<ExecutionId>,
        ordinary: VecDeque<ExecutionId>,
    },
}

impl InvocationQueue {
    /// Creates an empty queue allowing `capacity` pending invocations.
    ///
    /// # Errors
    /// Returns [`SchedulingError::InvalidCapacity`] when `capacity` is zero.
    ///
    /// # Examples
    /// ```
    /// use nessa_sdk::domain::agent_execution::executions::{
    ///     ExecutionId, InvocationKind, InvocationQueue,
    /// };
    /// let mut queue = InvocationQueue::new(2)?;
    /// let id = ExecutionId::new("first")?;
    /// queue.enqueue(id.clone(), InvocationKind::Queued)?;
    /// assert_eq!(queue.pop_next(), Some((id, InvocationKind::Queued)));
    /// # Ok::<(), Box<dyn std::error::Error>>(())
    /// ```
    pub fn new(capacity: usize) -> Result<Self, SchedulingError> {
        if capacity == 0 {
            return Err(SchedulingError::InvalidCapacity);
        }
        Ok(Self {
            capacity,
            steering: VecDeque::new(),
            ordinary: VecDeque::new(),
            seen: HashSet::new(),
            seen_bytes: 0,
        })
    }

    /// Admits `id` with dispatch priority `kind`, taking ownership of its identity.
    ///
    /// # Errors
    /// Returns [`SchedulingError::Duplicate`] if `id` was ever admitted, even if
    /// the queue is also full. Otherwise returns [`SchedulingError::Full`] when
    /// pending capacity is exhausted. Rejections leave queue and history unchanged.
    pub fn enqueue(
        &mut self,
        id: ExecutionId,
        kind: InvocationKind,
    ) -> Result<(), SchedulingError> {
        self.validate_enqueue(&id)?;
        self.seen.insert(id.clone());
        self.seen_bytes = self.seen_bytes.saturating_add(id.as_str().len());
        match kind {
            InvocationKind::Steering => self.steering.push_back(id),
            InvocationKind::Queued => self.ordinary.push_back(id),
        }
        Ok(())
    }

    /// Checks admission without reserving an identity or creating another queue.
    /// Returns Duplicate or Full with the same precedence as enqueue. Callers must
    /// retain exclusive queue ownership between this check and enqueue; enqueue
    /// always checks again before mutation.
    pub fn validate_enqueue(&self, id: &ExecutionId) -> Result<(), SchedulingError> {
        if self.seen.contains(id) {
            return Err(SchedulingError::Duplicate);
        }
        if self.len() == self.capacity {
            return Err(SchedulingError::Full);
        }
        Ok(())
    }

    /// Removes and returns the next identity and kind in dispatch order.
    ///
    /// Returns `None` when empty. Removal frees pending capacity but retains
    /// identity history. This takes constant time; it never scans waiting input.
    /// The caller owns execution and evidence of dispatch.
    #[must_use]
    pub fn pop_next(&mut self) -> Option<(ExecutionId, InvocationKind)> {
        self.steering
            .pop_front()
            .map(|id| (id, InvocationKind::Steering))
            .or_else(|| {
                self.ordinary
                    .pop_front()
                    .map(|id| (id, InvocationKind::Queued))
            })
    }

    /// Removes pending input identified by `id`, returning its admission kind.
    ///
    /// Returns `None` if the identity is not pending, including after dispatch or
    /// a previous removal. Other pending inputs retain their dispatch order. The
    /// identity remains remembered and cannot be admitted again. The caller owns
    /// recording withdrawal evidence; removal has no provider cancellation effect.
    /// Finding a specific identity takes linear time in the pending queue length.
    #[must_use]
    pub fn remove(&mut self, id: &ExecutionId) -> Option<InvocationKind> {
        if let Some(position) = self.steering.iter().position(|pending| pending == id) {
            self.steering.remove(position);
            return Some(InvocationKind::Steering);
        }
        let position = self.ordinary.iter().position(|pending| pending == id)?;
        self.ordinary.remove(position);
        Some(InvocationKind::Queued)
    }

    /// Removes all pending invocations and returns them in dispatch order.
    ///
    /// Identity history is retained. The returned identities let the caller
    /// record cleanup evidence; draining alone does not cancel external work.
    /// Draining takes linear time in the number of pending invocations.
    #[must_use]
    pub fn drain(&mut self) -> Vec<(ExecutionId, InvocationKind)> {
        let mut drained = Vec::with_capacity(self.len());
        while let Some(invocation) = self.pop_next() {
            drained.push(invocation);
        }
        drained
    }

    /// Copy pending identities and priorities in their current dispatch order.
    /// Running and previously removed work is absent.
    pub fn pending(&self) -> Vec<(ExecutionId, InvocationKind)> {
        self.steering
            .iter()
            .map(|id| (id.clone(), InvocationKind::Steering))
            .chain(
                self.ordinary
                    .iter()
                    .map(|id| (id.clone(), InvocationKind::Queued)),
            )
            .collect()
    }
    /// Apply validated replacement order while preserving identities and kinds.
    /// Returns QueueChanged if membership or ordering changed after validation.
    /// The owner must separately retain and persist the change's audit evidence.
    pub fn apply_order(&mut self, change: &QueueOrderChange) -> Result<(), QueueOrderError> {
        self.replace_order(change).map(drop)
    }
    fn replace_order(
        &mut self,
        change: &QueueOrderChange,
    ) -> Result<InvocationQueueUndo, QueueOrderError> {
        if self.pending() != change.before() {
            return Err(QueueOrderError::QueueChanged);
        }
        let steering = std::mem::take(&mut self.steering);
        let ordinary = std::mem::take(&mut self.ordinary);
        for id in change.after() {
            match change
                .before()
                .iter()
                .find(|(known, _)| known == id)
                .expect("validated membership")
                .1
            {
                InvocationKind::Steering => self.steering.push_back(id.clone()),
                InvocationKind::Queued => self.ordinary.push_back(id.clone()),
            }
        }
        Ok(InvocationQueueUndo(QueueUndo::Order { steering, ordinary }))
    }

    /// Replay this local membership fact. Rejects contradictory ordering, missing
    /// membership, repeated admission, priority changes and unnecessary resets.
    /// This aggregate retains admitted identities across restoration resets.
    pub fn apply_mutation(&mut self, mutation: &QueueMutation) -> Result<(), &'static str> {
        self.apply_reversible_mutation(mutation).map(drop)
    }
    pub(crate) fn apply_reversible_mutation(
        &mut self,
        mutation: &QueueMutation,
    ) -> Result<InvocationQueueUndo, &'static str> {
        match mutation {
            QueueMutation::Admitted { id, kind } => {
                self.enqueue(id.clone(), *kind)
                    .map_err(|_| "invalid queue admission")?;
                Ok(InvocationQueueUndo(QueueUndo::Admitted(id.clone())))
            }
            QueueMutation::Selected { id } => {
                if self.pending().first().is_some_and(|(next, _)| next == id) {
                    let (id, kind) = self.pop_next().expect("validated dispatch front");
                    Ok(InvocationQueueUndo(QueueUndo::Removed {
                        id,
                        kind,
                        position: 0,
                    }))
                } else {
                    Err("queue selection disagrees with dispatch order")
                }
            }
            QueueMutation::Removed { id, .. } => {
                let (queue, kind) = if self.steering.iter().any(|known| known == id) {
                    (&mut self.steering, InvocationKind::Steering)
                } else {
                    (&mut self.ordinary, InvocationKind::Queued)
                };
                let position = queue
                    .iter()
                    .position(|known| known == id)
                    .ok_or("removed input was not queued")?;
                let id = queue.remove(position).expect("validated membership");
                Ok(InvocationQueueUndo(QueueUndo::Removed {
                    id,
                    kind,
                    position,
                }))
            }
            QueueMutation::Reordered(change) => self
                .replace_order(change)
                .map_err(|_| "reorder before-state disagrees with queue history"),
            QueueMutation::Restored => {
                if self.is_empty() {
                    Err("empty queue restoration is not a transition")
                } else {
                    Ok(InvocationQueueUndo(QueueUndo::Order {
                        steering: std::mem::take(&mut self.steering),
                        ordinary: std::mem::take(&mut self.ordinary),
                    }))
                }
            }
        }
    }
    pub(crate) fn restore_mutation(&mut self, undo: InvocationQueueUndo) {
        match undo.0 {
            QueueUndo::Admitted(id) => {
                let _ = self.remove(&id);
                assert!(self.seen.remove(&id));
                self.seen_bytes -= id.as_str().len();
            }
            QueueUndo::Removed { id, kind, position } => match kind {
                InvocationKind::Steering => self.steering.insert(position, id),
                InvocationKind::Queued => self.ordinary.insert(position, id),
            },
            QueueUndo::Order { steering, ordinary } => {
                self.steering = steering;
                self.ordinary = ordinary;
            }
        }
    }
    pub(crate) fn retained_bytes(&self) -> usize {
        self.seen_bytes
            .saturating_add(
                self.seen
                    .capacity()
                    .saturating_mul(std::mem::size_of::<ExecutionId>() + 16),
            )
            .saturating_add(
                (self.steering.capacity() + self.ordinary.capacity())
                    .saturating_mul(std::mem::size_of::<ExecutionId>()),
            )
            .saturating_add(
                self.steering
                    .iter()
                    .chain(self.ordinary.iter())
                    .map(|id| id.as_str().len())
                    .sum::<usize>(),
            )
    }

    /// Returns the number of pending invocations, excluding dispatched work.
    pub fn len(&self) -> usize {
        self.steering.len() + self.ordinary.len()
    }

    /// Reports whether no invocations remain pending.
    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.ordinary.is_empty()
    }
}

#[cfg(test)]
mod reversible_tests {
    use super::*;

    fn id(value: &str) -> ExecutionId {
        ExecutionId::new(value).unwrap()
    }

    #[test]
    fn reversed_queue_tokens_restore_membership_order_and_seen_identity() {
        let mut queue = InvocationQueue::new(4).unwrap();
        queue.enqueue(id("old"), InvocationKind::Queued).unwrap();
        assert_eq!(queue.pop_next().unwrap().0, id("old"));
        let mut undo = Vec::new();
        for name in ["a", "b"] {
            undo.push(
                queue
                    .apply_reversible_mutation(&QueueMutation::Admitted {
                        id: id(name),
                        kind: InvocationKind::Queued,
                    })
                    .unwrap(),
            );
        }
        let order = QueueOrderChange::new(queue.pending(), vec![id("b"), id("a")]).unwrap();
        undo.push(
            queue
                .apply_reversible_mutation(&QueueMutation::Reordered(order))
                .unwrap(),
        );
        undo.push(
            queue
                .apply_reversible_mutation(&QueueMutation::Selected { id: id("b") })
                .unwrap(),
        );
        undo.push(
            queue
                .apply_reversible_mutation(&QueueMutation::Restored)
                .unwrap(),
        );
        assert!(queue.is_empty());
        for token in undo.into_iter().rev() {
            queue.restore_mutation(token);
        }
        assert!(queue.is_empty());
        assert_eq!(
            queue.enqueue(id("old"), InvocationKind::Queued),
            Err(SchedulingError::Duplicate)
        );
        queue.enqueue(id("a"), InvocationKind::Queued).unwrap();
        queue.enqueue(id("b"), InvocationKind::Queued).unwrap();
        assert_eq!(
            queue.pending(),
            vec![
                (id("a"), InvocationKind::Queued),
                (id("b"), InvocationKind::Queued)
            ]
        );
    }
}
