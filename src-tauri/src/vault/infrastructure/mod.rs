//! Native child/anonymous-pipe implementation and the trusted-window entry point.
mod command;
mod process;
pub use command::{__cmd__vault_command, __tauri_command_name_vault_command, vault_command};
pub use process::NativeVault;
