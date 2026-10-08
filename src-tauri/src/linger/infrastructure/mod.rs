//! logind, the audit files, and the commands setup calls.
//!
//! The live logind adapter is not run in tests. It would prompt and might
//! enable linger on the machine running them. What is tested is the name
//! classification that adapter uses, and the decision that consumes its result.

#[cfg(any(test, target_os = "linux"))]
mod audit;
mod commands;
#[cfg(any(test, target_os = "linux"))]
mod errors;
#[cfg(target_os = "linux")]
mod logind;

#[cfg(any(test, target_os = "linux"))]
pub(crate) use audit::{FileLingerAudit, UnavailableLingerAudit};
pub(crate) use commands::{
    __cmd__linger_accept, __cmd__linger_decline, __cmd__linger_status,
    __tauri_command_name_linger_accept, __tauri_command_name_linger_decline,
    __tauri_command_name_linger_status, linger_accept, linger_decline, linger_status,
};
#[cfg(target_os = "linux")]
pub(crate) use logind::SystemLogind;
