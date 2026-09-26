//! What deleting a session means for Claude.
//!
//! Claude's ACP adapter (`@agentclientprotocol/claude-agent-acp` 0.76)
//! advertises `sessionCapabilities.delete`, and on `session/delete` tears down
//! any live query for the session and then deletes the Claude Code session
//! file. A successful answer is therefore reported as
//! [`ProviderSessionDeletion::Deleted`]. The exchange itself is the shared ACP
//! one.
use super::binding::ClaudeAcpProvider;
use crate::application::agent_execution::providers::{
    ProviderSessionDeleter, ProviderSessionDeletion, ProviderSessionDeletionFuture,
};
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::infrastructure::acp::sessions::deletion::{self as acp_deletion, AcpSessionDeletion};
use std::{future::Future, pin::Pin};

impl ProviderSessionDeleter for ClaudeAcpProvider {
    fn delete_session(&self, session: ExecutionSessionId) -> ProviderSessionDeletionFuture<'_> {
        Box::pin(async move {
            let answer = acp_deletion::delete_session(
                self.process_factory(),
                self.config().clone(),
                &self.profile(),
                &session,
                self.deletions(),
            )
            .await?;
            Ok(match answer {
                AcpSessionDeletion::Acknowledged => ProviderSessionDeletion::Deleted,
                AcpSessionDeletion::NotListed => ProviderSessionDeletion::NotListed,
                AcpSessionDeletion::NotAdvertised => ProviderSessionDeletion::NotSupported,
            })
        })
    }
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.deletions().settled(self.config()))
    }
    fn cleanup_outstanding(&self) -> bool {
        self.deletions().outstanding()
    }
}
