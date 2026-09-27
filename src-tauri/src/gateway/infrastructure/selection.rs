use super::super::application::{
    GatewayHost, GatewayReconciliationAudit, GatewayReconciliationIds, LoginShellPath,
    MonotonicClock,
};
#[cfg(target_os = "linux")]
use super::linux::SystemdGateway;
#[cfg(target_os = "macos")]
use super::macos::{LaunchctlDisabledServiceStatus, Launchd, NativeLaunchctl};
#[cfg(any(target_os = "macos", target_os = "linux"))]
use super::reconciliation_audit::FileReconciliationAudit;
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
use super::unsupported::{Unsupported, UnsupportedAudit};
use super::{login_shell::LoginShell, reconciliation_ids::RandomReconciliationIds};
use crate::gateway::domain::value_objects::ServiceConfiguration;
#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::gateway::domain::value_objects::ServiceManager;
use std::{io, path::PathBuf, sync::Arc};

/// One composition-time snapshot of the account paths used by the gateway.
pub struct PlatformContext {
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    home: PathBuf,
    #[cfg(target_os = "linux")]
    config_home: PathBuf,
    #[cfg(target_os = "linux")]
    data_home: PathBuf,
    #[cfg(target_os = "linux")]
    state_home: PathBuf,
}

pub fn platform_context(home: PathBuf) -> io::Result<PlatformContext> {
    #[cfg(target_os = "linux")]
    {
        let config_home = resolved_xdg_root("XDG_CONFIG_HOME", &home, ".config")?;
        let data_home = resolved_xdg_root("XDG_DATA_HOME", &home, ".local/share")?;
        let state_home = resolved_xdg_root("XDG_STATE_HOME", &home, ".local/state")?;
        Ok(PlatformContext {
            home,
            config_home,
            data_home,
            state_home,
        })
    }
    #[cfg(target_os = "macos")]
    {
        Ok(PlatformContext { home })
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = home;
        Ok(PlatformContext {})
    }
}

#[cfg(target_os = "linux")]
fn resolved_xdg_root(name: &str, home: &std::path::Path, fallback: &str) -> io::Result<PathBuf> {
    let value = std::env::var_os(name)
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(fallback));
    if value.is_absolute()
        && value.components().all(|component| {
            !matches!(
                component,
                std::path::Component::CurDir | std::path::Component::ParentDir
            )
        })
    {
        Ok(value)
    } else {
        Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            format!("{name} must be absolute and normalized"),
        ))
    }
}

/// Selects the native gateway adapter for the current build target.
pub fn current(
    configuration: ServiceConfiguration,
    platform: &PlatformContext,
    clock: Arc<dyn MonotonicClock>,
) -> Arc<dyn GatewayHost> {
    #[cfg(target_os = "macos")]
    {
        let _ = clock;
        Arc::new(Launchd::new(
            Arc::new(LaunchctlDisabledServiceStatus),
            Arc::new(NativeLaunchctl),
            configuration,
            platform.home.clone(),
        ))
    }
    #[cfg(target_os = "linux")]
    {
        Arc::new(SystemdGateway::new(
            configuration,
            platform.home.clone(),
            (
                Some(platform.config_home.clone().into_os_string()),
                Some(platform.data_home.clone().into_os_string()),
                Some(platform.state_home.clone().into_os_string()),
            ),
            clock,
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (configuration, platform, clock);
        Arc::new(Unsupported)
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
    Arc::new(RandomReconciliationIds)
}

pub fn reconciliation_audit(
    config_root: Option<PathBuf>,
    platform: &PlatformContext,
    clock: Arc<dyn MonotonicClock>,
) -> Arc<dyn GatewayReconciliationAudit> {
    #[cfg(target_os = "macos")]
    {
        let _ = platform;
        Arc::new(FileReconciliationAudit::new(
            config_root,
            ServiceManager::Launchd,
            clock,
        ))
    }
    #[cfg(target_os = "linux")]
    {
        let _ = config_root;
        Arc::new(FileReconciliationAudit::new(
            Some(platform.state_home.join("nessa")),
            ServiceManager::Systemd,
            clock,
        ))
    }
    #[cfg(not(any(target_os = "macos", target_os = "linux")))]
    {
        let _ = (config_root, platform, clock);
        Arc::new(UnsupportedAudit)
    }
}
