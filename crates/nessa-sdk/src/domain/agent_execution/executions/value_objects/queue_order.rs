//! Immutable evidence of one caller-selected change to pending dispatch order.
#![deny(missing_docs)]
use super::{ExecutionId, InvocationKind};
use std::collections::HashSet;

/// Why a requested pending order cannot replace the current order.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum QueueOrderError {
    /// Requested membership differs from the current pending set.
    QueueChanged,
    /// A steering input would follow ordinary queued input.
    PriorityConflict,
    /// An identity occurs more than once.
    Duplicate,
    /// The order exceeds the supported bounded pending capacity.
    TooLarge,
}
/// Complete before/after evidence. Identities and priority kinds never change.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct QueueOrderChange {
    before: Vec<(ExecutionId, InvocationKind)>,
    after: Vec<ExecutionId>,
}
impl QueueOrderChange {
    /// Maximum pending entries in one order change.
    pub const MAX_PENDING: usize = 64;
    /// Validate both complete orders, their exact membership and priority boundary.
    /// `before` is current dispatch order; `after` is the caller's desired order.
    /// Returns a typed rejection without changing either input's identities.
    pub fn new(
        before: Vec<(ExecutionId, InvocationKind)>,
        after: Vec<ExecutionId>,
    ) -> Result<Self, QueueOrderError> {
        if before.len() > Self::MAX_PENDING || after.len() > Self::MAX_PENDING {
            return Err(QueueOrderError::TooLarge);
        }
        let prior: HashSet<_> = before.iter().map(|(id, _)| id).collect();
        let next: HashSet<_> = after.iter().collect();
        if prior.len() != before.len() || next.len() != after.len() {
            return Err(QueueOrderError::Duplicate);
        }
        if prior != next {
            return Err(QueueOrderError::QueueChanged);
        }
        let mut ordinary = false;
        for (_, kind) in &before {
            if *kind == InvocationKind::Queued {
                ordinary = true;
            } else if ordinary {
                return Err(QueueOrderError::PriorityConflict);
            }
        }
        ordinary = false;
        for id in &after {
            let kind = before
                .iter()
                .find(|(known, _)| known == id)
                .expect("equal membership")
                .1;
            if kind == InvocationKind::Queued {
                ordinary = true;
            } else if ordinary {
                return Err(QueueOrderError::PriorityConflict);
            }
        }
        Ok(Self {
            before: before.into_boxed_slice().into_vec(),
            after: after.into_boxed_slice().into_vec(),
        })
    }
    /// Complete prior dispatch order, with immutable admission priorities.
    pub fn before(&self) -> &[(ExecutionId, InvocationKind)] {
        &self.before
    }
    /// Complete desired dispatch order over exactly the same identities.
    pub fn after(&self) -> &[ExecutionId] {
        &self.after
    }
    /// Whether this command requests the existing order.
    pub fn is_unchanged(&self) -> bool {
        self.before.iter().map(|(id, _)| id).eq(self.after.iter())
    }
}
