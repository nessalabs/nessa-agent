//! The organization budget (issue #791): how many leases of an agent, and
//! how many commands, may run at once across every paired environment of
//! this gateway, which is one organization's. Configured as
//! `limits.organizationLeases` and `limits.organizationCommands`; absent,
//! nothing is counted against a bound.
//!
//! ```text
//! a paired environment asked for a lease ──▶ OrganizationBudgets::lease()
//!   room ──▶ a BudgetPlace, held by the lease's hold until it is let go
//!   none ──▶ refused environment_busy before the environment is asked
//! a paired environment asked for a command ──▶ OrganizationBudgets::command()
//!   (the same, held by the command's hold)
//! ```
//!
//! The count is what runs now, so it lives in memory and starts at zero
//! when the gateway does: every lease and command it counted ended with the
//! process that held it. It is a bound on work, not a decision about who may
//! do it, so it is no policy and asks nothing of Cedar.
use std::sync::{Arc, Mutex, PoisonError};

/// One kind of work counted: what runs now, against its bound.
#[derive(Debug, Default)]
struct Kind {
    /// `None` counts without a bound.
    bound: Option<usize>,
    running: Mutex<usize>,
}

impl Kind {
    fn take(self: &Arc<Self>) -> Option<BudgetPlace> {
        {
            let mut running = self.running.lock().unwrap_or_else(PoisonError::into_inner);
            if self.bound.is_some_and(|bound| *running >= bound) {
                return None;
            }
            *running += 1;
        }
        Some(BudgetPlace { kind: self.clone() })
    }

    fn running(&self) -> usize {
        *self.running.lock().unwrap_or_else(PoisonError::into_inner)
    }
}

/// One place in the budget, given back when dropped.
#[derive(Debug)]
pub(crate) struct BudgetPlace {
    kind: Arc<Kind>,
}

impl Drop for BudgetPlace {
    fn drop(&mut self) {
        let mut running = self
            .kind
            .running
            .lock()
            .unwrap_or_else(PoisonError::into_inner);
        *running = running.saturating_sub(1);
    }
}

/// Leases of an agent and commands running on this gateway's paired
/// environments, against the bounds configured for them: one budget,
/// shared by every one of them.
#[derive(Clone, Debug, Default)]
pub(crate) struct OrganizationBudgets(Arc<(Arc<Kind>, Arc<Kind>)>);

impl OrganizationBudgets {
    /// At most `leases` leases of an agent and `commands` commands at once;
    /// `None` bounds nothing.
    pub(crate) fn new(leases: Option<usize>, commands: Option<usize>) -> Self {
        let kind = |bound| {
            Arc::new(Kind {
                bound,
                running: Mutex::new(0),
            })
        };
        Self(Arc::new((kind(leases), kind(commands))))
    }

    /// A place for one more lease of an agent, or `None` with every one the
    /// budget allows running.
    pub(crate) fn lease(&self) -> Option<BudgetPlace> {
        self.0 .0.take()
    }

    /// A place for one more command, or `None` with every one the budget
    /// allows running.
    pub(crate) fn command(&self) -> Option<BudgetPlace> {
        self.0 .1.take()
    }

    /// Leases of an agent and commands holding a place now.
    pub(crate) fn running(&self) -> (usize, usize) {
        (self.0 .0.running(), self.0 .1.running())
    }
}

#[cfg(test)]
#[path = "../../../tests/conversation/organization_budget.rs"]
mod tests;
