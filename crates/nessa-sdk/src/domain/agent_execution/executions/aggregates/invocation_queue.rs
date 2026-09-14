//! Pending invocation admission and ordering, without provider or runtime effects.
#![deny(missing_docs)]

use std::collections::{HashSet, VecDeque};

use crate::domain::agent_execution::executions::{ExecutionId, InvocationKind, SchedulingError};

/// Owns bounded pending work and remembers every successfully admitted identity.
///
/// Steering work precedes ordinary work; each kind has its own FIFO deque.
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

    /// Returns the number of pending invocations, excluding dispatched work.
    pub fn len(&self) -> usize {
        self.steering.len() + self.ordinary.len()
    }

    /// Reports whether no invocations remain pending.
    pub fn is_empty(&self) -> bool {
        self.steering.is_empty() && self.ordinary.is_empty()
    }
}
