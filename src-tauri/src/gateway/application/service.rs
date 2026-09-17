use super::{GatewayError, GatewayHost, ReconciledGateway};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub struct Gateway {
    host: Arc<dyn GatewayHost>,
    runtime: PathBuf,
    stage: String,
    reconciled_gateway: Arc<Mutex<Option<ReconciledGateway>>>,
    reconciliation: Arc<Mutex<()>>,
}
impl Gateway {
    pub fn bootstrap(host: Arc<dyn GatewayHost>, runtime: PathBuf, stage: String) -> Self {
        Self {
            host,
            runtime,
            stage,
            reconciled_gateway: Arc::new(Mutex::new(None)),
            reconciliation: Arc::new(Mutex::new(())),
        }
    }
    pub async fn wait_ready(&self) -> Result<(), GatewayError> {
        let host = self.host.clone();
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
            let gateway = host.register(&runtime, &stage)?;
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
#[cfg(test)]
#[path = "../../../tests/gateway/application.rs"]
mod tests;
