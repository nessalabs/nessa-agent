//! Adapt per-thread workspace streams to the conversation environment contract.
use super::ssh_environment::{
    ArtifactTransport, EnvironmentAudit, LeaseConnection, SshEnvironment, WorkspaceConnector,
};
use crate::{
    conversation::application::{
        ChannelStore, ConversationError, Environment, EnvironmentFuture, ThreadEnvironments,
        MAX_RETAINED_ENVIRONMENTS,
    },
    workspaces::application::Workspaces,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::LeaseRefusal;
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
    pub(crate) environments: Mutex<BTreeMap<String, Arc<SshEnvironment>>>,
}
struct Connector {
    service: Arc<Workspaces>,
    channels: Arc<dyn ChannelStore>,
    conversation: ConversationId,
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
            let stream = runtime.serve().await.map_err(|error| {
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
            }),
            self.audit.clone(),
        ));
        environments.insert(conversation.to_string(), environment.clone());
        Ok(environment)
    }
    fn available(&self) -> EnvironmentFuture<'_, bool> {
        Box::pin(async move { self.service.settings.read().await.is_ok() })
    }
    fn rest<'a>(&'a self, conversation: &'a ConversationId) -> EnvironmentFuture<'a, bool> {
        Box::pin(async move {
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
            match self.service.rest(conversation.clone()).await {
                Ok(stopped) => stopped,
                Err(error) => {
                    tracing::warn!(conversation_id = %conversation, ?error, "the conversation's VM could not be stopped; it keeps running until its own time limit");
                    false
                }
            }
        })
    }
}
