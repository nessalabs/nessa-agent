//! Boat VM allocation plus an attached workspace-sdk backend. Nessa owns VM
//! creation/deletion; the SDK owns supervisor runs and files inside that VM.
use crate::workspaces::{
    application::{Provider, WorkspaceReceipt, WorkspaceRuntime},
    domain::{BoatConnection, EnvironmentIdentity, Failure},
};
use async_trait::async_trait;
use std::{sync::Arc, time::Duration};
use workspace_sdk::{
    boat_client::{self, Api, ClientConfig, SandboxState},
    infrastructure::MemoryStateRepository,
    providers::{SandboxOpenOptions, SandboxProvider, SupervisorDeployment},
    OpenRequest, ProviderId, WorkspaceId, WorkspaceSdk,
};

/// How long an opening waits for a VM that is stopping to have stopped.
/// With the 90 s wait for it to be ready after its resume, the opening stays
/// within the workspace connect budget (`agent-startup-budgets.json`,
/// 120 s); a stop slower than this fails the opening as unreachable.
const STOP_WAIT: Duration = Duration::from_secs(25);

pub(crate) struct BoatProvider {
    pub(crate) http: reqwest::Client,
    pub(crate) api_base: String,
    pub(crate) relay_socket: std::path::PathBuf,
    pub(crate) relay_program: std::path::PathBuf,
}
fn api(connection: &BoatConnection, base: &str) -> Result<Api, Failure> {
    let token =
        std::str::from_utf8(connection.token.as_slice()).map_err(|_| Failure::InvalidInput)?;
    let _ = rustls::crypto::ring::default_provider().install_default();
    let mut config = ClientConfig::new(token)
        .map_err(|_| Failure::InvalidInput)?
        .with_base_path(base)
        .map_err(|_| Failure::InvalidInput)?
        .with_connect_timeout(Duration::from_secs(10))
        .with_request_timeout(Duration::from_secs(45));
    config.org = Some("personal".into());
    config.max_response_bytes = 4 * 1024 * 1024;
    Api::new(config).map_err(|_| Failure::Provider)
}
#[async_trait]
impl Provider for BoatProvider {
    async fn validate(&self, connection: &BoatConnection) -> Result<(), Failure> {
        let api = api(connection, &self.api_base)?;
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
                    env_contents: Some(String::new()),
                    secret_files: Some(vec![]),
                    repositories: Some(vec![]),
                    credential_profile_id: Some(None),
                    proxy_ai_credentials: Some(false),
                    ..Default::default()
                },
            )
            .await
            .map_err(|_| Failure::Provider)?;
        }
        super::boat_admission::environment(&api, connection).await?;
        Ok(())
    }
    async fn create(
        &self,
        connection: &BoatConnection,
        thread: &str,
    ) -> Result<WorkspaceReceipt, Failure> {
        let api = api(connection, &self.api_base)?;
        let admission = super::boat_admission::environment(&api, connection).await?;
        super::boat_admission::create(
            &self.http,
            &api,
            connection,
            thread,
            admission,
            connection.auto_stop.seconds(),
        )
        .await
    }
    async fn connect(
        &self,
        connection: &BoatConnection,
        thread: &str,
        vm: &str,
        admission: &EnvironmentIdentity,
    ) -> Result<Arc<dyn WorkspaceRuntime>, Failure> {
        let api = api(connection, &self.api_base)?;
        let mut found =
            super::boat_admission::check(&self.http, &api, connection, vm, admission, None).await?;
        // A VM still stopping from its thread's last rest is resumed once it
        // has stopped.
        let stopping_until = tokio::time::Instant::now() + STOP_WAIT;
        while found.sandbox.state == SandboxState::Archiving {
            if tokio::time::Instant::now() >= stopping_until {
                return Err(Failure::Provider);
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
            found = super::boat_admission::check(&self.http, &api, connection, vm, admission, None)
                .await?;
        }
        let resumed = found.sandbox.state == SandboxState::Archived;
        if resumed {
            api.resume(
                vm,
                Some(boat_client::ResumeRequest {
                    ttl_seconds: Some(connection.auto_stop.seconds()),
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
        let maximum_ttl = if resumed {
            connection.auto_stop.seconds()
        } else {
            None
        };
        super::boat_admission::check(&self.http, &api, connection, vm, admission, maximum_ttl)
            .await?;
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
            root.clone(),
        )
        .await?;
        Ok(Arc::new(super::runtime::BoatRuntime { access }))
    }
    async fn stop(&self, connection: &BoatConnection, vm: &str) -> Result<(), Failure> {
        // Boat archives it: its disk is kept as a snapshot, nothing runs or
        // is billed, and `connect` resumes it.
        api(connection, &self.api_base)?
            .stop(vm, None)
            .await
            .map_err(|_| Failure::Provider)?;
        Ok(())
    }
    async fn remove(&self, connection: &BoatConnection, vm: &str) -> Result<(), Failure> {
        api(connection, &self.api_base)?
            .delete_sandbox(vm)
            .await
            .map_err(|_| Failure::Provider)?;
        // Provider acknowledgement is not confirmed erasure. Keep the uncertain
        // intent rather than offering a duplicate replacement after a failed save.
        Err(Failure::CleanupUncertain)
    }
}
