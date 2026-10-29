use crate::{
    composition::HostDependencies,
    gateway::infrastructure::{desktop_startup_wait, GatewayReader},
    vault::application::{Command, Failure},
};
use tauri::{State, WebviewWindow};
#[tauri::command]
pub async fn vault_command(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    request: Command,
) -> Result<serde_json::Value, Failure> {
    let reader = GatewayReader::of_window(window.label()).map_err(|_| Failure::Forbidden)?;
    if let Some(gateway) = &deps.gateway {
        reader
            .ready(gateway, desktop_startup_wait())
            .await
            .map_err(|_| Failure::VaultUnavailable)?;
    }
    crate::vault::application::execute(
        deps.vault.clone(),
        request,
        std::time::Duration::from_secs(15),
    )
    .await
}
