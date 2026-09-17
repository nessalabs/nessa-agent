use super::super::application::GatewayHost;
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
