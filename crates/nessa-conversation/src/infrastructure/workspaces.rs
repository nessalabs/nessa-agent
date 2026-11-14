//! Adapt per-thread workspace streams to the conversation environment contract.
use super::ssh_environment::{
    ArtifactTransport, EnvironmentAudit, HostInstaller, LeaseConnection, RemoteShell, ShellFuture,
    SshEnvironment, WorkspaceConnector,
};
use crate::timer::clock;
use crate::{
    application::{
        ChannelStore, ConversationError, Environment, EnvironmentFuture, ThreadEnvironments,
        WorkspaceRest, MAX_RETAINED_ENVIRONMENTS,
    },
    domain::{BackgroundCommand, TaskStop},
    env::LEASE_PROTOCOL,
    workspaces::application::{
        RestCause, ServedCopy, WorkspaceRuntime, WorkspaceStream, Workspaces,
    },
};
use futures_util::future::{BoxFuture, FutureExt, Shared};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};
use tokio::sync::watch::Receiver;

/// The longest wait between two asks to let a resting thread's connection
/// go: 1 + 2 + 4 + 8 + 16 seconds in all.
const LET_GO_PATIENCE: Duration = Duration::from_secs(16);

pub struct ThreadWorkspaces {
    pub service: Arc<Workspaces>,
    pub channels: Arc<dyn ChannelStore>,
    pub audit: Arc<dyn EnvironmentAudit>,
    /// Puts this build in a VM whose image's copy speaks another lease
    /// protocol, as on an SSH host.
    pub installer: Arc<HostInstaller>,
    pub environments: Mutex<BTreeMap<String, Arc<SshEnvironment>>>,
    /// Asked for each VM lease's saved login; `None` hands none.
    pub logins: Option<Arc<dyn super::ssh_environment::SavedLogins>>,
}
/// The VM's copy being chosen, and installed if it must be, on a task of
/// its own: an install outlasts the opening that started it.
type PendingCopy = Shared<BoxFuture<'static, Result<ServedCopy, LeaseRefusal>>>;
struct Connector {
    service: Arc<Workspaces>,
    channels: Arc<dyn ChannelStore>,
    conversation: ConversationId,
    copies: VmCopies,
}
/// Which copy of `nessa` serves one thread's VM.
struct VmCopies {
    audit: Arc<dyn EnvironmentAudit>,
    installer: Arc<HostInstaller>,
    conversation: ConversationId,
    /// The one [`PendingCopy`] of this thread's VM, while it runs.
    choosing: Arc<Mutex<Option<PendingCopy>>>,
}
impl VmCopies {
    /// [`served_copy`] for this thread's VM, carried through on a task of
    /// its own. An install of a build into a VM can take longer than one
    /// opening may wait: one that gives up leaves it running, and the
    /// next opening waits on the same one, never a second.
    async fn copy(
        &self,
        runtime: Arc<dyn WorkspaceRuntime>,
        install: bool,
    ) -> Result<ServedCopy, LeaseRefusal> {
        let pending = {
            let mut choosing = self
                .choosing
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            match choosing.as_ref() {
                Some(pending) => pending.clone(),
                None => {
                    let installer = self.installer.clone();
                    let audit = self.audit.clone();
                    let conversation = self.conversation.clone();
                    let done = self.choosing.clone();
                    let task = tokio::spawn(async move {
                        // Let go once done, a panic included, so the next
                        // opening asks the VM again.
                        let _done = Done(done);
                        served_copy(
                            &installer,
                            audit.as_ref(),
                            &conversation,
                            runtime.as_ref(),
                            install,
                        )
                        .await
                    });
                    let pending = async move {
                        task.await
                            .unwrap_or(Err(LeaseRefusal::EnvironmentUnreachable))
                    }
                    .boxed()
                    .shared();
                    *choosing = Some(pending.clone());
                    pending
                }
            }
        };
        pending.await
    }
}
/// The copy of `nessa` in `conversation`'s VM that serves its leases,
/// chosen before either starts: the image's own when it speaks this build's
/// lease protocol; else this build, where it is installed in the VM, or is
/// installed now by `installer`. A VM made from an image an earlier build
/// pinned keeps its files and gets this build beside them. An account of a
/// lease already held (`install` false) never installs, and nothing is
/// installed on a guess: a VM that could not be asked, with no copy found,
/// is served its image's.
async fn served_copy(
    installer: &HostInstaller,
    audit: &dyn EnvironmentAudit,
    conversation: &ConversationId,
    runtime: &dyn WorkspaceRuntime,
    install: bool,
) -> Result<ServedCopy, LeaseRefusal> {
    let image = runtime.image_protocol().await;
    if image.as_deref() == Some(LEASE_PROTOCOL) {
        return Ok(ServedCopy::Image);
    }
    // The VM is named by its thread in the install's records, and the
    // thread stands for the lease: they join the thread's lease records.
    let host = SshDestination::new(format!("boat-{conversation}"))
        .map_err(|_| LeaseRefusal::EnvironmentUnreachable)?;
    let shell = VmShell(runtime);
    if installer.installed_on(&shell, &host).await != Some(true) {
        if !(install && image.is_some()) {
            return Ok(ServedCopy::Image);
        }
        installer
            .ensure_on(&shell, &host, &conversation.to_string(), audit)
            .await?;
    }
    Ok(ServedCopy::Installed(installer.serve_command().await?))
}
/// Clears a thread's [`PendingCopy`] when its task ends, however it ends.
struct Done(Arc<Mutex<Option<PendingCopy>>>);
impl Drop for Done {
    fn drop(&mut self) {
        *self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
    }
}
/// A VM's own connection, as the installer's shell: the host it is given
/// only names the VM in the records.
struct VmShell<'a>(&'a dyn WorkspaceRuntime);
impl RemoteShell for VmShell<'_> {
    fn run<'a>(
        &'a self,
        _: &'a SshDestination,
        command: String,
        input: Option<std::fs::File>,
    ) -> ShellFuture<'a> {
        self.0.run(command, input)
    }
}
/// The channel and allocation checks shared by agent and background sessions.
async fn open_workspace(
    service: &Workspaces,
    channels: &dyn ChannelStore,
    conversation: &ConversationId,
    provision: bool,
) -> Result<Arc<dyn WorkspaceRuntime>, LeaseRefusal> {
    let channel = channels
        .channel_of(conversation)
        .await
        .map_err(|_| LeaseRefusal::EnvironmentUnreachable)?;
    if let Some(id) = &channel {
        if channels
            .get(id)
            .await
            .map_err(|_| LeaseRefusal::EnvironmentUnreachable)?
            .is_none_or(|found| found.id() != id)
        {
            return Err(LeaseRefusal::EnvironmentUnreachable);
        }
    }
    service
        .open(conversation.clone(), provision, channel)
        .await
        .map_err(|error| {
            tracing::warn!(
                cause = error.cause().as_str(),
                ?error,
                "Boat workspace could not be prepared"
            );
            LeaseRefusal::EnvironmentUnreachable
        })
}
impl WorkspaceConnector for Connector {
    fn connect(
        &self,
        provision: bool,
    ) -> EnvironmentFuture<'_, Result<LeaseConnection, LeaseRefusal>> {
        Box::pin(async move {
            let runtime = open_workspace(
                self.service.as_ref(),
                self.channels.as_ref(),
                &self.conversation,
                provision,
            )
            .await?;
            let copy = self.copies.copy(runtime.clone(), provision).await?;
            let stream = runtime.serve(copy).await.map_err(|error| {
                tracing::warn!(?error, "Boat environment process could not be started");
                LeaseRefusal::EnvironmentUnreachable
            })?;
            Ok(LeaseConnection {
                mcp_relay: stream.mcp_relay,
                from_environment: stream.output,
                to_environment: stream.input,
                keep: stream.owner,
                artifacts: stream
                    .files
                    .map(ArtifactTransport::Channels)
                    .unwrap_or(ArtifactTransport::Frames),
                updates: None,
            })
        })
    }
}
impl ThreadWorkspaces {
    /// After each change to the Claude sign-in, let go of every conversation's
    /// connection that nothing uses, so its next lease is served again with
    /// the sign-in now saved (ADR 501): a serve keeps the environment it
    /// started with. One in use is asked again, as `rest` asks, and kept if it
    /// stays in use; it is served again once it is let go some other way.
    /// Holds them only while letting go, so it ends once they are dropped.
    pub async fn follow_claude_sign_ins(self: Arc<Self>) {
        let mut changes = self.service.claude_sign_in_changed_watch();
        let workspaces = Arc::downgrade(&self);
        drop(self);
        while changes.changed().await.is_ok() {
            let Some(workspaces) = workspaces.upgrade() else {
                return;
            };
            workspaces.let_go_all().await;
        }
    }

