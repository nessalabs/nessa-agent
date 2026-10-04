//! Provider-owned login flows launched by the trusted desktop window.
//!
//! ```text
//! bundled window ──▶ sign_in_to_provider ──▶ ProviderLogin ──▶ native terminal
//! ```
//! The application port reports only whether the login was opened. Credentials
//! and the authentication result remain with the provider CLI, not this host.

use std::sync::Arc;

use serde::{Deserialize, Serialize};
use tauri::{State, WebviewWindow};

use crate::{composition::HostDependencies, desktop_window, panel};

mod native;
pub use native::NativeProviderLogin;

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Provider {
    Claude,
    Codex,
}

#[derive(Clone, Copy, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "kebab-case")]
pub enum LoginFailure {
    UntrustedCaller,
    Unavailable,
    LaunchFailed,
}

/// Outside-process login launch, supplied by host composition.
pub trait ProviderLogin: Send + Sync {
    /// Whether this host implements a provider login launcher.
    fn available(&self) -> bool;
    fn open(&self, provider: Provider) -> Result<(), LoginFailure>;
}

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
mod tests {
    use super::*;
    struct Refused;
    impl ProviderLogin for Refused {
        fn available(&self) -> bool {
            false
        }
        fn open(&self, _: Provider) -> Result<(), LoginFailure> {
            Err(LoginFailure::Unavailable)
        }
    }
    #[test]
    fn only_bundled_conversation_windows_may_launch_provider_login() {
        assert_eq!(admit(panel::MAIN_WINDOW), Ok(()));
        assert_eq!(admit(desktop_window::DESKTOP_WINDOW), Ok(()));
        assert_eq!(
            admit(panel::SETUP_WINDOW),
            Err(LoginFailure::UntrustedCaller)
        );
        assert_eq!(admit("external"), Err(LoginFailure::UntrustedCaller));
    }
    #[test]
    fn a_host_without_login_reports_unavailable() {
        assert!(!Refused.available());
        assert_eq!(
            tauri::async_runtime::block_on(launch(Arc::new(Refused), Provider::Claude)),
            Err(LoginFailure::Unavailable)
        );
    }
}
