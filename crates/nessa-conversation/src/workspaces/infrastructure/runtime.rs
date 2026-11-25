//! The workspace SDK prepares the VM and its connection; a Cloud Box's
//! bootstrap runs its commands over it.
use crate::workspaces::application::WorkspaceRuntime;
use async_trait::async_trait;
use std::sync::Arc;

pub(super) struct BoatRuntime {
    pub(super) access: Arc<super::ssh::VmLink>,
}

#[async_trait]
impl WorkspaceRuntime for BoatRuntime {
    async fn run(&self, command: String, input: Option<std::fs::File>) -> std::io::Result<String> {
        self.access.run(command, input).await
    }
}
