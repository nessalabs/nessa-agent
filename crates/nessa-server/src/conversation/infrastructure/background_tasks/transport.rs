//! SSH task transport: one bounded terminal record after Shepherd cleans up.
//! The stream owner keeps the workspace InUse; the conversation owns settlement.
use super::super::workspaces::ThreadWorkspaces;
use super::encoding::Outcome;
use crate::conversation::{
    application::background_tasks::{
        BackgroundRun, BackgroundRunner, BackgroundTarget, BackgroundTaskError,
    },
    domain::{BackgroundCommand, TaskCause, TaskInitiator, TaskLifecycle, TaskOutcome, TaskStop},
};
use std::{io, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::watch,
};

// JSON escaping and replacement characters can expand the retained byte tail.
const MAX_RECORD_BYTES: usize = TaskOutcome::MAX_OUTPUT_BYTES * 6 + 8192;

fn interrupted() -> TaskOutcome {
    let mut outcome = TaskOutcome::interrupted();
    outcome.error = Some("The VM connection ended before an outcome was recorded. The command was not restarted and cleanup is unconfirmed.".into());
    outcome
}

pub(crate) struct WorkspaceBackgroundTasks {
    local: Arc<dyn BackgroundRunner>,
    workspaces: Arc<ThreadWorkspaces>,
}
impl WorkspaceBackgroundTasks {
    pub(crate) fn new(local: Arc<dyn BackgroundRunner>, workspaces: Arc<ThreadWorkspaces>) -> Self {
        Self { local, workspaces }
    }
}
impl BackgroundRunner for WorkspaceBackgroundTasks {
    fn run(
        &self,
        command: BackgroundCommand,
        target: BackgroundTarget,
        stop: watch::Receiver<Option<TaskStop>>,
    ) -> BackgroundRun<'_, TaskOutcome> {
        Box::pin(async move {
            let BackgroundTarget::Vm {
                conversation,
                workspace,
            } = target
            else {
                return self.local.run(command, target, stop).await;
            };
            if stop.borrow().is_some() {
                let mut outcome = TaskOutcome::not_started();
                outcome.cause = TaskCause::Cancelled;
                return outcome;
            }
            let timeout = Duration::from_secs(command.timeout_seconds() + 15);
            let stream = match self
                .workspaces
                .background(conversation, &workspace, &command)
                .await
            {
                Ok(stream) => stream,
                Err(error) => {
                    tracing::warn!(%error, "background SSH session could not be opened");
                    return interrupted();
                }
            };
            let _owner = stream.owner;
            tokio::time::timeout(timeout, receive(stream.input, stream.output, stop))
                .await
                .unwrap_or_else(|_| interrupted())
        })
    }
    fn shutdown(&self) -> BackgroundRun<'_, Result<(), BackgroundTaskError>> {
        // The conversation signals its owners before this call, then joins
        // them after it. Remote readers await their VM's cleanup evidence while
        // this independently drains the local supervisor.
        self.local.shutdown()
    }
}

async fn receive(
    mut input: impl AsyncWrite + Unpin,
    output: impl AsyncRead + Unpin,
    mut stop: watch::Receiver<Option<TaskStop>>,
) -> TaskOutcome {
    let mut bytes = Vec::new();
    let read = {
        let mut limited = output.take((MAX_RECORD_BYTES + 1) as u64);
        let read = limited.read_to_end(&mut bytes);
        tokio::pin!(read);
        tokio::select! {
            biased;
            result = &mut read => result,
            _ = async { let _ = stop.wait_for(Option::is_some).await; } => {
                // Wait for Shepherd's cleanup evidence after requesting stop.
                let _ = input.write_all(&[1]).await;
                read.await
            }
        }
    };
    if read.is_err() || bytes.len() > MAX_RECORD_BYTES {
        return interrupted();
    }
    serde_json::from_slice::<Outcome>(&bytes)
        .ok()
        .and_then(|wire| wire.decode().ok())
        .filter(|outcome| {
            outcome
                .error
                .as_ref()
                .is_none_or(|error| error.len() <= 4096)
                && outcome
                    .signal
                    .as_ref()
                    .is_none_or(|signal| signal.len() <= 128)
        })
        .filter(|outcome| {
            TaskLifecycle::admitted(0)
                .settle(outcome.clone(), 0)
                .is_ok()
        })
        .unwrap_or_else(interrupted)
}

/// The VM half treats a stop byte or stdin EOF as cancellation, then waits for
/// Shepherd's scope cleanup before writing the terminal record.
pub(crate) async fn serve(
    runner: &dyn BackgroundRunner,
    command: BackgroundCommand,
    workspace: String,
    mut input: impl AsyncRead + Unpin,
    mut output: impl AsyncWrite + Unpin,
) -> io::Result<()> {
    let (cancel, stop) = watch::channel(None);
    let cause = TaskStop::GatewayStopped(
        TaskInitiator::new(
            "background-run".into(),
            "ssh".into(),
            "connection-ended".into(),
        )
        .expect("static initiator"),
    );
    let mut byte = [0];
    let read = input.read(&mut byte);
    tokio::pin!(read);
    // Observe an already closed connection or queued stop before asking the
    // runner for any process effect. A later exit still wins the select below.
    if futures_util::poll!(read.as_mut()).is_ready() {
        cancel.send_replace(Some(cause.clone()));
    }
    let run = runner.run(command, BackgroundTarget::Local(workspace), stop);
    tokio::pin!(run);
    let outcome = tokio::select! {
        biased;
        outcome = &mut run => outcome,
        _ = &mut read => {
            cancel.send_replace(Some(cause));
            run.await
        }
    };
    let bytes = serde_json::to_vec(&Outcome::encode(&outcome)).map_err(io::Error::other)?;
    if bytes.len() > MAX_RECORD_BYTES {
        return Err(io::Error::other("background outcome too large"));
    }
    output.write_all(&bytes).await?;
    output.shutdown().await
}

#[cfg(test)]
#[path = "../../../../tests/conversation/background_transport.rs"]
mod tests;
