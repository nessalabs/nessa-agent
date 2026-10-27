//! Workspace SDK prepares the workspace; the existing SSH lease protocol
//! owns continuous agent IO and process cleanup inside it.
use crate::workspaces::{
    application::{WorkspaceRuntime, WorkspaceStream},
    domain::Failure,
};
use async_trait::async_trait;
use std::sync::Arc;
pub(super) struct BoatRuntime {
    pub(super) access: Arc<super::ssh::SshAccess>,
}
#[async_trait]
impl WorkspaceRuntime for BoatRuntime {
    async fn serve(&self) -> Result<WorkspaceStream, Failure> {
        self.access.serve().map_err(|_| Failure::Provider)
    }
}