    async fn let_go_all(&self) {
        let environments: Vec<_> = self
            .environments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .values()
            .cloned()
            .collect();
        let_go_each(environments).await;
    }
}

/// Let go of each environment's connection once nothing uses it, asking
/// again as `rest` does; one still in use after that keeps its connection.
pub async fn let_go_each(environments: Vec<Arc<SshEnvironment>>) {
    for environment in environments {
        let mut wait = Duration::from_secs(1);
        while !environment.let_go().await {
            if wait > LET_GO_PATIENCE {
                tracing::warn!("a Boat connection is still in use; it stays open");
                break;
            }
            clock::sleep(wait).await;
            wait *= 2;
        }
    }
}
impl ThreadEnvironments for ThreadWorkspaces {
    fn environment(
        &self,
        conversation: &ConversationId,
    ) -> Result<Arc<dyn Environment>, ConversationError> {
        let mut environments = self
            .environments
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if let Some(environment) = environments.get(&conversation.to_string()) {
            return Ok(environment.clone());
        }
        if environments.len() >= MAX_RETAINED_ENVIRONMENTS {
            environments.retain(|_, environment| {
                Arc::strong_count(environment) != 1 || !environment.unowned()
            });
        }
        if environments.len() >= MAX_RETAINED_ENVIRONMENTS {
            return Err(ConversationError::Unavailable);
        }
        let environment = SshEnvironment::workspace(
            Arc::new(Connector {
                service: self.service.clone(),
                channels: self.channels.clone(),
                conversation: conversation.clone(),
                copies: VmCopies {
                    audit: self.audit.clone(),
                    installer: self.installer.clone(),
                    conversation: conversation.clone(),
                    choosing: Arc::default(),
                },
            }),
            self.audit.clone(),
        );
        let environment = Arc::new(match &self.logins {
            Some(logins) => environment.with_logins(logins.clone()),
            None => environment,
        });
        environments.insert(conversation.to_string(), environment.clone());
        Ok(environment)
    }
    fn available(&self) -> EnvironmentFuture<'_, bool> {
        Box::pin(async move { self.service.settings.read().await.is_ok() })
    }
    fn rest<'a>(
        &'a self,
        conversation: &'a ConversationId,
        cause: WorkspaceRest,
    ) -> EnvironmentFuture<'a, bool> {
        Box::pin(async move {
            let cause = match cause {
                WorkspaceRest::Idle => RestCause::Idle,
                WorkspaceRest::Closed => RestCause::Closed,
            };
            let environment = self
                .environments
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&conversation.to_string())
                .cloned();
            // Its connection goes first: while anything holds it the VM is
            // in use. What the stopped agent held is let go as its tasks
            // wind down, so it is asked again for half a minute; a thread
            // opened again meanwhile keeps it, and its own rest comes later.
            if let Some(environment) = environment {
                let mut wait = Duration::from_secs(1);
                while !environment.let_go().await {
                    if wait > LET_GO_PATIENCE {
                        tracing::warn!(conversation_id = %conversation, "the conversation's connection is still in use; its VM keeps running");
                        return false;
                    }
                    clock::sleep(wait).await;
                    wait *= 2;
                }
            }
            let rested = self.service.rest(conversation.clone(), cause).await;
            if rested.audit_failed {
                tracing::warn!(conversation_id = %conversation, "the record of the conversation's VM stop could not be kept");
            }
            match rested.stopped {
                Ok(stopped) => stopped,
                Err(error) => {
                    tracing::warn!(conversation_id = %conversation, ?error, "the conversation's VM could not be stopped; it keeps running until its own time limit");
                    false
                }
            }
        })
    }
}

