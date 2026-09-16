use super::{GatewayError, GatewayHost};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};
pub struct Gateway {
    host: Arc<dyn GatewayHost>,
    runtime: PathBuf,
    stage: String,
    reconciled_service: Mutex<Option<String>>,
}
impl Gateway {
    pub fn bootstrap(host: Arc<dyn GatewayHost>, runtime: PathBuf, stage: String) -> Self {
        Self {
            host,
            runtime,
            stage,
            reconciled_service: Mutex::new(None),
        }
    }
    pub async fn wait_ready(&self) -> Result<(), GatewayError> {
        let host = self.host.clone();
        let runtime = self.runtime.clone();
        let stage = self.stage.clone();
        let service = tauri::async_runtime::spawn_blocking(move || host.register(&runtime, &stage))
            .await
            .map_err(|error| GatewayError::Registration(error.to_string()))??;
        *self.reconciled_service.lock().map_err(|_| {
            GatewayError::Registration("gateway reconciliation state is unavailable".into())
        })? = Some(service);
        Ok(())
    }
    pub fn stop_agents(&self) -> Result<(), GatewayError> {
        let service = self
            .reconciled_service
            .lock()
            .map_err(|_| GatewayError::NotReconciled)?
            .clone()
            .ok_or(GatewayError::NotReconciled)?;
        self.host.stop_agents(&service)
    }
}
#[cfg(test)]
#[path = "../../../tests/gateway/application.rs"]
mod tests;
