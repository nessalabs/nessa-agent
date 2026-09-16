use super::{GatewayError, GatewayHost};
use std::{path::PathBuf, sync::Arc};
pub struct Gateway {
    host: Arc<dyn GatewayHost>,
    runtime: PathBuf,
    stage: String,
}
impl Gateway {
    pub fn bootstrap(host: Arc<dyn GatewayHost>, runtime: PathBuf, stage: String) -> Self {
        Self {
            host,
            runtime,
            stage,
        }
    }
    pub async fn wait_ready(&self) -> Result<(), GatewayError> {
        let host = self.host.clone();
        let runtime = self.runtime.clone();
        let stage = self.stage.clone();
        tauri::async_runtime::spawn_blocking(move || host.register(&runtime, &stage))
            .await
            .map_err(|error| GatewayError::Registration(error.to_string()))??;
        Ok(())
    }
    pub fn stop_agents(&self) -> Result<(), GatewayError> {
        self.host
            .stop_agents(&self.host.service_identity(&self.stage)?)
    }
}
#[cfg(test)]
#[path = "../../../tests/gateway/application.rs"]
mod tests;
