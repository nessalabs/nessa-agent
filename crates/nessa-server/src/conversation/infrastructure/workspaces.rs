//! Adapt per-thread workspace streams to the conversation environment contract.
use super::ssh_environment::{
    ArtifactTransport, EnvironmentAudit, LeaseConnection, SshEnvironment, WorkspaceConnector,
};
use crate::{
    conversation::application::{
        ConversationError, Environment, EnvironmentFuture, ThreadEnvironments,
        MAX_RETAINED_ENVIRONMENTS,
    },
    workspaces::application::Workspaces,
};
use nessa_protocol::conversation::domain::ConversationId;
use nessa_sdk::domain::agent_execution::leases::LeaseRefusal;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
};

pub(crate) struct ThreadWorkspaces {
    pub(crate) service: Arc<Workspaces>,
    pub(crate) audit: Arc<dyn EnvironmentAudit>,
    pub(crate) environments: Mutex<BTreeMap<String, Arc<SshEnvironment>>>,
}
struct Connector {
    service: Arc<Workspaces>,
    conversation: ConversationId,
}
impl WorkspaceConnector for Connector {
    fn connect(
        &self,
        provision: bool,
    ) -> EnvironmentFuture<'_, Result<LeaseConnection, LeaseRefusal>> {
        Box::pin(async move {
            let runtime = self
                .service
                .open(self.conversation.clone(), provision)
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
                conversation: conversation.clone(),
            }),
            self.audit.clone(),
        ));
        environments.insert(conversation.to_string(), environment.clone());
        Ok(environment)
    }
}
