use super::super::application::{GatewayHost, LoginShellPath};
use super::login_shell::LoginShell;
use std::sync::Arc;

/// Selects the native gateway adapter for the current build target.
pub fn current() -> Arc<dyn GatewayHost> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::macos::Launchd)
    }
    #[cfg(not(target_os = "macos"))]
    {
        Arc::new(super::unsupported::Unsupported)
    }
}

/// Selects how the agent's search path is read on the current build target.
///
/// Separate from [`current`] because it answers a different question: the
/// gateway host is how a service is registered, and this is what the account
/// running it has on its `PATH`. A target with no service to register can still
/// have a login shell, and one with no login shell still registers a service.
pub fn login_shell_path() -> Arc<dyn LoginShellPath> {
    Arc::new(LoginShell::for_current_user())
}
