//! SDK physical reads on tracked non-entered workers after passive admission.
//! source owns admission/lifecycle; operation owns SDK source execution/drop.
mod operation;
mod source;
pub(crate) use operation::{DISCOVERY_STEPS_PER_READ, READ_WORK_BUDGET};
pub use source::NessaRecordReadSource;

#[cfg(test)]
pub(crate) use source::TestReadGate;
