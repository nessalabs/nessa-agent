//! What deleting a session means for Opencode.
//!
//! Whether Opencode offers deleting a session is whatever its `initialize`
//! advertises; this binding does not know what it does on a successful
//! `session/delete`, so such an answer is reported as
//! [`ProviderSessionDeletion::Acknowledged`], claiming nothing about the
//! record. The exchange itself is the shared ACP one.
use super::binding::OpencodeAcpProvider;
use crate::application::agent_execution::providers::{
    ProviderSessionDeleter, ProviderSessionDeletion, ProviderSessionDeletionFuture,
};
use crate::domain::agent_execution::sessions::ExecutionSessionId;
use crate::infrastructure::acp::sessions::deletion::{self as acp_deletion, AcpSessionDeletion};
use std::{future::Future, pin::Pin};

impl ProviderSessionDeleter for OpencodeAcpProvider {
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
                AcpSessionDeletion::Acknowledged => ProviderSessionDeletion::Acknowledged,
                AcpSessionDeletion::NotListed => ProviderSessionDeletion::NotListed,
                AcpSessionDeletion::NotAdvertised => ProviderSessionDeletion::NotSupported,
            })
        })
    }
    fn settled(&self) -> Pin<Box<dyn Future<Output = ()> + Send + '_>> {
        Box::pin(self.deletions().settled(self.config()))
    }
}
