use super::super::application::{
    GatewayHost, GatewayReconciliationAudit, GatewayReconciliationIds, LoginShellPath,
};
use super::login_shell::LoginShell;
use std::{path::PathBuf, sync::Arc};

/// Selects the native gateway adapter for the current build target.
pub fn current() -> Arc<dyn GatewayHost> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::macos::Launchd::new(Arc::new(
            super::macos::LaunchctlDisabledServiceStatus,
        )))
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

pub fn reconciliation_ids() -> Arc<dyn GatewayReconciliationIds> {
    Arc::new(super::reconciliation_ids::RandomReconciliationIds)
}

pub fn reconciliation_audit(config_root: Option<PathBuf>) -> Arc<dyn GatewayReconciliationAudit> {
    #[cfg(target_os = "macos")]
    {
        Arc::new(super::macos::FileReconciliationAudit::new(config_root))
    }
    #[cfg(not(target_os = "macos"))]
    {
        let _ = config_root;
        Arc::new(super::unsupported::UnsupportedAudit)
    }
}
