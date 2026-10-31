//! Adapt per-thread workspace streams to the conversation environment contract.
use super::ssh_environment::{
    ArtifactTransport, EnvironmentAudit, HostInstaller, LeaseConnection, RemoteShell, ShellFuture,
    SshEnvironment, WorkspaceConnector,
};
use crate::{
    conversation::application::{
        ChannelStore, ConversationError, Environment, EnvironmentFuture, ThreadEnvironments,
        WorkspaceRest, MAX_RETAINED_ENVIRONMENTS,
    },
    env::LEASE_PROTOCOL,
    workspaces::application::{RestCause, ServedCopy, WorkspaceRuntime, Workspaces},
};
use futures_util::future::{BoxFuture, FutureExt, Shared};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::{LeaseRefusal, SshDestination};
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::Duration,
};

/// The longest wait between two asks to let a resting thread's connection
/// go: 1 + 2 + 4 + 8 + 16 seconds in all.
const LET_GO_PATIENCE: Duration = Duration::from_secs(16);

pub(crate) struct ThreadWorkspaces {
    pub(crate) service: Arc<Workspaces>,
    pub(crate) channels: Arc<dyn ChannelStore>,
    pub(crate) audit: Arc<dyn EnvironmentAudit>,
    /// Puts this build in a VM whose image's copy speaks another lease
    /// protocol, as on an SSH host.
    pub(crate) installer: Arc<HostInstaller>,
    pub(crate) environments: Mutex<BTreeMap<String, Arc<SshEnvironment>>>,
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
                        let copy = served_copy(
                            &installer,
                            audit.as_ref(),
                            &conversation,
                            runtime.as_ref(),
                            install,
                        )
                        .await;
                        *done
                            .lock()
                            .unwrap_or_else(std::sync::PoisonError::into_inner) = None;
                        copy
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
/// The copy of `nessa` in `conversation`'s VM that speaks this build's
/// lease protocol: the image's own when it does, else this build, installed
/// in the VM now by `installer` unless it is there already. A VM made from
/// an image an earlier build pinned keeps its files and gets this build
/// beside them. An account of a lease already held (`install` false) never
/// installs: it is served whichever copy is there. The installed copy, when
/// there is one, comes first ([`ServedCopy::Installed`]).
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
    // A VM that could not be asked is served what is installed there, else
    // its image's copy, as it would have been: nothing is installed on a
    // guess.
    if install && image.is_some() {
        // The VM is named by its thread in the install's records, and the
        // thread stands for the lease: they join the thread's lease records.
        let host = SshDestination::new(format!("boat-{conversation}"))
            .map_err(|_| LeaseRefusal::EnvironmentUnreachable)?;
        installer
            .ensure_on(&VmShell(runtime), &host, &conversation.to_string(), audit)
            .await?;
    }
    Ok(ServedCopy::Installed(installer.serve_command().await?))
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
impl WorkspaceConnector for Connector {
    fn connect(
        &self,
        provision: bool,
    ) -> EnvironmentFuture<'_, Result<LeaseConnection, LeaseRefusal>> {
        Box::pin(async move {
            let channel = self
                .channels
                .channel_of(&self.conversation)
                .await
                .map_err(|_| LeaseRefusal::EnvironmentUnreachable)?;
            if let Some(id) = &channel {
                if self
                    .channels
                    .get(id)
                    .await
                    .map_err(|_| LeaseRefusal::EnvironmentUnreachable)?
                    .is_none_or(|found| found.id() != id)
                {
                    return Err(LeaseRefusal::EnvironmentUnreachable);
                }
            }
            let runtime = self
                .service
                .open(self.conversation.clone(), provision, channel)
                .await
                .map_err(|error| {
                    tracing::warn!(?error, "Boat workspace could not be prepared");
                    LeaseRefusal::EnvironmentUnreachable
                })?;
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
            })
        })
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
        let environment = Arc::new(SshEnvironment::workspace(
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
        ));
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
                    tokio::time::sleep(wait).await;
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
#[path = "../../../tests/conversation/workspace_copies.rs"]
mod tests;
