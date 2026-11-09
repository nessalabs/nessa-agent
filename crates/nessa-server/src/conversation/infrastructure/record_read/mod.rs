//! SDK physical reads on tracked non-entered workers after passive admission.
//! source owns admission/lifecycle; operation owns SDK source execution/drop.
//! An occupied discovery cache is not a step: the read waits for the owner
//! or for the work budget, and that budget starts when discovery calls start.
mod operation;
mod source;
pub(crate) use operation::{DISCOVERY_STEPS_PER_READ, READ_WORK_BUDGET};
pub use source::{InvalidReadWorkBudget, NessaRecordReadSource};

#[cfg(test)]
pub(crate) use source::TestReadGate;
