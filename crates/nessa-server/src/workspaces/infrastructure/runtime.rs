//! Workspace SDK prepares the workspace; the existing SSH lease protocol
//! owns continuous agent IO and process cleanup inside it.
use crate::workspaces::{
    application::{ClaudeSignIns, ServedCopy, WorkspaceRuntime, WorkspaceStream},
    domain::{ClaudeSignIn, Failure},
};
use async_trait::async_trait;
use std::sync::Arc;
pub(super) struct BoatRuntime {
    pub(super) access: Arc<super::ssh::SshAccess>,
    pub(super) claude_sign_ins: Arc<dyn ClaudeSignIns>,
}
#[async_trait]
impl WorkspaceRuntime for BoatRuntime {
    async fn serve(&self, copy: ServedCopy) -> Result<WorkspaceStream, Failure> {
        let sign_in = claude_sign_in(self.claude_sign_ins.as_ref()).await;
        self.access
            .serve(&copy, sign_in.as_ref())
            .await
            .map_err(|_| Failure::Provider)
    }
    async fn image_protocol(&self) -> Option<String> {
        self.access.image_protocol().await
    }
    async fn run(&self, command: String, input: Option<std::fs::File>) -> std::io::Result<String> {
        self.access.run(command, input).await
    }
}
/// The saved sign-in, or none. One that cannot be read is served as none:
/// Claude then refuses, and its card asks for a sign-in again.
pub(super) async fn claude_sign_in(sign_ins: &dyn ClaudeSignIns) -> Option<ClaudeSignIn> {
    match sign_ins.read().await {
        Ok(sign_in) => sign_in,
        Err(Failure::Locked) => None,
        Err(failure) => {
            tracing::warn!(
                failure = failure.kind(),
                "Claude sign-in for Boat unreadable"
            );
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use async_trait::async_trait;

    struct Answer(Result<Option<ClaudeSignIn>, Failure>);
    #[async_trait]
    impl ClaudeSignIns for Answer {
        async fn read(&self) -> Result<Option<ClaudeSignIn>, Failure> {
            self.0.clone()
        }
        async fn save(&self, _: ClaudeSignIn) -> Result<(), Failure> {
            unreachable!("a serve never saves")
        }
        async fn clear(&self) -> Result<(), Failure> {
            unreachable!("a serve never clears")
        }
    }

    #[tokio::test]
    async fn a_serve_carries_the_saved_sign_in_and_none_it_cannot_read() {
        let saved = ClaudeSignIn::new("sk-ant-oat01-saved".into()).unwrap();
        assert_eq!(
            claude_sign_in(&Answer(Ok(Some(saved))))
                .await
                .unwrap()
                .as_bytes(),
            b"sk-ant-oat01-saved"
        );
        for unread in [Ok(None), Err(Failure::Locked), Err(Failure::Storage)] {
            assert!(claude_sign_in(&Answer(unread)).await.is_none());
        }
    }
}
