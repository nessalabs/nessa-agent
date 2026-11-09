use std::sync::Arc;

use tauri::{State, WebviewWindow};

use super::{LoginFailure, Provider, ProviderLogin};
use crate::{composition::HostDependencies, desktop_window, panel};

async fn launch(login: Arc<dyn ProviderLogin>, provider: Provider) -> Result<(), LoginFailure> {
    tauri::async_runtime::spawn_blocking(move || login.open(provider))
        .await
        .map_err(|_| LoginFailure::LaunchFailed)?
}

/// Open the provider CLI's login without claiming authentication succeeded.
#[tauri::command]
pub async fn sign_in_to_provider(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    provider: Provider,
) -> Result<(), LoginFailure> {
    admit(window.label())?;
    launch(deps.provider_login.clone(), provider).await
}

/// Mint a Claude sign-in for Boat VMs. The token is answered to the window,
/// which hands it straight to the gateway's vault; the host keeps nothing.
#[tauri::command]
pub async fn mint_claude_sign_in(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<String, LoginFailure> {
    admit(window.label())?;
    mint(deps.provider_login.clone()).await
}

async fn mint(login: Arc<dyn ProviderLogin>) -> Result<String, LoginFailure> {
    tauri::async_runtime::spawn_blocking(move || login.mint_claude_sign_in())
        .await
        .map_err(|_| LoginFailure::LaunchFailed)?
}

/// Whether [`mint_claude_sign_in`] can be offered on this host.
#[tauri::command]
pub fn claude_sign_in_mint_available(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<bool, LoginFailure> {
    admit(window.label())?;
    Ok(deps.provider_login.mints_claude_sign_in())
}

/// Publish the injected launcher's capability before offering a login action.
#[tauri::command]
pub fn provider_login_available(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<bool, LoginFailure> {
    admit(window.label())?;
    Ok(deps.provider_login.available())
}

fn admit(label: &str) -> Result<(), LoginFailure> {
    if label == panel::MAIN_WINDOW || label == desktop_window::DESKTOP_WINDOW {
        Ok(())
    } else {
        Err(LoginFailure::UntrustedCaller)
    }
}

#[cfg(test)]
#[path = "../../tests/provider_authentication/commands.rs"]
mod tests;
