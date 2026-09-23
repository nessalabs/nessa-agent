use crate::{
    composition::HostDependencies, gateway::application::Gateway,
    gateway_endpoint::application::GatewayEndpointAccess, panel,
};
use std::sync::Arc;
use tauri::{State, WebviewWindow};

async fn load_for(
    label: &str,
    gateway: Option<&Gateway>,
    endpoint: Arc<GatewayEndpointAccess>,
    stage: &str,
) -> Result<Option<String>, String> {
    if label != panel::MAIN_WINDOW {
        return Err("Only the bundled chat surface can discover the gateway endpoint".into());
    }
    if let Some(gateway) = gateway {
        gateway
            .wait_ready()
            .await
            .map_err(|error| error.to_string())?;
    }
    let stage = stage.to_owned();
    tauri::async_runtime::spawn_blocking(move || endpoint.resolve(&stage))
        .await
        .map_err(|error| error.to_string())?
}

#[tauri::command]
pub async fn load_gateway_endpoint(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    stage: String,
) -> Result<Option<String>, String> {
    let gateway = deps.gateway.clone();
    let endpoint = Arc::clone(&deps.endpoint);
    load_for(window.label(), gateway.as_deref(), endpoint, &stage).await
}