#[cfg(all(test, unix))]
#[path = "../../tests/conversation/workspace_copies.rs"]
mod tests;

impl ThreadWorkspaces {
    /// Open only the task's existing VM. The returned stream retains InUse
    /// through the same HeldRuntime owner as agent streams.
    pub(super) async fn background(
        &self,
        conversation: ConversationId,
        workspace: &str,
        command: &BackgroundCommand,
        stop: &Receiver<Option<TaskStop>>,
    ) -> std::io::Result<Option<WorkspaceStream>> {
        let runtime = open_workspace(
            self.service.as_ref(),
            self.channels.as_ref(),
            &conversation,
            false,
        )
        .await
        .map_err(|_| std::io::Error::other("workspace unavailable"))?;
        background_on(
            self.installer.as_ref(),
            self.audit.as_ref(),
            runtime.as_ref(),
            &conversation,
            workspace,
            command,
            stop,
        )
        .await
    }
}

async fn background_on(
    installer: &HostInstaller,
    audit: &dyn EnvironmentAudit,
    runtime: &dyn WorkspaceRuntime,
    conversation: &ConversationId,
    workspace: &str,
    command: &BackgroundCommand,
    stop: &Receiver<Option<TaskStop>>,
) -> std::io::Result<Option<WorkspaceStream>> {
    if stop.borrow().is_some() {
        return Ok(None);
    }
    let host = SshDestination::new(format!("boat-{conversation}"))
        .map_err(|_| std::io::Error::other("invalid workspace identity"))?;
    installer
        .ensure_on(&VmShell(runtime), &host, &conversation.to_string(), audit)
        .await
        .map_err(|_| std::io::Error::other("background build unavailable"))?;
    let program = installer
        .background_command(workspace, command)
        .await
        .map_err(|_| std::io::Error::other("background command unavailable"))?;
    // Preparation may outlast the cancellation request. Keep its owner
    // until it returns, but never launch a command for a stopped task.
    if stop.borrow().is_some() {
        return Ok(None);
    }
    runtime.background(program).await.map(Some)
}
