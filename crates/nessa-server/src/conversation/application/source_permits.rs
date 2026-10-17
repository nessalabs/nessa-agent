//! A count per source conversation, so one agent's calls bound only that
//! agent: the gateway as a whole keeps no total, and a source's entry leaves
//! with its last permit.
use nessa_protocol::conversation::domain::ConversationId;
use std::{
    collections::HashMap,
    sync::{Arc, Mutex, PoisonError},
};

pub struct SourcePermits {
    limit: usize,
    held: Arc<Mutex<HashMap<ConversationId, usize>>>,
}

/// One of a source's permits; dropping it gives the place back.
pub struct SourcePermit {
    held: Arc<Mutex<HashMap<ConversationId, usize>>>,
    source: ConversationId,
}

impl SourcePermits {
    pub fn new(limit: usize) -> Self {
        Self {
            limit,
            held: Arc::default(),
        }
    }

    /// A permit for `source`, or `None` when it already holds `limit`.
    pub fn try_acquire(&self, source: &ConversationId) -> Option<SourcePermit> {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        let count = held.entry(source.clone()).or_default();
        if *count == self.limit {
            return None;
        }
        *count += 1;
        Some(SourcePermit {
            held: self.held.clone(),
            source: source.clone(),
        })
    }

    /// How many permits `source` holds now.
    pub fn held(&self, source: &ConversationId) -> usize {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(source)
            .copied()
            .unwrap_or_default()
    }

    /// How many sources hold any permit now.
    pub fn sources(&self) -> usize {
        self.held
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .len()
    }
}

impl Drop for SourcePermit {
    fn drop(&mut self) {
        let mut held = self.held.lock().unwrap_or_else(PoisonError::into_inner);
        if let Some(count) = held.get_mut(&self.source) {
            *count -= 1;
            if *count == 0 {
                held.remove(&self.source);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn source(index: u128) -> ConversationId {
        ConversationId::new(&uuid::Uuid::from_u128(index + 1).to_string()).unwrap()
    }

    #[test]
    fn each_source_has_its_own_limit_and_leaves_with_its_last_permit() {
        let permits = SourcePermits::new(2);
        let first = [
            permits.try_acquire(&source(0)).unwrap(),
            permits.try_acquire(&source(0)).unwrap(),
        ];
        assert!(permits.try_acquire(&source(0)).is_none());
        // Another source is not bounded by the first one's permits.
        let others = (1..10_000)
            .map(|index| permits.try_acquire(&source(index)).unwrap())
            .collect::<Vec<_>>();
        assert_eq!(permits.sources(), 10_000);
        drop(others);
        let [one, two] = first;
        drop(one);
        assert_eq!(permits.held(&source(0)), 1);
        assert!(permits.try_acquire(&source(0)).is_some());
        drop(two);
        assert_eq!(permits.sources(), 0);
    }
}
