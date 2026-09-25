//! The desktop host's composition root.
//!
//! Everything the host reads from outside the process — the settings file, the
//! shortcut cache, the surface credential, the background service, the release
//! endpoint, the file picker and the filesystem behind the files it answers
//! with — is
//! constructed here, once, while `main`'s `setup` is assembling the app.
//! Nothing below this point constructs any of them, and nothing below this
//! point goes looking for them either.
//!
//! ```text
//!   main::setup ──assemble──▶ HostDependencies ──manage──▶ Tauri
//!                                   │                        │
//!                 tray::create, updater, panel size      State<…> on commands
//!                                   │                        │
//!                              plain parameters ◀────────── resolve at handlers
//! ```
//! Arrows mean construction or handing over. The rule they describe:
//! **resolution happens at entry points; logic takes explicit parameters.**
//!
//! The entry points are the three shapes the framework offers, and no others:
//!
//! - `setup`, which builds the bundle and passes it down by hand;
//! - a `#[tauri::command]`, which declares `State<'_, HostDependencies>` and
//!   lets Tauri hand it over — framework-supported injection at the boundary;
//! - a handler the framework calls with only an `&AppHandle` — the tray menu,
//!   the window events, the exit event. A handler built while the bundle is in
//!   hand captures it (the tray menu does); one that is not resolves it with a
//!   single [`resolve`] at the top and passes what it found downwards.
//!
//! What is *not* here is as deliberate. The tray's menu items, the summon
//! registration slot, the pending update, and the settings snapshot a launch was
//! sized from are live objects and mutable slots, not outside things: there is
//! nothing to substitute for a window-server handle. Those stay managed state,
//! reached where they are used.

use std::{io, sync::Arc};

use tauri::{AppHandle, Manager};

use crate::agent_credentials::{
    application::{
        AgentCredentialStore, CredentialSaveAudit, CredentialSaveIds, CredentialSaveTargets,
    },
    infrastructure::{
        CanonicalCredentialSaveTargets, FileCredentialSaveAudit, LocalAgentCredentialStore,
        RandomCredentialSaveIds, UnavailableCredentialSaveAudit,
    },
};
use crate::attachments::{
    self, AttachmentTickets, ChosenFiles, ContentTypes, DragBoard, FilePicker, Readiness,
};
use crate::gateway::domain::value_objects::ServiceConfiguration;
use crate::gateway::{
    self,
    application::{Gateway, GatewayRuntimeDependencies, SystemMonotonicClock},
};
use crate::gateway_endpoint::{self, application::GatewayEndpointAccess};
use crate::local_data;
use crate::settings::{SettingsFile, SettingsStore};
use crate::shortcuts::{ShortcutStore, ShortcutsFile};
use crate::surface_credential::{service_namespace, SurfaceCredential, SurfaceCredentials};
#[cfg(desktop)]
use crate::updater::{self, ReleaseSource};

/// Every outside thing the host talks to, in one value.
///
/// Cheap to clone — each field is a pointer — because the bundle is handed to
/// Tauri to manage and kept by `setup` at the same time, and captured by the
/// handlers built during startup. Consumers are given the fields they use, not
/// the bundle: a function that needs the settings file takes
/// `&dyn SettingsStore`, which is also what makes it testable.
#[derive(Clone)]
pub struct HostDependencies {
    /// The settings file: the panel's geometry, the quit policy, and whether
    /// first-run setup has finished.
    pub settings: Arc<dyn SettingsStore>,
    /// Nessa-owned secure storage for supported local-agent API keys.
    pub agent_credentials: Arc<dyn AgentCredentialStore>,
    /// Canonical audited destinations, independent of the effect adapter's claim.
    pub credential_save_targets: Arc<dyn CredentialSaveTargets>,
    /// Correlations for credential-save audit lifecycles.
    pub credential_save_ids: Arc<dyn CredentialSaveIds>,
    /// Durable secret-free intent and outcome records for credential replacement.
    pub credential_save_audit: Arc<dyn CredentialSaveAudit>,
    /// The host-owned shortcut cache the shell hydrates from.
    pub shortcuts: Arc<dyn ShortcutStore>,
    /// The bundled surface's native credential.
    pub credential: Arc<dyn SurfaceCredentials>,
    /// Health-correlated endpoint source bound to the same service namespace.
    pub endpoint: Arc<GatewayEndpointAccess>,
    /// Where the person chooses files to attach: the operating system's own
    /// file dialog, which the webview has no way of putting on screen itself.
    pub picker: Arc<dyn FilePicker>,
    /// What the filesystem says about a file the picker answered with: how long
    /// it is, and what is in it. Its own dependency rather than part of the
    /// picker, because the filesystem is a different outside thing from the
    /// window the choice was made in and refuses for its own reasons — and one
    /// dependency rather than two, because a length and the contents behind it
    /// are that same filesystem asked twice.
    pub files: Arc<dyn ChosenFiles>,
    /// What this machine says a chosen file *is*. Its own dependency rather
    /// than another question on the filesystem, because it is a different
    /// outside thing — Launch Services on macOS, a subprocess on Linux, nothing
    /// at all elsewhere — and because it does not fail: a platform with no
    /// answer shrugs, and the panel's extension fallback carries the file.
    pub types: Arc<dyn ContentTypes>,
    /// The host's memory of the paths a person chose, and the one-shot tickets
    /// that stand in for them.
    ///
    /// One desk for the process, built here, because a ticket minted by one
    /// desk and presented to another is a ticket nobody has heard of. It is
    /// what stops the webview naming a path of its own to read.
    pub tickets: Arc<dyn AttachmentTickets>,
    /// Who can make a chosen file readable when it is not yet. Its own
    /// dependency because it is a service that starts work and reports on it
    /// later, rather than the filesystem answering about a path.
    pub readiness: Arc<Readiness>,
    /// What the drag currently over the panel is carrying besides files.
    ///
    /// Held here because it is state the host keeps between two window events
    /// — the snapshot taken when a drag enters, read when it drops. See
    /// `attachments::dragged` for why it cannot be read at the drop instead.
    pub dragging: Arc<DragBoard>,
    /// The registered background service, in the builds that have one.
    ///
    /// `None` in a development build: the gateway is a packaged runtime that a
    /// dev build does not stage or register, and the panel talks to a gateway
    /// somebody started themselves. It is not an error and nothing waits.
    pub gateway: Option<Arc<Gateway>>,
    /// Where "is there a newer Nessa?" is asked. See
    /// [`crate::updater::release_source`] for which source a build gets.
    ///
    /// Desktop only, matching its one caller: an installed phone app is
    /// replaced by its store, not by itself.
    #[cfg(desktop)]
    pub releases: Arc<dyn ReleaseSource>,
}

