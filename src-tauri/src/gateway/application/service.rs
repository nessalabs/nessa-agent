use super::{GatewayError, GatewayHost, LoginShellPath, ReconciledGateway};
use crate::gateway::domain::value_objects::SearchPath;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex, OnceLock},
};
pub struct Gateway {
    host: Arc<dyn GatewayHost>,
    login_shell: Arc<dyn LoginShellPath>,
    /// The agent's search path, resolved at most once for the life of this
    /// host process — the outcome, so a shell that could not be read is
    /// remembered as such rather than asked again.
    ///
    /// `wait_ready` runs on every webview load, not once per launch, and both
    /// consequences of resolving there matter. A login shell would be spawned,
    /// with its whole deadline available to it, in front of every panel. Worse,
    /// a profile edited while Nessa is open would resolve differently on the
    /// next load, and a service definition that differs is one that retires the
    /// running gateway and stops its agents — mid-session, with nobody having
    /// asked. The path is fixed when it is first needed; a change to it takes
    /// effect on the next launch of the app, which is when the re-registration
    /// it causes is something the user is expecting.
    agent_path: Arc<OnceLock<Option<SearchPath>>>,
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
            agent_path: Arc::new(OnceLock::new()),
            runtime,
            stage,
            reconciled_gateway: Arc::new(Mutex::new(None)),
            reconciliation: Arc::new(Mutex::new(())),
        }
    }
    pub async fn wait_ready(&self) -> Result<(), GatewayError> {
        let host = self.host.clone();
        let login_shell = self.login_shell.clone();
        let agent_path = self.agent_path.clone();
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
            let agent_path = agent_path.get_or_init(|| resolve_agent_path(login_shell.as_ref()));
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
/// The search path the agent should be given, if it can be had. Called once per
/// host process; see [`Gateway::agent_path`].
///
/// A login shell that hangs or fails is not a registration failure: the user
/// still gets their gateway, the agent still gets a working path, and the one
/// consequence — that the tools they installed are not on it — is said out loud
/// rather than left to be discovered as `command not found`.
fn resolve_agent_path(login_shell: &dyn LoginShellPath) -> Option<SearchPath> {
    match login_shell.resolve() {
        Ok(path) => Some(path),
        Err(error) => {
            eprintln!(
                "[nessa] Could not read the login shell's PATH ({error}); the agent keeps the path already registered for its service, or the system path if there is none, until Nessa is launched again"
            );
            None
        }
    }
}

#[cfg(test)]
#[path = "../../../tests/gateway/application.rs"]
mod tests;
