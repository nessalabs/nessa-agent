//! SDK physical reads on tracked non-entered workers after passive admission.
//! source owns admission/lifecycle; operation owns SDK source execution/drop.
mod operation;
mod source;
pub use source::NessaRecordReadSource;
