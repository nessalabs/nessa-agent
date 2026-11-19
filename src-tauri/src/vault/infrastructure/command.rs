use crate::{
    composition::HostDependencies,
    gateway::infrastructure::{desktop_startup_wait, GatewayReader},
    vault::application::{
        biometrics::{execute_biometric, settle_biometric, BiometricCommand, BiometricRefusal},
        Command, Failure,
    },
};
use tauri::{State, WebviewWindow};
const VAULT_DEADLINE: std::time::Duration = std::time::Duration::from_secs(15);
#[tauri::command]
pub async fn vault_command(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    request: Command,
) -> Result<serde_json::Value, Failure> {
    ready(&window, &deps).await?;
    let operation = request.operation;
    let remember = request.remember;
    let passphrase = request.passphrase.clone();
    let reply =
        crate::vault::application::execute(deps.vault.clone(), request, VAULT_DEADLINE).await?;
    settle_biometric(
        deps.biometrics.clone(),
        &operation,
        remember,
        passphrase,
        &reply,
    )
    .await;
    Ok(reply)
}
/// Fingerprint unlock for the same trusted windows and the same vault child.
#[tauri::command]
pub async fn vault_biometrics(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    request: BiometricCommand,
) -> Result<serde_json::Value, BiometricRefusal> {
    ready(&window, &deps)
        .await
        .map_err(BiometricRefusal::Vault)?;
    execute_biometric(
        deps.vault.clone(),
        deps.biometrics.clone(),
        request,
        VAULT_DEADLINE,
    )
    .await
}
async fn ready(window: &WebviewWindow, deps: &HostDependencies) -> Result<(), Failure> {
    let reader = GatewayReader::of_window(window.label()).map_err(|_| Failure::Forbidden)?;
    if let Some(gateway) = &deps.gateway {
        reader
            .ready(gateway, desktop_startup_wait())
            .await
            .map_err(|_| Failure::VaultUnavailable)?;
    }
    Ok(())
}
