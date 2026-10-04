use super::{LoginFailure, Provider, ProviderLogin};

/// The native terminal launcher; unsupported platforms refuse explicitly.
pub struct NativeProviderLogin;

impl ProviderLogin for NativeProviderLogin {
    fn open(&self, provider: Provider) -> Result<(), LoginFailure> {
        open(provider)
    }
}

#[cfg(target_os = "macos")]
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

#[cfg(target_os = "macos")]
fn open(provider: Provider) -> Result<(), LoginFailure> {
    use std::{
        process::{Command, Stdio},
        thread,
        time::{Duration, Instant},
    };
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
    let deadline = Instant::now() + Duration::from_secs(10);
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

#[cfg(not(target_os = "macos"))]
fn open(_: Provider) -> Result<(), LoginFailure> {
    Err(LoginFailure::Unavailable)
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    #[test]
    fn each_provider_opens_its_own_login_without_external_command_text() {
        assert!(script(Provider::Claude).contains("claude auth login"));
        assert!(script(Provider::Codex).contains("codex login"));
    }
}
