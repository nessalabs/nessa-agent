//! Coordinates permanent admission closure, agent cleanup and mandatory upgrade evidence.
#[cfg(any(target_os = "macos", target_os = "linux", test))]
mod retirement;
#[cfg(any(target_os = "macos", target_os = "linux", test))]
pub(crate) use retirement::{
    restore_retirement, retire, RetirementAudit, RetirementRecord, RetirementResult,
};
