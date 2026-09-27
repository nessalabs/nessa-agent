//! What deleting a session means for Codex.
//!
//! Codex's ACP adapter (`@agentclientprotocol/codex-acp` 1.12) advertises
//! `sessionCapabilities.delete`, but answers `session/delete` by archiving the
//! thread (`threadArchive`): the transcript stays in Codex's store. A
//! successful answer is therefore reported as
//! [`ProviderSessionDeletion::Archived`], never as erased. The exchange itself
//! is the shared ACP one.
use super::binding::CodexAcpProvider;
use crate::application::agent_execution::providers::{
    ProviderSessionDeleter, ProviderSessionDeletion, ProviderSessionDeletionFuture,
};
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::infrastructure::acp::sessions::deletion::{self as acp_deletion, AcpSessionDeletion};
use std::{future::Future, pin::Pin};

impl ProviderSessionDeleter for CodexAcpProvider {
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
                AcpSessionDeletion::Acknowledged => ProviderSessionDeletion::Archived,
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
