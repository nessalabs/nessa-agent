//! Native child/anonymous-pipe implementation and the trusted-window entry point.
mod command;
#[cfg(target_os = "macos")]
mod keychain;
mod process;
pub use command::{
    __cmd__vault_biometrics, __cmd__vault_command, __tauri_command_name_vault_biometrics,
    __tauri_command_name_vault_command, vault_biometrics, vault_command,
};
#[cfg(target_os = "macos")]
pub use keychain::TouchIdKeychain;
pub use process::NativeVault;
