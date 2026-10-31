//! Workspace SDK prepares the workspace; the existing SSH lease protocol
//! owns continuous agent IO and process cleanup inside it.
use crate::workspaces::{
    application::{ServedCopy, WorkspaceRuntime, WorkspaceStream},
    domain::Failure,
};
use async_trait::async_trait;
use std::sync::Arc;
pub(super) struct BoatRuntime {
    pub(super) access: Arc<super::ssh::SshAccess>,
}
#[async_trait]
impl WorkspaceRuntime for BoatRuntime {
    async fn serve(&self, copy: ServedCopy) -> Result<WorkspaceStream, Failure> {
        self.access.serve(&copy).map_err(|_| Failure::Provider)
    }
    async fn image_protocol(&self) -> Option<String> {
        self.access.image_protocol().await
    }
    async fn run(&self, command: String, input: Option<std::fs::File>) -> std::io::Result<String> {
        self.access.run(command, input).await
    }
}
