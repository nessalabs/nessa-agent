//! A cloud VM's connection as a Cloud Box's bootstrap uses it: short
//! commands over the workspace SDK's connection. The SDK owns the connection
//! itself (its key, the pinned host, and ending what it started).
use std::{io, sync::Arc, time::Duration};
use workspace_sdk::ports::IConnection;

pub(super) struct VmLink {
    connection: Arc<dyn IConnection>,
    /// The VM, as its diagnostics name it.
    vm: String,
}

/// The longest any one command may run.
const COMMAND_WAIT: Duration = Duration::from_secs(15 * 60);
/// Most lines of a command's standard error kept in the log.
const MAX_STDERR_LINES: usize = 32;

impl VmLink {
    pub(super) fn new(connection: Arc<dyn IConnection>, vm: &str) -> Arc<Self> {
        Arc::new(Self {
            connection,
            vm: vm.to_owned(),
        })
    }
    /// Run `command` in the VM, `input` on its standard input, answering
    /// the end of what it printed.
    pub(super) async fn run(
        &self,
        command: String,
        input: Option<std::fs::File>,
    ) -> io::Result<String> {
        let ran = self
            .connection
            .run(&command, input, COMMAND_WAIT)
            .await
            .map_err(|error| match error {
                workspace_sdk::Error::Timeout(_) => io::Error::from(io::ErrorKind::TimedOut),
                other => io::Error::other(other),
            })?;
        log_stderr(&self.vm, &ran.stderr);
        Ok(ran.output)
    }
}

/// Log the first lines a VM command wrote to its standard error, each line
/// its own record so nothing in it can forge another.
pub(super) fn log_stderr(vm: &str, stderr: &str) {
    for line in stderr.lines().take(MAX_STDERR_LINES) {
        tracing::warn!(vm, line, "cloud VM");
    }
}
