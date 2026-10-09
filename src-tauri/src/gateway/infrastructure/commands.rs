//! Tauri entry points and event adapter for managed gateway startup.

use super::super::application::{
    ClaudeConfigurationChangeError, ClaudeDirectorySettings, ClaudeSettingsPublishError, Gateway,
    GatewayStartup as ApplicationStartup, GatewayStartupEvents, GatewayStartupPhase, StartupStep,
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
use std::sync::Arc;
use std::{
    io::Error as IoError,
    panic::{catch_unwind, AssertUnwindSafe},
    path::PathBuf,
};
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
    pub(crate) async fn ready(self, gateway: &Gateway) -> Result<(), GatewayUnread> {
        match self {
            Self::Surface(surface) => gateway
                .wait_ready(surface)
                .await
                .map_err(|error| GatewayUnread::Other(error.to_string())),
            Self::DesktopWindow => match gateway
                .startup()
                .map_err(|error| GatewayUnread::Other(error.to_string()))?
                .phase()
            {
                GatewayStartupPhase::Ready => Ok(()),
                GatewayStartupPhase::Starting(_) | GatewayStartupPhase::Failed(_) => {
                    Err(GatewayUnread::NotReady)
                }
            },
        }
    }
}

/// Why a reader was not given a gateway that is up.
///
/// `NotReady` is the desktop window's own fact: startup has not reached
/// `Ready`. Anything a bundled surface's wait reports stays that wait's
/// words (`Other`), so a registration failure is not rewritten as "still
/// starting".
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum GatewayUnread {
    NotReady,
    Other(String),
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
    /// The original failure and a failed durable rollback are both retained.
    Rollback {
        failure: Box<ClaudeConfigurationError>,
        settings: Option<String>,
        gateway: Option<String>,
    },
}

/// Write the Claude configuration directory and re-register when it changed.
///
/// A development build has no packaged gateway
/// (`a_development_build_saves_the_directory_without_a_gateway`). The settings
/// file still changes, and the next packaged launch reads it. An unchanged
/// directory does not reconcile
/// (`a_settings_change_registers_once_and_a_repeat_does_not`).
pub(crate) async fn apply_claude_configuration_directory(
    settings: Arc<dyn SettingsStore>,
    gateway: Option<Arc<Gateway>>,
    surface: BundledSurface,
    directory: Option<String>,
) -> Result<(), ClaudeConfigurationError> {
    let directory = durable_directory(directory)?;
    let saved = ClaudeSettings(settings);
    if let Some(gateway) = gateway {
        gateway
            .change_claude_configuration(surface, directory, Arc::new(saved))
            .await
            .map_err(configuration_change_error)?;
    } else {
        saved.publish(directory).map_err(|error| {
            let message = match error {
                ClaudeSettingsPublishError::Unavailable(message)
                | ClaudeSettingsPublishError::NotConfirmed { message, .. } => message,
            };
            ClaudeConfigurationError::Settings { message }
        })?;
    }
    Ok(())
}

struct ClaudeSettings(Arc<dyn SettingsStore>);
impl ClaudeDirectorySettings for ClaudeSettings {
    fn publish(
        &self,
        directory: Option<PathBuf>,
    ) -> Result<Option<PathBuf>, ClaudeSettingsPublishError> {
        let mut previous = None;
        let result = catch_unwind(AssertUnwindSafe(|| {
            self.0.update(&mut |chosen| {
                previous = Some(chosen.service.claude.configuration_directory.clone());
                chosen.service.claude.configuration_directory = directory.clone();
            })
        }))
        .unwrap_or_else(|_| Err(IoError::other("claude settings publication panicked")));
        match result {
            Ok(_) => previous.ok_or_else(|| {
                ClaudeSettingsPublishError::Unavailable(
                    "The settings adapter did not apply the directory publication".into(),
                )
            }),
            Err(error) => match previous {
                Some(previous) => Err(ClaudeSettingsPublishError::NotConfirmed {
                    message: error.to_string(),
                    previous,
                }),
                None => Err(ClaudeSettingsPublishError::Unavailable(error.to_string())),
            },
        }
    }
    fn restore(&self, expected: &Option<PathBuf>, previous: Option<PathBuf>) -> Result<(), String> {
        self.0
            .update(&mut |chosen| {
                if &chosen.service.claude.configuration_directory == expected {
                    chosen.service.claude.configuration_directory = previous.clone();
                }
            })
            .map(|_| ())
            .map_err(|error| error.to_string())
    }
}

