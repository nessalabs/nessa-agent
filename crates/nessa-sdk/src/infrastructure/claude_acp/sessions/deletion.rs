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

#[cfg(all(test, unix))]
mod tests {
    use super::super::profile::ClaudeProfile;
    use crate::application::agent_execution::agents::AgentError;
    use crate::application::agent_execution::providers::ExecutableUseSnapshot;
    use crate::domain::agent_execution::permissions::PermissionOfferPolicy;
    use crate::infrastructure::acp::sessions::{
        deletion::{self as acp_deletion, DeletionCleanups},
        AcpConfig,
    };
    use std::{
        collections::BTreeMap,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc,
        },
        time::Duration,
    };

    /// ADR 221: a deletion is counted before its process is launched, so there
    /// is no moment with a process and nothing outstanding.
    #[tokio::test]
    async fn a_deletion_is_counted_before_its_process_is_launched() {
        let root = tempfile::tempdir().unwrap();
        let config = AcpConfig {
            executable: ExecutableUseSnapshot::unmanaged(root.path().join("unused-test-runtime")),
            arguments: Vec::new(),
            environment: BTreeMap::new(),
            credential_environment: BTreeMap::new(),
            workspace: root.path().to_owned(),
            tools_enabled: false,
            mcp_servers: Vec::new(),
            permissions: PermissionOfferPolicy::once_only(),
            launch_timeout: Duration::from_secs(1),
            startup_timeout: Duration::from_secs(1),
            execution_timeout: None,
            shutdown_grace: Duration::from_millis(10),
            kill_timeout: Duration::from_secs(1),
            event_capacity: 1,
            max_frame_bytes: 1024,
            max_incoming_frame_bytes: 1024,
            images: None,
            clock: Arc::new(crate::infrastructure::clock::RuntimeClock::new()),
        };
        let cleanups = DeletionCleanups::default();
        let counted_at_launch = Arc::new(AtomicBool::new(false));
        let factory = {
            let cleanups = cleanups.clone();
            let counted_at_launch = counted_at_launch.clone();
            Arc::new(move || {
                counted_at_launch.store(cleanups.outstanding(), Ordering::SeqCst);
                Err(AgentError::Transport("no process in this test".into()).into())
            }) as crate::infrastructure::acp::sessions::binding::ProcessFactory
        };
        let session =
            crate::domain::agent_execution::sessions::ExecutionSessionId::new("session-1").unwrap();
        let answer = acp_deletion::delete_session(
            factory,
            config,
            &ClaudeProfile::new(None),
            &session,
            &cleanups,
        )
        .await;
        assert!(answer.is_err());
        assert!(
            counted_at_launch.load(Ordering::SeqCst),
            "counted before launch"
        );
        assert!(
            !cleanups.outstanding(),
            "nothing retained, nothing outstanding"
        );
    }
}
