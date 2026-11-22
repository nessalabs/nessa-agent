//! This machine's folders and login shell, for `terminal.open`.
use super::TerminalHost;
use std::path::{Path, PathBuf};

/// The real filesystem and account database.
pub struct SystemTerminalHost;

impl TerminalHost for SystemTerminalHost {
    /// The runtime opens the folder by its real path, one step at a time.
    fn folder(&self, folder: &Path) -> Option<PathBuf> {
        std::fs::canonicalize(folder)
            .ok()
            .filter(|folder| folder.is_dir())
    }

    fn login_shell(&self) -> PathBuf {
        login_shell()
    }

    fn ssh_program(&self) -> PathBuf {
        if let Some(paths) = std::env::var_os("PATH") {
            for dir in std::env::split_paths(&paths) {
                let candidate = dir.join("ssh");
                if candidate.is_file() {
                    return candidate;
                }
            }
        }
        PathBuf::from("/usr/bin/ssh")
    }
}

/// The person's login shell: theirs from the account database, then
/// `$SHELL`, then `/bin/sh`. A gateway started by launchd has no `$SHELL`.
fn login_shell() -> PathBuf {
    #[cfg(unix)]
    {
        // SAFETY: getpwuid returns a pointer into static storage or null;
        // it is read at once, on this thread, before any other call.
        let from_account = unsafe {
            let entry = libc::getpwuid(libc::getuid());
            if entry.is_null() || (*entry).pw_shell.is_null() {
                None
            } else {
                std::ffi::CStr::from_ptr((*entry).pw_shell)
                    .to_str()
                    .ok()
                    .map(PathBuf::from)
            }
        };
        if let Some(shell) = from_account.filter(|shell| shell.is_absolute() && shell.exists()) {
            return shell;
        }
    }
    std::env::var_os("SHELL")
        .map(PathBuf::from)
        .filter(|shell| shell.is_absolute() && shell.exists())
        .unwrap_or_else(|| PathBuf::from("/bin/sh"))
}
