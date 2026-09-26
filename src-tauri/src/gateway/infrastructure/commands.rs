//! Tauri entry points and event adapter for managed gateway startup.

use super::super::application::{
    GatewayStartup as ApplicationStartup, GatewayStartupEvents, GatewayStartupPhase, StartupStep,
};
use crate::{
    composition::HostDependencies,
    gateway::domain::value_objects::BundledSurface,
    host::{self, GatewayStartup, GATEWAY_STARTUP},
    panel,
};
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State, WebviewWindow};

pub(super) struct HostGatewayStartupEvents {
    app: AppHandle,
}

impl GatewayStartupEvents for HostGatewayStartupEvents {
    fn publish(&self, startup: &ApplicationStartup) {
        let payload = payload(startup);
        for label in [panel::MAIN_WINDOW, panel::SETUP_WINDOW] {
            if let Err(error) = self.app.emit_to(label, GATEWAY_STARTUP, &payload) {
                eprintln!("[nessa] could not publish gateway startup to {label}: {error}");
            }
        }
    }
}

pub fn startup_events(app: &AppHandle) -> Arc<dyn GatewayStartupEvents> {
    Arc::new(HostGatewayStartupEvents { app: app.clone() })
}

fn payload(startup: &ApplicationStartup) -> GatewayStartup {
    let revision = startup.revision();
    match startup.phase() {
        GatewayStartupPhase::Starting(step) => GatewayStartup::Starting {
            revision,
            step: match step {
                StartupStep::Preparing => host::StartupStep::Preparing,
                StartupStep::Replacing => host::StartupStep::Replacing,
                StartupStep::Launching => host::StartupStep::Launching,
            },
        },
        GatewayStartupPhase::Ready => GatewayStartup::Ready { revision },
        GatewayStartupPhase::Failed(error) => GatewayStartup::Failed {
            revision,
            message: error.to_string(),
        },
    }
}

fn bundled_window(label: &str) -> Result<BundledSurface, String> {
    match label {
        panel::MAIN_WINDOW => Ok(BundledSurface::Main),
        panel::SETUP_WINDOW => Ok(BundledSurface::Setup),
        _ => Err("Only a bundled Nessa surface can inspect gateway startup".into()),
    }
}

#[tauri::command]
pub fn gateway_startup(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<GatewayStartup, String> {
    bundled_window(window.label())?;
    deps.gateway
        .as_deref()
        .map(|gateway| gateway.startup().map(|startup| payload(&startup)))
        .transpose()
        .map(|startup| startup.unwrap_or(GatewayStartup::Unmanaged { revision: 0 }))
        .map_err(|error| error.to_string())
}

#[tauri::command]
pub async fn retry_gateway_startup(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<(), String> {
    let surface = bundled_window(window.label())?;
    if let Some(gateway) = deps.gateway.as_deref() {
        gateway
            .retry(surface)
            .await
            .map_err(|error| error.to_string())?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::{bundled_window, payload};
    use crate::{
        gateway::{application::GatewayStartup, domain::value_objects::BundledSurface},
        host, panel,
    };

    #[test]
    fn only_bundled_surfaces_may_reach_gateway_startup() {
        assert_eq!(bundled_window(panel::MAIN_WINDOW), Ok(BundledSurface::Main));
        assert_eq!(
            bundled_window(panel::SETUP_WINDOW),
            Ok(BundledSurface::Setup)
        );
        assert!(bundled_window("untrusted").is_err());
    }

    #[test]
    fn initial_managed_startup_keeps_its_revision_at_the_seam() {
        assert_eq!(
            payload(&GatewayStartup::starting()),
            host::GatewayStartup::Starting {
                revision: 0,
                step: host::StartupStep::Preparing,
            }
        );
    }
}