fn configuration_change_error(error: ClaudeConfigurationChangeError) -> ClaudeConfigurationError {
    match error {
        ClaudeConfigurationChangeError::Settings(message) => {
            ClaudeConfigurationError::Settings { message }
        }
        ClaudeConfigurationChangeError::Gateway(error) => ClaudeConfigurationError::Gateway {
            message: error.to_string(),
        },
        ClaudeConfigurationChangeError::Rollback {
            failure,
            settings,
            gateway,
        } => ClaudeConfigurationError::Rollback {
            failure: Box::new(configuration_change_error(*failure)),
            settings,
            gateway: gateway.map(|error| error.to_string()),
        },
    }
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
        deps.settings.clone(),
        deps.gateway.clone(),
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
    use crate::settings::{Service, Settings, SettingsStore};
    use std::{
        future::Future,
        io,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicBool, AtomicUsize, Ordering},
            mpsc, Arc, Mutex,
        },
        task::{Context, Poll, Waker},
        time::Duration,
    };

    struct DirectoryHost {
        directory: Mutex<Option<PathBuf>>,
        causes: Mutex<Vec<ReconciliationCause>>,
        fail: bool,
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
            if self.fail {
                return Err(GatewayError::Registration(
                    "definition was not published".into(),
                ));
            }
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

    fn gateway(host: Arc<DirectoryHost>) -> Arc<Gateway> {
        Arc::new(Gateway::bootstrap(
            host,
            Arc::new(FixedLoginShell(Ok(SearchPath::parse("/usr/bin").unwrap()))),
            testing::discard_startup_events(),
            testing::sequential_reconciliation_ids(),
            testing::discard_reconciliation_audit(),
            "/runtime".into(),
            "ci".into(),
        ))
    }

    #[test]
    fn a_settings_change_registers_once_and_a_repeat_does_not() {
        let saved = in_memory();
        let store = Arc::new(saved.store);
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
            fail: false,
        });
        let gateway = gateway(host.clone());
        let directory = absolute_claude_directory(".claude-work");

        tauri::async_runtime::block_on(apply_claude_configuration_directory(
            store.clone(),
            Some(gateway.clone()),
            BundledSurface::Main,
            Some(directory.clone()),
        ))
        .unwrap();
        tauri::async_runtime::block_on(apply_claude_configuration_directory(
            store.clone(),
            Some(gateway.clone()),
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
            store
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
        let store = Arc::new(saved.store);

        tauri::async_runtime::block_on(apply_claude_configuration_directory(
            store.clone(),
            None,
            BundledSurface::Setup,
            Some(absolute_claude_directory(".claude-work")),
        ))
        .unwrap();

        assert_eq!(
            store
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
        let store = Arc::new(saved.store);
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
            fail: false,
        });
        let gateway = gateway(host.clone());

        assert_eq!(
            tauri::async_runtime::block_on(apply_claude_configuration_directory(
                store.clone(),
                Some(gateway.clone()),
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
        let store = Arc::new(saved.store);
        saved.storage.put(&saved.path, b"{");
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
            fail: false,
        });
        let gateway = gateway(host.clone());

        assert!(matches!(
            tauri::async_runtime::block_on(apply_claude_configuration_directory(
                store.clone(),
                Some(gateway.clone()),
                BundledSurface::Main,
                Some(absolute_claude_directory(".claude-work")),
            )),
            Err(ClaudeConfigurationError::Settings { .. })
        ));
        assert_eq!(saved.storage.get(&saved.path).unwrap(), b"{");
        assert!(host.causes.lock().unwrap().is_empty());
    }
    #[test]
    fn a_failed_native_change_restores_the_real_settings_store_and_live_directory() {
        let saved = in_memory();
        let store = Arc::new(saved.store);
        let old = PathBuf::from(absolute_claude_directory(".claude-old"));
        store
            .update(&mut |settings| {
                settings.service.claude.configuration_directory = Some(old.clone())
            })
            .unwrap();
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(Some(old.clone())),
            causes: Mutex::new(Vec::new()),
            fail: true,
        });
        let gateway = gateway(host.clone());
        assert!(matches!(
            tauri::async_runtime::block_on(apply_claude_configuration_directory(
                store.clone(),
                Some(gateway.clone()),
                BundledSurface::Setup,
                Some(absolute_claude_directory(".claude-new")),
            )),
            Err(ClaudeConfigurationError::Gateway { .. })
        ));
        assert_eq!(
            store.load_service().unwrap().claude.configuration_directory,
            Some(old.clone())
        );
        assert_eq!(*host.directory.lock().unwrap(), Some(old));
    }
    struct UnconfirmedSettings {
        inner: Arc<dyn SettingsStore>,
        host: Arc<DirectoryHost>,
        prior: Option<PathBuf>,
        failures: AtomicUsize,
        skip_callback: bool,
        panic_after_save: bool,
    }
    impl SettingsStore for UnconfirmedSettings {
        fn load(&self) -> Settings {
            self.inner.load()
        }
        fn load_service(&self) -> io::Result<Service> {
            self.inner.load_service()
        }
        fn update(&self, change: &mut dyn FnMut(&mut Settings)) -> io::Result<Settings> {
            // Another reconciliation must still read the old live configuration while the save is unconfirmed.
            assert_eq!(*self.host.directory.lock().unwrap(), self.prior);
            if self.skip_callback {
                return Ok(self.inner.load());
            }
            let updated = self.inner.update(change)?;
            if self
                .failures
                .fetch_update(Ordering::SeqCst, Ordering::SeqCst, |count| {
                    count.checked_sub(1)
                })
                .is_ok()
            {
                assert!(!self.panic_after_save, "settings acknowledgement panicked");
                return Err(io::Error::other("settings publication not confirmed"));
            }
            Ok(updated)
        }
    }

    #[test]
    fn an_unconfirmed_settings_publication_restores_its_known_prior_before_native_dispatch() {
        for (failures, panic_after_save) in [(1, false), (2, false), (1, true), (2, true)] {
            let saved = in_memory();
            let store = Arc::new(saved.store);
            let prior = Some(PathBuf::from(absolute_claude_directory(".claude-prior")));
            store
                .update(&mut |settings| {
                    settings.service.claude.configuration_directory = prior.clone()
                })
                .unwrap();
            let host = Arc::new(DirectoryHost {
                directory: Mutex::new(prior.clone()),
                causes: Mutex::new(Vec::new()),
                fail: false,
            });
            let gateway = gateway(host.clone());
            let uncertain = UnconfirmedSettings {
                inner: store.clone(),
                host: host.clone(),
                prior: prior.clone(),
                failures: AtomicUsize::new(failures),
                skip_callback: false,
                panic_after_save,
            };
            let result = tauri::async_runtime::block_on(apply_claude_configuration_directory(
                Arc::new(uncertain),
                Some(gateway.clone()),
                BundledSurface::Setup,
                Some(absolute_claude_directory(".claude-requested")),
            ));
            if failures == 1 {
                assert!(matches!(
                    result,
                    Err(ClaudeConfigurationError::Settings { .. })
                ));
            } else {
                assert!(
                    matches!(result, Err(ClaudeConfigurationError::Rollback { failure, settings: Some(_), gateway: None }) if matches!(*failure, ClaudeConfigurationError::Settings { .. }))
                );
            }
            assert_eq!(
                store.load_service().unwrap().claude.configuration_directory,
                prior
            );
            assert_eq!(*host.directory.lock().unwrap(), prior);
            assert!(host.causes.lock().unwrap().is_empty());
        }
    }

    #[test]
    fn settings_success_without_applying_the_owned_change_does_not_dispatch_native_work() {
        let saved = in_memory();
        let store = Arc::new(saved.store);
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
            fail: false,
        });
        let gateway = gateway(host.clone());
        let unsupported = UnconfirmedSettings {
            inner: store.clone(),
            host: host.clone(),
            prior: None,
            failures: AtomicUsize::new(0),
            skip_callback: true,
            panic_after_save: false,
        };
        assert!(matches!(
            tauri::async_runtime::block_on(apply_claude_configuration_directory(
                Arc::new(unsupported),
                Some(gateway.clone()),
                BundledSurface::Setup,
                Some(absolute_claude_directory(".claude-requested"))
            )),
            Err(ClaudeConfigurationError::Settings { .. })
        ));
        assert!(host.causes.lock().unwrap().is_empty());
        assert_eq!(*host.directory.lock().unwrap(), None);
        assert!(store
            .load_service()
            .unwrap()
            .claude
            .configuration_directory
            .is_none());
    }

    #[test]
    fn a_dropped_caller_during_settings_save_keeps_the_owned_store_through_native_settlement() {
        struct GatedStore {
            inner: Arc<dyn SettingsStore>,
            entered: mpsc::SyncSender<()>,
            release: Mutex<mpsc::Receiver<()>>,
            first: AtomicBool,
        }
        impl SettingsStore for GatedStore {
            fn load(&self) -> Settings {
                self.inner.load()
            }
            fn load_service(&self) -> io::Result<Service> {
                self.inner.load_service()
            }
            fn update(&self, change: &mut dyn FnMut(&mut Settings)) -> io::Result<Settings> {
                if self.first.swap(false, Ordering::SeqCst) {
                    self.entered.send(()).unwrap();
                    self.release.lock().unwrap().recv().unwrap();
                }
                self.inner.update(change)
            }
        }
        let saved = in_memory();
        let store = Arc::new(saved.store);
        let host = Arc::new(DirectoryHost {
            directory: Mutex::new(None),
            causes: Mutex::new(Vec::new()),
            fail: false,
        });
        let gateway = gateway(host.clone());
        let (entered, arrival) = mpsc::sync_channel(1);
        let (release, wait) = mpsc::sync_channel(1);
        let owned: Arc<dyn SettingsStore> = Arc::new(GatedStore {
            inner: store.clone(),
            entered,
            release: Mutex::new(wait),
            first: AtomicBool::new(true),
        });
        let requested = absolute_claude_directory(".claude-owned-store");
        let mut first = Box::pin(apply_claude_configuration_directory(
            owned.clone(),
            Some(gateway.clone()),
            BundledSurface::Main,
            Some(requested.clone()),
        ));
        assert!(matches!(
            first.as_mut().poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        arrival.recv_timeout(Duration::from_secs(2)).unwrap();
        drop(first);
        assert_eq!(*host.directory.lock().unwrap(), None);
        assert_eq!(
            store.load_service().unwrap().claude.configuration_directory,
            None
        );
        let mut joined = Box::pin(apply_claude_configuration_directory(
            owned,
            Some(gateway),
            BundledSurface::Setup,
            Some(requested.clone()),
        ));
        assert!(matches!(
            joined
                .as_mut()
                .poll(&mut Context::from_waker(Waker::noop())),
            Poll::Pending
        ));
        release.send(()).unwrap();
        tauri::async_runtime::block_on(joined).unwrap();
        assert_eq!(
            store.load_service().unwrap().claude.configuration_directory,
            Some(PathBuf::from(requested.clone()))
        );
        assert_eq!(
            *host.directory.lock().unwrap(),
            Some(PathBuf::from(requested))
        );
        assert_eq!(
            host.causes.lock().unwrap().as_slice(),
            [ReconciliationCause::ClaudeConfigurationChanged]
        );
    }
}
