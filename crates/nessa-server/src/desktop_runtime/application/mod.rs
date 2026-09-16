//! Coordinates permanent admission closure, agent cleanup and mandatory upgrade evidence.
mod retirement;
pub(crate) use retirement::{
    restore_retirement, retire, RetirementAudit, RetirementRecord, RetirementResult,
};
