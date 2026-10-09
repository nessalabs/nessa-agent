//! logind and the commands setup calls.
//!
//! Unit tests do not open the system bus. `linger_live` does, for user `lt`.

mod commands;
#[cfg(any(test, target_os = "linux"))]
mod errors;
#[cfg(target_os = "linux")]
mod logind;

pub(crate) use commands::{
    __cmd__linger_accept, __cmd__linger_status, __tauri_command_name_linger_accept,
    __tauri_command_name_linger_status, linger_accept, linger_status,
};
#[cfg(target_os = "linux")]
pub(crate) use logind::SystemLogind;
