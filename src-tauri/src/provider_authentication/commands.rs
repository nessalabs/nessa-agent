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
