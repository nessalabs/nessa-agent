//! Coordinates permanent admission closure, agent cleanup and mandatory upgrade evidence.
#[cfg(any(target_os = "macos", test))]
mod retirement;
#[cfg(any(target_os = "macos", test))]
pub(crate) use retirement::{
    restore_retirement, retire, RetirementAudit, RetirementRecord, RetirementResult,
};
