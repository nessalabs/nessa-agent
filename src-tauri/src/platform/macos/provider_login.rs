use std::{process::Child, time::Duration};

use crate::provider_authentication::{LoginFailure, Provider, ProviderLogin};

/// Login opened in the macOS Terminal, selected by host composition.
pub struct MacosProviderLogin;

impl ProviderLogin for MacosProviderLogin {
    fn available(&self) -> bool {
        true
    }
    fn open(&self, provider: Provider) -> Result<(), LoginFailure> {
        open(provider)
    }
}

fn script(provider: Provider) -> &'static str {
    // Only this closed provider enum chooses terminal syntax, never external text.
    match provider {
        Provider::Claude => {
            "tell application \"Terminal\"\nactivate\ndo script \"claude auth login\"\nend tell"
        }
        Provider::Codex => {
            "tell application \"Terminal\"\nactivate\ndo script \"codex login\"\nend tell"
        }
    }
}

// First-use Automation consent waits on the person; retain a bounded wait.
const LOGIN_TIMEOUT: Duration = Duration::from_secs(120);

fn open(provider: Provider) -> Result<(), LoginFailure> {
    use std::process::{Command, Stdio};
    let mut child = Command::new("/usr/bin/osascript")
        .args(["-e", script(provider)])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|error| match error.kind() {
            std::io::ErrorKind::NotFound => LoginFailure::Unavailable,
            _ => LoginFailure::LaunchFailed,
        })?;
    wait_for_login(&mut child, LOGIN_TIMEOUT)
}

fn wait_for_login(child: &mut Child, timeout: Duration) -> Result<(), LoginFailure> {
    use std::{thread, time::Instant};

    let deadline = Instant::now() + timeout;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                return if status.success() {
                    Ok(())
                } else {
                    Err(LoginFailure::LaunchFailed)
                }
            }
            Ok(None) if Instant::now() < deadline => thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return Err(LoginFailure::LaunchFailed);
            }
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/platform/macos/provider_login.rs"]
mod tests;
