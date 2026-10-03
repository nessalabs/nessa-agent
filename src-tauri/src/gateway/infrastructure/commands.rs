//! Tauri entry points and event adapter for managed gateway startup.

use super::super::application::{
    Gateway, GatewayStartup as ApplicationStartup, GatewayStartupEvents, GatewayStartupPhase,
    StartupStep,
};
use crate::{
    composition::HostDependencies,
    desktop_window::DESKTOP_WINDOW,
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
        for (label, _) in BUNDLED_WINDOWS {
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

/// The bundled surfaces, by their windows' labels: the one list that both
/// admits a window as one ([`bundled_window`]) and is published startup to
/// ([`HostGatewayStartupEvents`]), so the two cannot disagree (#419).
const BUNDLED_WINDOWS: [(&str, BundledSurface); 2] = [
    (panel::MAIN_WINDOW, BundledSurface::Main),
    (panel::SETUP_WINDOW, BundledSurface::Setup),
];

pub(crate) fn bundled_window(label: &str) -> Result<BundledSurface, String> {
    BUNDLED_WINDOWS
        .iter()
        .find(|(bundled, _)| *bundled == label)
        .map(|(_, surface)| *surface)
        .ok_or_else(|| "Only a bundled Nessa surface can access the gateway".into())
}

/// Who may read the local gateway's endpoint and the surface credential, and
/// how each waits for the gateway first (#419).
///
/// The host starts the gateway at launch (`Gateway::start`), and each
/// bundled surface waits for it to reconcile, which its load may start again,
/// on record as the initiator. The desktop window only reads the gateway that
/// startup brought up: it is served once startup is `Ready` and refused
/// before that. Its arm of [`GatewayReader::ready`] reads the startup state
/// and calls nothing that reconciles, so a window polling while it cannot
/// connect causes no reconciliation (the desktop-window tests in
/// `surface_credential.rs` and `gateway_endpoint`). It is no bundled surface
/// ([`bundled_window`]): the `gateway_startup` and `retry_gateway_startup`
/// commands refuse it, and no startup event is published to it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum GatewayReader {
    /// The panel or setup, which waits for the gateway to reconcile.
    Surface(BundledSurface),
    /// The desktop window, which reads a gateway that is already ready.
    DesktopWindow,
}

impl GatewayReader {
    /// The reader a window is, by its label; any other window is refused.
    pub(crate) fn of_window(label: &str) -> Result<Self, String> {
        if label == DESKTOP_WINDOW {
            return Ok(Self::DesktopWindow);
        }
        bundled_window(label)
            .map(Self::Surface)
            .map_err(|_| "Only Nessa's own windows can read the gateway".into())
    }

    /// Waits for the gateway as this reader may: a bundled surface until it
    /// reconciles; the desktop window not at all — it is ready now, or the
    /// read is refused.
    pub(crate) async fn ready(self, gateway: &Gateway) -> Result<(), String> {
        match self {
            Self::Surface(surface) => gateway
                .wait_ready(surface)
                .await
                .map_err(|error| error.to_string()),
            Self::DesktopWindow => match gateway
                .startup()
                .map_err(|error| error.to_string())?
                .phase()
            {
                GatewayStartupPhase::Ready => Ok(()),
                GatewayStartupPhase::Starting(_) | GatewayStartupPhase::Failed(_) => {
                    Err("The local server isn't ready yet".into())
                }
            },
        }
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
    use super::{bundled_window, payload, GatewayReader};
    use crate::{
        desktop_window::DESKTOP_WINDOW,
        gateway::{application::GatewayStartup, domain::value_objects::BundledSurface},
        host, panel,
    };

    #[test]
    fn the_desktop_window_reads_the_gateway_and_is_no_bundled_surface() {
        assert_eq!(
            GatewayReader::of_window(DESKTOP_WINDOW),
            Ok(GatewayReader::DesktopWindow)
        );
        assert!(bundled_window(DESKTOP_WINDOW).is_err());
        assert_eq!(
            GatewayReader::of_window(panel::MAIN_WINDOW),
            Ok(GatewayReader::Surface(BundledSurface::Main))
        );
        assert_eq!(
            GatewayReader::of_window(panel::SETUP_WINDOW),
            Ok(GatewayReader::Surface(BundledSurface::Setup))
        );
        assert!(GatewayReader::of_window("untrusted").is_err());
    }

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
