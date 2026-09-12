use super::binding::ClaudeAcpConfig;
use crate::{
    application::agent_binding::{BindingError, StopOutcome},
    domain::effective_capabilities::value_objects::EffectiveCapabilities,
};
use std::{process::Stdio, time::Duration};
use tokio::{
    io::AsyncReadExt,
    process::{Child, ChildStdin, ChildStdout, Command},
    task::JoinHandle,
    time::{sleep, timeout, Instant},
};

/// Only the pinned harness's non-detaching file-tool profile may use this scope.
/// A process group is deliberately not offered as arbitrary command containment.
pub(super) struct ProcessScope {
    child: Child,
    group: u32,
    pub stdin: Option<ChildStdin>,
    pub stdout: Option<ChildStdout>,
    stderr: JoinHandle<()>,
    cleaned: bool,
}
impl ProcessScope {
    pub fn spawn(
        config: &ClaudeAcpConfig,
        capabilities: &EffectiveCapabilities,
    ) -> Result<Self, BindingError> {
        let mut command = Command::new(&config.executable);
        command
            .args(&config.arguments)
            .current_dir(&config.workspace)
            .env_clear()
            .envs(&config.environment)
            .env("ANTHROPIC_MODEL", capabilities.model().model_id())
            .env(
                "ANTHROPIC_CUSTOM_MODEL_OPTION",
                capabilities.model().model_id(),
            )
            .env(
                "CLAUDE_CODE_MAX_OUTPUT_TOKENS",
                capabilities.limits().max_output().to_string(),
            )
            .env("DISABLE_AUTOUPDATER", "1")
            .env("CLAUDE_CODE_DISABLE_NONESSENTIAL_TRAFFIC", "1")
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        command.process_group(0);
        let mut child = command
            .spawn()
            .map_err(|error| BindingError::Transport(error.to_string()))?;
        let group = child
            .id()
            .ok_or_else(|| BindingError::Transport("process has no ID".into()))?;
        let stdin = child.stdin.take();
        let stdout = child.stdout.take().expect("piped stdout");
        let mut stderr = child.stderr.take().expect("piped stderr");
        let stderr = tokio::spawn(async move {
            // Drain without retaining provider output, which may contain secrets.
            let mut bytes = [0; 8192];
            while let Ok(count) = stderr.read(&mut bytes).await {
                if count == 0 {
                    break;
                }
            }
        });
        Ok(Self {
            child,
            group,
            stdin,
            stdout: Some(stdout),
            stderr,
            cleaned: false,
        })
    }
    pub async fn cleanup(
        &mut self,
        grace: Duration,
        kill_timeout: Duration,
    ) -> Result<StopOutcome, BindingError> {
        self.stdin.take(); // EOF asks the adapter to tear down every owned query.
        let mut forced = false;
        let mut gone = self.wait_scope(grace).await;
        if !gone {
            forced = true;
            signal_group(self.group, false)?;
            gone = self.wait_scope(kill_timeout).await;
        }
        if !gone {
            signal_group(self.group, true)?;
            gone = self.wait_scope(kill_timeout).await;
        }
        self.stderr.abort();
        let _ = (&mut self.stderr).await;
        if !gone {
            return Err(BindingError::CleanupUncertain);
        }
        // Reap the direct child, even if the process group disappeared first.
        timeout(kill_timeout, self.child.wait())
            .await
            .map_err(|_| BindingError::CleanupUncertain)?
            .map_err(|_| BindingError::CleanupUncertain)?;
        self.cleaned = true;
        Ok(StopOutcome { forced })
    }
    async fn wait_scope(&mut self, budget: Duration) -> bool {
        let end = Instant::now() + budget;
        loop {
            // try_wait reaps the parent; unreaped descendants still count as live.
            if self.child.try_wait().is_err() {
                return false;
            }
            if matches!(group_exists(self.group), Ok(false)) {
                return true;
            }
            if Instant::now() >= end {
                return false;
            }
            sleep(Duration::from_millis(20)).await;
        }
    }
}
impl Drop for ProcessScope {
    fn drop(&mut self) {
        if !self.cleaned {
            let _ = signal_group(self.group, true);
        }
        self.stderr.abort();
    }
}
#[cfg(unix)]
fn signal_group(group: u32, force: bool) -> Result<(), BindingError> {
    // The group ID is assigned by the OS to this child's new process group.
    let result = unsafe {
        libc::kill(
            -(group as i32),
            if force { libc::SIGKILL } else { libc::SIGTERM },
        )
    };
    if result == 0 || std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(())
    } else {
        Err(BindingError::CleanupUncertain)
    }
}
#[cfg(unix)]
fn group_exists(group: u32) -> Result<bool, BindingError> {
    let result = unsafe { libc::kill(-(group as i32), 0) };
    if result == 0 {
        Ok(true)
    } else if std::io::Error::last_os_error().raw_os_error() == Some(libc::ESRCH) {
        Ok(false)
    } else {
        Err(BindingError::CleanupUncertain)
    }
}
#[cfg(not(unix))]
fn signal_group(_: u32, _: bool) -> Result<(), BindingError> {
    Err(BindingError::CleanupUncertain)
}
#[cfg(not(unix))]
fn group_exists(_: u32) -> Result<bool, BindingError> {
    Err(BindingError::CleanupUncertain)
}
