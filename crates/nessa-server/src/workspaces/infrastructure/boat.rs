//! Boat VM allocation plus an attached workspace-sdk backend. Nessa owns VM
//! creation/deletion; the SDK owns supervisor runs and files inside that VM.
use crate::workspaces::{
    application::{Provider, WorkspaceRuntime},
    domain::{BoatConnection, Failure},
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use workspace_sdk::{
    boat_client::{self, Api, ClientConfig, CreateOptions, CreateSandboxRequest, SandboxState},
    infrastructure::MemoryStateRepository,
    providers::{SandboxOpenOptions, SandboxProvider, SupervisorDeployment},
    OpenRequest, ProviderId, WorkspaceId, WorkspaceSdk,
};

const VM_TTL_SECONDS: u32 = 3600;

pub(crate) struct BoatProvider {
    pub(crate) relay_socket: std::path::PathBuf,
    pub(crate) relay_program: std::path::PathBuf,
}
pub(crate) fn api(connection: &BoatConnection) -> Result<Api, Failure> {
    let token =
        std::str::from_utf8(connection.token.as_slice()).map_err(|_| Failure::InvalidInput)?;
    let config = ClientConfig::new(token)
        .map_err(|_| Failure::InvalidInput)?
        .with_connect_timeout(Duration::from_secs(10))
        .with_request_timeout(Duration::from_secs(45));
    Api::new(config).map_err(|_| Failure::Provider)
}
#[async_trait]
impl Provider for BoatProvider {
    async fn validate(&self, connection: &BoatConnection) -> Result<(), Failure> {
        let api = api(connection)?;
        api.me().await.map_err(|_| Failure::Provider)?;
        if !connection.image.starts_with("bsh_") {
            api.get_named_snapshot(&connection.image)
                .await
                .map_err(|_| Failure::Missing)?;
        }
        let mut environments = api.environments().await.map_err(|_| Failure::Provider)?;
        if !environments
            .environments
            .iter()
            .any(|env| env.name == connection.environment)
        {
            environments = api
                .create_environment(boat_client::CreateSandboxEnvironmentRequest {
                    name: connection.environment.clone(),
                })
                .await
                .map_err(|_| Failure::Provider)?;
            let created = environments
                .environments
                .iter()
                .find(|env| env.name == connection.environment)
                .ok_or(Failure::Provider)?;
            api.update_environment(
                &created.id,
                boat_client::UpdateSandboxEnvironmentRequest {
                    safe_for_third_parties: Some(false),
                    pass_agents_credentials: Some(true),
                    pass_github: Some(false),
                    pass_secrets: Some(false),
                    pass_sandbox_credentials: Some(false),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| Failure::Provider)?;
            environments = api.environments().await.map_err(|_| Failure::Provider)?;
        }
        let environment = environments
            .environments
            .iter()
            .find(|env| env.name == connection.environment)
            .ok_or(Failure::Missing)?;
        if environment.safe_for_third_parties || !environment.pass_agents_credentials {
            return Err(Failure::EnvironmentUnavailable);
        }

        Ok(())
    }
    async fn create(&self, connection: &BoatConnection, thread: &str) -> Result<String, Failure> {
        let result = api(connection)?
            .create(
                Some(CreateSandboxRequest {
                    type_: Some("small".into()),
                    ttl_seconds: Some(Some(VM_TTL_SECONDS)),
                    environment: Some(connection.environment.clone()),
                    env: Some(std::collections::HashMap::from([(
                        "NESSA_THREAD_ID".into(),
                        thread.into(),
                    )])),
                    from: Some(connection.image.clone()),
                    snapshots: Some(true),
                    ..Default::default()
                }),
                &CreateOptions {
                    idempotency_key: Some(format!("nessa-thread-{thread}")),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| Failure::ProvisioningUncertain)?;
        let id = result.sandbox.id;
        if id.is_empty()
            || id.len() > 128
            || !id
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'-'))
        {
            return Err(Failure::ProvisioningUncertain);
        }
        Ok(id)
    }
    async fn connect(
        &self,
        connection: &BoatConnection,
        thread: &str,
        vm: &str,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let api = api(connection)?;
        let found = api.get(vm).await.map_err(|_| Failure::Missing)?;
        if found.sandbox.state == SandboxState::Archived {
            api.resume(
                vm,
                Some(boat_client::ResumeRequest {
                    ttl_seconds: Some(Some(VM_TTL_SECONDS)),
                    ..Default::default()
                }),
            )
            .await
            .map_err(|_| Failure::Provider)?;
        }
        boat_client::wait_until_ready_with(
            &api,
            vm,
            boat_client::WaitOptions::from_millis(90000, 1000),
        )
        .await
        .map_err(|_| Failure::Provider)?;
        let provider =
            SandboxProvider::new(api.clone()).with_supervisor(SupervisorDeployment::Preinstalled {
                executable: "/opt/nessa/bin/workspace-supervisor".into(),
            });
        let sdk = WorkspaceSdk::builder(Arc::new(MemoryStateRepository::default()))
            .provider(Arc::new(provider))
            .map_err(|_| Failure::Provider)?
            .build()
            .map_err(|_| Failure::Provider)?;
        let mut options = SandboxOpenOptions::new("/home/user/.nessa/workspaces");
        options.sandbox_id = Some(vm.to_owned());
        let request = OpenRequest::new(
            WorkspaceId::new(thread).map_err(|_| Failure::InvalidInput)?,
            &options,
        )
        .map_err(|_| Failure::Provider)?;
        let workspace = sdk
            .open(
                &ProviderId::new("boat").map_err(|_| Failure::Provider)?,
                request,
            )
            .await
            .map_err(|_| Failure::Provider)?;
        let root = workspace.descriptor().root;
        let config = serde_json::json!({"envServe":{"commands":true,"artifactsIn":true},"agents":{"catalog":"/opt/nessa/models.json","workspace":root,"selected":"codex","runtimes":{
            "codex":{"command":"/opt/nessa/bin/node","args":["/opt/nessa/harnesses/node_modules/@agentclientprotocol/codex-acp/dist/index.js"],"model":"gpt-5.6-terra","toolsEnabled":true},
            "claude":{"command":"/opt/nessa/bin/node","args":["/opt/nessa/harnesses/node_modules/@agentclientprotocol/claude-agent-acp/dist/index.js"],"model":"claude-sonnet-5","toolsEnabled":true}
        }}});
        boat_client::write_text(
            &api,
            vm,
            "/home/user/.nessa/config.json",
            &config.to_string(),
        )
        .await
        .map_err(|_| Failure::Provider)?;
        let secured = api
            .exec(
                vm,
                "chmod 700 /home/user/.nessa && chmod 600 /home/user/.nessa/config.json",
            )
            .await
            .map_err(|_| Failure::Provider)?;
        if !secured.success || secured.exit_code != Some(0) {
            return Err(Failure::Provider);
        }
        let access = super::ssh::SshAccess::connect(
            &api,
            vm,
            self.relay_socket.clone(),
            self.relay_program.clone(),
        )
        .await?;
        Ok(Arc::new(super::runtime::BoatRuntime { access }))
    }
    async fn remove(&self, connection: &BoatConnection, vm: &str) -> Result<(), Failure> {
        api(connection)?
            .delete_sandbox(vm)
            .await
            .map_err(|_| Failure::Provider)?;
        // Provider acknowledgement is not confirmed erasure. Keep the uncertain
        // intent rather than offering a duplicate replacement after a failed save.
        Err(Failure::CleanupUncertain)
    }
}
