//! Immutable count and carrying cost of the asks already open.
#![deny(missing_docs)]

use super::{AgentQuestion, QuestionRefusalReason, MAX_OPEN_ASK_COST, MAX_OPEN_QUESTIONS};
use crate::domain::agent_execution::ExecutionError;

/// A bounded summary derived from actual open asks, retaining no ask contents.
///
/// The caller supplies its retained asks. This value derives their count and
/// carrying cost; it does not independently observe a provider or prove audit
/// delivery. Cloning shares no mutable admission authority.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct OpenQuestionAccounting {
    count: usize,
    cost: usize,
}

impl OpenQuestionAccounting {
    /// Summarize `asks` by reference using [`AgentQuestion::carrying_cost`].
    ///
    /// The count is in asks and the cost is in carrying bytes. Construction
    /// performs no I/O and retains only the totals. An empty collection has
    /// zero count and cost. Iteration stops at the first exceeded limit.
    ///
    /// # Errors
    ///
    /// Returns [`ExecutionError::InvalidQuestionRefusal`] if the collection
    /// exceeds [`MAX_OPEN_QUESTIONS`] or its cost exceeds [`MAX_OPEN_ASK_COST`].
    pub fn new<'a>(
        asks: impl IntoIterator<Item = &'a AgentQuestion>,
    ) -> Result<Self, ExecutionError> {
        let mut count = 0;
        let mut cost: usize = 0;
        for ask in asks {
            count += 1;
            cost = cost.saturating_add(ask.carrying_cost());
            if count > MAX_OPEN_QUESTIONS || cost > MAX_OPEN_ASK_COST {
                return Err(ExecutionError::InvalidQuestionRefusal);
            }
        }
        Ok(Self { count, cost })
    }

    /// Number of asks in the supplied collection.
    pub fn count(&self) -> usize {
        self.count
    }

    /// Total carrying bytes derived from the supplied asks.
    pub fn cost(&self) -> usize {
        self.cost
    }

    /// Why `ask` cannot join the open collection, or `None` when it fits.
    ///
    /// The count limit takes precedence when both limits would be exceeded.
    /// This is a pure comparison; it admits no ask and creates no audit record.
    pub fn reason_for(&self, ask: &AgentQuestion) -> Option<QuestionRefusalReason> {
        if self.count >= MAX_OPEN_QUESTIONS {
            Some(QuestionRefusalReason::TooManyOpen)
        } else if self.cost.saturating_add(ask.carrying_cost()) > MAX_OPEN_ASK_COST {
            Some(QuestionRefusalReason::TooLarge)
        } else {
            None
        }
    }
}

#[cfg(test)]
#[path = "../../../../../tests/domain/agent_execution/question_accounting.rs"]
mod tests;
