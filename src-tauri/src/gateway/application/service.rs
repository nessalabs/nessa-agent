use super::{GatewayError, GatewayHost, LoginShellPath, ReconciledGateway};
use crate::gateway::domain::value_objects::SearchPath;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub struct Gateway {
    host: Arc<dyn GatewayHost>,
    login_shell: Arc<dyn LoginShellPath>,
    runtime: PathBuf,
    stage: String,
    reconciled_gateway: Arc<Mutex<Option<ReconciledGateway>>>,
    reconciliation: Arc<Mutex<()>>,
}
impl Gateway {
    pub fn bootstrap(
        host: Arc<dyn GatewayHost>,
        login_shell: Arc<dyn LoginShellPath>,
        runtime: PathBuf,
        stage: String,
    ) -> Self {
        Self {
            host,
            login_shell,
            runtime,
            stage,
            reconciled_gateway: Arc::new(Mutex::new(None)),
            reconciliation: Arc::new(Mutex::new(())),
        }
    }
    pub async fn wait_ready(&self) -> Result<(), GatewayError> {
        let host = self.host.clone();
        let login_shell = self.login_shell.clone();
        let runtime = self.runtime.clone();
        let stage = self.stage.clone();
        let reconciled_gateway = self.reconciled_gateway.clone();
        let reconciliation = self.reconciliation.clone();
        tauri::async_runtime::spawn_blocking(move || {
            let _reconciliation = reconciliation.lock().map_err(|_| {
                GatewayError::Registration("gateway reconciliation lock is unavailable".into())
            })?;
            *reconciled_gateway.lock().map_err(|_| {
                GatewayError::Registration("gateway reconciliation state is unavailable".into())
            })? = None;
            let agent_path = agent_path(login_shell.as_ref());
            let gateway = host.register(&runtime, &stage, agent_path.as_ref())?;
            *reconciled_gateway.lock().map_err(|_| {
                GatewayError::Registration("gateway reconciliation state is unavailable".into())
            })? = Some(gateway);
            Ok(())
        })
        .await
        .map_err(|error| GatewayError::Registration(error.to_string()))?
    }
    pub fn stop_agents(&self) -> Result<(), GatewayError> {
        let _reconciliation = self
            .reconciliation
            .lock()
            .map_err(|_| GatewayError::NotReconciled)?;
        let gateway = self
            .reconciled_gateway
            .lock()
            .map_err(|_| GatewayError::NotReconciled)?
            .clone()
            .ok_or(GatewayError::NotReconciled)?;
        self.host.stop_agents(&gateway)
    }
}
/// The search path the agent should be given this launch, if it can be had.
///
/// A login shell that hangs or fails is not a registration failure: the user
/// still gets their gateway, the agent still gets a working path, and the one
/// consequence — that the tools they installed are not on it — is said out loud
/// rather than left to be discovered as `command not found`.
fn agent_path(login_shell: &dyn LoginShellPath) -> Option<SearchPath> {
    match login_shell.resolve() {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!(
                "[nessa] Could not read the login shell's PATH ({error}); the agent keeps the path already registered for its service, or the system path if there is none"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/gateway/application.rs"]
mod tests;