impl HostDependencies {
    /// Builds every dependency the host has, in the order `setup` used to build
    /// them in.
    ///
    /// `stage` was resolved once before Tauri started. It names both the config
    /// root the settings and shortcut files sit under and the service the
    /// gateway registers; reading the environment again here could disagree.
    pub fn assemble(app: &AppHandle, stage: String) -> tauri::Result<Self> {
        let config_root = local_data::config_root(app, &stage);
        let settings: Arc<dyn SettingsStore> = Arc::new(SettingsFile::at(config_root.clone()));
        // These fields own the service registration and its credential
        // namespace. A corrupt or unreadable authority cannot be replaced by
        // defaults without potentially starting and addressing another service.
        let durable = settings.load_service()?;
        let home = app.path().home_dir()?;
        let service_configuration = ServiceConfiguration::new(
            stage.clone(),
            durable.data_root.unwrap_or_else(|| home.join(".nessa")),
            durable.instance,
            durable
                .port
                .or_else(|| crate::stage_port::stage_port(&stage))
                .ok_or_else(|| {
                    tauri::Error::Io(io::Error::new(
                        io::ErrorKind::InvalidData,
                        format!("no gateway port is defined for stage {stage}"),
                    ))
                })?,
            durable.claude.configuration_directory,
        )
        .map_err(|error| tauri::Error::Io(io::Error::new(io::ErrorKind::InvalidData, error)))?;
        let service_namespace = service_namespace(
            Some(service_configuration.data_root().to_path_buf()),
            service_configuration.stage(),
            service_configuration.instance(),
        );
        let credential_save_audit: Arc<dyn CredentialSaveAudit> = app
            .path()
            .app_config_dir()
            .ok()
            .and_then(|trusted_root| {
                config_root.as_ref().and_then(|config_root| {
                    config_root
                        .strip_prefix(&trusted_root)
                        .ok()
                        .map(|relative| {
                            Arc::new(FileCredentialSaveAudit::beneath(
                                trusted_root,
                                relative.join("agent-credential-audit"),
                            )) as Arc<dyn CredentialSaveAudit>
                        })
                })
            })
            .unwrap_or_else(|| Arc::new(UnavailableCredentialSaveAudit));

        // A development build has no staged runtime to register, exactly as
        // before: the branch is on the profile, not on whether a path exists.
        let gateway = if cfg!(debug_assertions) {
            None
        } else {
            let runtime = app.path().resource_dir()?.join("runtime");
            let clock = Arc::new(SystemMonotonicClock);
            let reconciliation_audit =
                gateway::infrastructure::reconciliation_audit(config_root.clone(), clock.clone());
            Some(Arc::new(Gateway::bootstrap_with_dependencies(
                GatewayRuntimeDependencies::new(
                    gateway::infrastructure::current(
                        service_configuration.clone(),
                        home,
                        clock.clone(),
                    ),
                    gateway::infrastructure::login_shell_path(),
                    gateway::infrastructure::startup_events(app),
                    gateway::infrastructure::reconciliation_ids(),
                    reconciliation_audit,
                    clock,
                ),
                runtime,
                stage.clone(),
            )))
        };

        Ok(Self {
            settings,
            agent_credentials: Arc::new(LocalAgentCredentialStore::new(
                service_configuration.credential_namespace().clone(),
            )),
            credential_save_targets: Arc::new(CanonicalCredentialSaveTargets::new(
                service_configuration.credential_namespace().clone(),
            )),
            credential_save_ids: Arc::new(RandomCredentialSaveIds),
            credential_save_audit,
            shortcuts: Arc::new(ShortcutsFile::at(config_root)),
            credential: Arc::new(SurfaceCredential::for_namespace(
                service_namespace.clone(),
                stage.clone(),
            )),
            endpoint: Arc::new(GatewayEndpointAccess::new(
                stage,
                gateway_endpoint::infrastructure::endpoint_discovery(service_namespace),
            )),
            picker: attachments::file_picker(app),
            files: attachments::chosen_files(),
            types: attachments::content_types(),
            tickets: attachments::attachment_tickets(),
            readiness: attachments::readiness(),
            dragging: attachments::drag_board(),
            gateway,
            #[cfg(desktop)]
            releases: updater::release_source(app),
        })
    }
}

/// The bundle, for a handler the framework calls with only an app handle.
///
/// One of these at the top of an entry point is the whole of what a handler may
/// do to find its dependencies; everything it calls takes them as parameters.
/// `None` only in the window a failed `setup` leaves behind, and the callers
/// treat that the way the host treats every other startup failure: carry on
/// doing the part that still works.
pub fn resolve(app: &AppHandle) -> Option<HostDependencies> {
    app.try_state::<HostDependencies>()
        .map(|state| state.inner().clone())
}
