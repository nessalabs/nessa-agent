//! Tauri entry points and event adapter for managed gateway startup.

use super::super::application::{
    Gateway, GatewayStartup as ApplicationStartup, GatewayStartupEvents, GatewayStartupPhase,
    StartupStep,
};
use crate::gateway::domain::value_objects::claude_config_directory_is_durable;
use crate::settings::SettingsStore;
use crate::{
    composition::HostDependencies,
    desktop_window::DESKTOP_WINDOW,
    gateway::domain::value_objects::BundledSurface,
    host::{self, GatewayStartup, GATEWAY_STARTUP},
    panel,
};
use serde::Serialize;
use std::path::PathBuf;
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

/// Why a Claude configuration-directory change was not applied.
#[derive(Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum ClaudeConfigurationError {
    /// The window is not a bundled Nessa surface.
    UntrustedCaller,
    /// The directory is not an absolute, normalized path.
    Directory,
    /// The settings file could not be updated.
    Settings { message: String },
    /// The packaged gateway did not accept the new directory.
    Gateway { message: String },
}

/// Write the Claude configuration directory and re-register when it changed.
///
/// A development build has no packaged gateway
/// (`a_development_build_saves_the_directory_without_a_gateway`). The settings
/// file still changes, and the next packaged launch reads it. An unchanged
/// directory does not reconcile
/// (`a_settings_change_registers_once_and_a_repeat_does_not`).
pub(crate) async fn apply_claude_configuration_directory(
    settings: &dyn SettingsStore,
    gateway: Option<&Gateway>,
    surface: BundledSurface,
    directory: Option<String>,
) -> Result<(), ClaudeConfigurationError> {
    let directory = durable_directory(directory)?;
    settings
        .update(&mut |chosen| {
            chosen.service.claude.configuration_directory = directory.clone();
        })
        .map_err(|error| ClaudeConfigurationError::Settings {
            message: error.to_string(),
        })?;
    if let Some(gateway) = gateway {
        gateway
            .change_claude_configuration(surface, directory)
            .await
            .map_err(|error| ClaudeConfigurationError::Gateway {
                message: error.to_string(),
            })?;
    }
    Ok(())
}

fn durable_directory(
    directory: Option<String>,
) -> Result<Option<PathBuf>, ClaudeConfigurationError> {
    let directory = directory.and_then(|value| {
        if value.trim().is_empty() {
            None
        } else {
            Some(PathBuf::from(value))
        }
    });
    claude_config_directory_is_durable(directory.as_deref())
        .map_err(|_| ClaudeConfigurationError::Directory)?;
    Ok(directory)
}

fn configuration_surface(label: &str) -> Result<BundledSurface, ClaudeConfigurationError> {
    bundled_window(label).map_err(|_| ClaudeConfigurationError::UntrustedCaller)
}

/// Change Claude's configuration directory from a bundled surface.
///
/// The desktop window is refused: it reads a gateway that is already up and
/// does not retire agents (#419). Setup and the panel are the surfaces that
/// may re-register, and the audit records that surface.
#[tauri::command]
pub async fn set_claude_configuration_directory(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    directory: Option<String>,
) -> Result<(), ClaudeConfigurationError> {
    let surface = configuration_surface(window.label())?;
    apply_claude_configuration_directory(
        deps.settings.as_ref(),
        deps.gateway.as_deref(),
        surface,
        directory,
    )
    .await
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

    #[test]
    fn only_bundled_surfaces_may_change_the_claude_configuration_directory() {
        assert_eq!(
            super::configuration_surface(panel::MAIN_WINDOW),
            Ok(BundledSurface::Main)
        );
        assert_eq!(
            super::configuration_surface(panel::SETUP_WINDOW),
            Ok(BundledSurface::Setup)
        );
        assert_eq!(
            super::configuration_surface(DESKTOP_WINDOW),
            Err(super::ClaudeConfigurationError::UntrustedCaller)
        );
        assert_eq!(
            super::configuration_surface("untrusted"),
            Err(super::ClaudeConfigurationError::UntrustedCaller)
        );
    }
}

#[cfg(test)]
mod configuration_directory {
    use super::{apply_claude_configuration_directory, ClaudeConfigurationError};
    use crate::gateway::{
        application::{
            testing::{self, FixedLoginShell},
            ClaudeDirectoryReplacement, Gateway, GatewayError, GatewayHost,
            GatewayReconciliationAttempt, GatewayReconciliationIntent,
            GatewayReconciliationJournalSession, GatewayReconciliationProgress, GatewayStopSession,
            ReconciledGateway,
        },
        domain::value_objects::{
            AuditDeliveryReceipt, BundledSurface, LifecycleObservation, ReconciliationCause,
            ReconciliationTarget, SearchPath,
        },
    };
    use crate::settings::testing::in_memory;
    use crate::settings::SettingsStore;
    use std::{
        path::{Path, PathBuf},
        sync::{Arc, Mutex},
    };

    struct DirectoryHost {
        directory: Mutex<Option<PathBuf>>,
        causes: Mutex<Vec<ReconciliationCause>>,
    }

