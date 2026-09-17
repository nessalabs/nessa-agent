//! Reject invalid unbounded change lists before constructing their owned records.
use serde::de;
use std::cell::Cell;

pub(super) struct Indices {
    initial: usize,
    queue_events: Cell<usize>,
    next_append: Cell<usize>,
    previous: Cell<Option<usize>>,
    declared: Cell<Option<usize>>,
}
impl Indices {
    pub(super) fn new(initial: usize) -> Self {
        Self {
            initial,
            queue_events: Cell::new(0),
            next_append: Cell::new(initial),
            previous: Cell::new(None),
            declared: Cell::new(None),
        }
    }
    pub(super) fn index<E: de::Error>(&self, index: usize) -> Result<(), E> {
        if self.previous.get().is_some_and(|prior| prior >= index) || index > self.next_append.get()
        {
            return Err(E::custom(
                "journal invocation indices are not ordered and contiguous",
            ));
        }
        if index == self.next_append.get() {
            self.next_append.set(
                index
                    .checked_add(1)
                    .ok_or_else(|| E::custom("journal index overflow"))?,
            );
        }
        self.previous.set(Some(index));
        Ok(())
    }
    pub(super) fn count(&self, count: usize) {
        self.declared.set(Some(count));
    }
    pub(super) fn queue_events(&self, count: usize) {
        self.queue_events.set(count);
    }
    pub(super) fn finish<E: de::Error>(&self) -> Result<(), E> {
        let count = self
            .declared
            .get()
            .ok_or_else(|| E::custom("journal invocation count missing"))?;
        if self.queue_events.get() > count.saturating_mul(3).saturating_add(1024) {
            return Err(E::custom("queue history exceeds invocation bound"));
        }
        if self.previous.get().is_some_and(|index| index >= count)
            || (count > self.initial && self.next_append.get() != count)
        {
            return Err(E::custom("journal invocation count disagrees with changes"));
        }
        Ok(())
    }
    pub(super) fn needs_metadata(&self) -> bool {
        self.previous
            .get()
            .is_some_and(|index| index >= self.initial)
    }
}