    impl GatewayHost for DirectoryHost {
        fn replace_claude_config_directory(
            &self,
            directory: Option<PathBuf>,
        ) -> Result<ClaudeDirectoryReplacement, GatewayError> {
            let mut current = self.directory.lock().unwrap();
            if *current == directory {
                return Ok(ClaudeDirectoryReplacement::Unchanged);
            }
            let previous = current.clone();
            *current = directory;
            Ok(ClaudeDirectoryReplacement::Changed { previous })
        }

        fn restore_claude_config_directory(
            &self,
            expected: &Option<PathBuf>,
            previous: Option<PathBuf>,
        ) -> Result<bool, GatewayError> {
            let mut current = self.directory.lock().unwrap();
            if *current != *expected || *current == previous {
                return Ok(false);
            }
            *current = previous;
            Ok(true)
        }

        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            self.causes
                .lock()
                .unwrap()
                .push(attempt.origin().evidence().cause());
            let service = "claude-directory";
            let gateway = ReconciledGateway::new(
                service.into(),
                "a".repeat(64),
                "550e8400-e29b-41d4-a716-446655440000".into(),
                "b".repeat(64),
                42,
                7420,
            );
            let target =
                ReconciliationTarget::new(service.into(), "a".repeat(64), "b".repeat(64)).unwrap();
            progress
                .intent_admitted(
                    GatewayReconciliationIntent::new(
                        attempt.clone(),
                        target,
                        Some(gateway.audit_identity().unwrap()),
                    )
                    .unwrap(),
                )
                .unwrap();
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            _: &GatewayStopSession,
            _: &dyn GatewayReconciliationJournalSession,
            _: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            Err(GatewayError::Stop("not used".into()))
        }
    }

    fn absolute_claude_directory(name: &str) -> String {
        if cfg!(windows) {
            format!(r"C:\Users\me\{name}")
        } else {
            format!("/Users/me/{name}")
        }
    }

    fn gateway(host: Arc<DirectoryHost>) -> Gateway {
        Gateway::bootstrap(
            host,
            Arc::new(FixedLoginShell(Ok(SearchPath::parse("/usr/bin").unwrap()))),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            testing::discard_reconciliation_audit(),
            "/runtime".into(),
            "ci".into(),
        )
    }

    #[test]
    fn a_settings_change_registers_once_and_a_repeat_does_not() {
        let saved = in_memory();
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
        });
        let gateway = gateway(host.clone());
        let directory = absolute_claude_directory(".claude-work");

        tauri::async_runtime::block_on(apply_claude_configuration_directory(
            &saved.store,
            Some(&gateway),
            BundledSurface::Main,
            Some(directory.clone()),
        ))
        .unwrap();
        tauri::async_runtime::block_on(apply_claude_configuration_directory(
            &saved.store,
            Some(&gateway),
            BundledSurface::Main,
            Some(directory.clone()),
        ))
        .unwrap();

        let file = String::from_utf8(saved.storage.get(&saved.path).unwrap()).unwrap();
        assert!(file.contains(".claude-work"));
        assert!(!file.contains("ANTHROPIC_API_KEY"));
        assert!(!file.contains("CLAUDE_CODE_OAUTH_TOKEN"));
        assert_eq!(
            host.causes.lock().unwrap().as_slice(),
            [ReconciliationCause::ClaudeConfigurationChanged]
        );
        assert_eq!(
            saved
                .store
                .load()
                .service
                .claude
                .configuration_directory
                .as_deref(),
            Some(Path::new(&directory))
        );
    }

    #[test]
    fn a_development_build_saves_the_directory_without_a_gateway() {
        let saved = in_memory();

        tauri::async_runtime::block_on(apply_claude_configuration_directory(
            &saved.store,
            None,
            BundledSurface::Setup,
            Some(absolute_claude_directory(".claude-work")),
        ))
        .unwrap();

        assert_eq!(
            saved
                .store
                .load()
                .service
                .claude
                .configuration_directory
                .as_deref(),
            Some(Path::new(&absolute_claude_directory(".claude-work")))
        );
    }

    #[test]
    fn a_relative_directory_is_refused_without_writing_or_registering() {
        let saved = in_memory();
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
        });
        let gateway = gateway(host.clone());

        assert_eq!(
            tauri::async_runtime::block_on(apply_claude_configuration_directory(
                &saved.store,
                Some(&gateway),
                BundledSurface::Setup,
                Some("relative/claude".into()),
            )),
            Err(ClaudeConfigurationError::Directory)
        );
        assert!(saved.storage.get(&saved.path).is_none());
        assert!(host.causes.lock().unwrap().is_empty());
    }

    #[test]
    fn an_unusable_settings_file_does_not_register() {
        let saved = in_memory();
        saved.storage.put(&saved.path, b"{");
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
        });
        let gateway = gateway(host.clone());

        assert!(matches!(
            tauri::async_runtime::block_on(apply_claude_configuration_directory(
                &saved.store,
                Some(&gateway),
                BundledSurface::Main,
                Some(absolute_claude_directory(".claude-work")),
            )),
            Err(ClaudeConfigurationError::Settings { .. })
        ));
        assert_eq!(saved.storage.get(&saved.path).unwrap(), b"{");
        assert!(host.causes.lock().unwrap().is_empty());
    }
}
