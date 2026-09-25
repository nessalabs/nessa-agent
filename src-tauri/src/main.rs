// The release build is a menu bar app with no console window on Windows.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod agent_credentials;
mod attachments;
mod composition;
mod diagnostics;
mod gateway;
mod gateway_endpoint;
mod host;
mod launch;
mod links;
mod local_data;
mod panel;
mod platform;
mod settings;
mod shortcut;
mod shortcuts;
// Read by the launchd registration alone, which is the macOS gateway adapter:
// the table it consults says which loopback port a stage's background service
// gets, and no other platform registers one yet. Gated to match that adapter
// rather than carried everywhere and unused, which `-D warnings` calls dead on
// the platforms that never reach it.
mod stage_port;
mod surface_credential;
mod tray;
mod updater;

use composition::HostDependencies;
use gateway::application::Gateway;
use settings::SettingsStore;
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};

use tauri::{Manager, WindowEvent};

fn main() {
    if let Err(error) = launch::accept(std::env::args_os().skip(1)) {
        eprintln!("[nessa] {error}");
        std::process::exit(2);
    }

    let stage = match local_data::process_stage() {
        Ok(stage) => stage,
        Err(error) => {
            eprintln!("[nessa] {error}");
            std::process::exit(1);
        }
    };

    // Before Tauri (and GTK) start. Linux uses this to disable WebKit's
    // DMA-BUF renderer when there is no DRM device.
    platform::current().prepare();

    tauri::Builder::default()
        .plugin(tauri_plugin_global_shortcut::Builder::new().build())
        .plugin(diagnostics::init())
        // Before any window exists, so it covers the panel `tauri.conf.json`
        // declares as well as the setup window built later: a link clicked in
        // a Nessa window goes to the person's browser, and never turns the
        // floating bar into a web page.
        .plugin(links::init())
        .invoke_handler(tauri::generate_handler![
            platform::set_frosted,
            platform::panel_size,
            platform::flush_compositor,
            panel::finish_setup,
            panel::retry_setup_record,
            panel::chosen_agent,
            panel::reveal_setup_window,
            agent_credentials::infrastructure::save_agent_api_key,
            gateway::infrastructure::gateway_startup,
            gateway::infrastructure::retry_gateway_startup,
            surface_credential::load_surface_credential,
            gateway_endpoint::entrypoint::command::load_gateway_endpoint,
            shortcuts::load_shortcuts,
            shortcuts::apply_shortcuts,
            updater::available_update,
            updater::install_update,
            attachments::choose_attachment_files,
            attachments::read_attachment_bytes,
        ])
        .setup(move |app| {
            // Registered here rather than in the builder chain because there is
            // nothing to update on a phone: an installed iOS or Android app is
            // replaced by its store, not by itself. This is the plugin's own
            // documented placement for that reason.
            #[cfg(desktop)]
            app.handle()
                .plugin(tauri_plugin_updater::Builder::new().build())?;

            // Registered here for the same reason, and with the same shape: a
            // phone hands an app a document through its own share sheet rather
            // than a path somebody browsed to, so attaching a file by path is a
            // desktop question. Registered before the bundle is assembled,
            // because the picker composition builds asks this plugin to put the
            // dialog on screen. The webview is granted nothing by this — it calls
            // `choose_attachment_files`, which is Nessa's own command, and the
            // host calls the plugin.
            #[cfg(desktop)]
            app.handle().plugin(tauri_plugin_dialog::init())?;

            // The composition root. Every outside thing the host talks to is
            // built here, once, and handed down from here: the settings file,
            // the shortcut cache, the surface credential, the background
            // service, and the release endpoint. Managed as one value so the
            // commands below can declare `State<HostDependencies>` and be given
            // it, and kept here so the rest of `setup` can pass it by hand.
            let deps = HostDependencies::assemble(app.handle(), stage.clone())?;
            app.manage(deps.clone());

            // The host owns packaged gateway startup. Start it independently of
            // either webview so setup can subscribe to progress before a
            // credential request, and keep setup/window presentation unblocked.
            if let Some(gateway) = deps.gateway.clone() {
                tauri::async_runtime::spawn(async move {
                    if let Err(error) = gateway.start().await {
                        eprintln!("[nessa] gateway startup failed: {error}");
                    }
                });
            }

            platform::current().configure_app(app.handle());

            // A missing tray is survivable. On Linux especially, GNOME without
            // an app-indicator extension has no tray at all; the panel then
            // has to be reachable from the taskbar.
            let tray_present = match tray::create(app.handle(), &deps) {
                Ok(()) => true,
                Err(error) => {
                    eprintln!("[nessa] could not create the tray: {error}");
                    if let Some(window) = app.get_webview_window("main") {
                        let _ = window.set_skip_taskbar(false);
                    }
                    false
                }
            };
            app.manage(tray::Present(tray_present));

            let settings = deps.settings.load();
            let shortcut_doc = deps.shortcuts.load();
            let summon = shortcuts::SummonRegistration(Mutex::new(None));
            shortcut::reregister_summon(
                app.handle(),
                &summon,
                shortcuts::summon_accelerator(&shortcut_doc),
            );
            app.manage(summon);

            if let Some(window) = app.get_webview_window("main") {
                if let Err(error) = panel::apply_configured_size(&window, &settings) {
                    eprintln!("[nessa] could not size the panel: {error}");
                }
                platform::bind_window(&window, &settings);
            }

            // First run only: setup records that it finished, and a launch that
            // reads that record opens straight into the panel. The settings
            // loaded above are the same ones the panel is sized from — the file
            // is read once per launch, not once per question.
            //
            // Setup is a takeover: it covers the screen, menu bar included, and
            // is the active window when it does. A release page reveals it once
            // it has a frame to show; a debug host reveals its static loading
            // fallback too, so broken JavaScript cannot leave first run hidden.
            // See `panel::open_setup_window` and `panel::reveal_setup_window`.
            if !settings.onboarding.completed {
                panel::open_setup_window(app.handle());
            } else {
                // A debug executable is also a supported way to exercise the
                // real embedded desktop without `tauri dev`. Reveal it after
                // setup has sized and bound the configured panel. The static
                // document remains visible if the application bundle cannot
                // replace its loading fallback.
                #[cfg(all(debug_assertions, feature = "custom-protocol"))]
                if let Some(window) = app.get_webview_window(panel::MAIN_WINDOW) {
                    if let Err(error) = panel::show(&window, &settings) {
                        eprintln!("[nessa] could not reveal the debug panel: {error}");
                    }
                }
            }

            // Last, and on purpose. It must not delay anything above it, so it
            // is spawned rather than awaited and every window on screen is
            // already placed before it starts. It is safe next to first-run
            // setup for the same reason it is quiet in general: finding an
            // update puts a notice inside the panel and nothing else — no
            // window, no focus change, no prompt — and setup is a different
            // window, so setup keeps the screen it took whether the check
            // succeeds, finds nothing, or fails. A panel that is closed, or
            // whose page has not loaded yet, is not a missed announcement
            // either: the host keeps it and the panel asks on mount.
            #[cfg(desktop)]
            updater::check_in_background(app.handle(), deps.releases.clone());

            // The panel reads these on every show, to re-fit the frame.
            app.manage(settings);

            Ok(())
        })
        .on_window_event(|window, event| {
            // Close-to-dismiss is the panel's policy alone. Setup closes its own
            // window to finish, and a hidden, undestroyed setup window would
            // leave the panel lifted over the menu bar for the rest of the
            // session — the cleanup below only runs on an actual destroy.
            if window.label() == panel::MAIN_WINDOW {
                // The drag belongs to the host now: `dragDropEnabled` is on, so
                // the webview receives no drop of any kind and the paths only
                // exist here. See `attachments::dropping` for what that buys
                // and what it costs.
                if let WindowEvent::DragDrop(drag) = event {
                    attachments::dropped_on_panel(window.app_handle(), drag);
                }
                if let WindowEvent::CloseRequested { api, .. } = event {
                    // With a tray, close dismisses the panel. Without one — Linux
                    // with no StatusNotifierItem — close has to end the process,
                    // or there is no quit path at all.
                    let tray = window
                        .app_handle()
                        .try_state::<tray::Present>()
                        .map(|state| state.0)
                        .unwrap_or(false);
                    if tray {
                        api.prevent_close();
                        let _ = window.hide();
                    }
                }
            }
            // The panel is lifted over setup while setup is on screen. If it is
            // still showing when setup goes, it would be left floating above
            // the menu bar for the rest of the session.
            //
            // A safety net, not the handoff's cleanup path: `panel::finish_setup`
            // owns the finishing order and closes setup itself. This catches the
            // ways setup can go that nothing coordinates — a dismissal, a
            // crashed webview — and is idempotent with the handoff's own close.
            if matches!(event, WindowEvent::Destroyed) && window.label() == panel::SETUP_WINDOW {
                if let Some(panel) = window.app_handle().get_webview_window(panel::MAIN_WINDOW) {
                    platform::current().set_above_overlay(&panel, false);
                }
            }
            platform::current().on_window_event(window, event);
        })
        .build(tauri::generate_context!())
        .expect("error while building Nessa")
        .run(|app, event| {
            if matches!(event, tauri::RunEvent::Exit) {
                // One resolution, at the entry point. The decision itself takes
                // what it needs as parameters and lives below.
                if let Some(deps) = composition::resolve(app) {
                    stop_agents_if_asked(&*deps.settings, deps.gateway.as_deref());
                }
            }
        });
}

/// The quit policy, on the way out.
///
/// Agents keep running after the desktop quits unless the settings file says
/// otherwise, and there is nothing to ask when this build has no registered
/// gateway — a development build, which never had one. The gateway is checked
/// first, so a launch with no service does not read the file to decide nothing.
///
/// A refused stop is survivable and reported: the app is already leaving, and
/// launchd owns the gateway's lifetime either way.
fn stop_agents_if_asked(settings: &dyn SettingsStore, gateway: Option<&Gateway>) {
    let Some(gateway) = gateway else {
        return;
    };
    if !settings.load().stop_agents_on_quit {
        return;
    }
    if let Err(error) = gateway.stop_agents(Instant::now() + Duration::from_secs(5)) {
        eprintln!("[nessa] could not request agent shutdown: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gateway::application::{
        GatewayError, GatewayHost, GatewayReconciliationAttempt, GatewayReconciliationIntent,
        GatewayReconciliationJournalSession, GatewayReconciliationProgress, GatewayStopSession,
        ReconciledGateway, ReconciliationHistoryFact,
    };
    use gateway::domain::value_objects::{
        AuditDeliveryReceipt, LifecycleCommandResult, LifecycleObservation,
        LifecycleObservationSource, SearchPath,
    };
    use settings::testing::in_memory;
    use std::path::Path;
    use std::sync::Arc;

    /// A background service host that records what it was asked to do, without
    /// launchd or a staged runtime anywhere near it.
    #[derive(Default)]
    struct FakeHost {
        calls: Mutex<Vec<&'static str>>,
        stop: Mutex<Option<GatewayError>>,
    }

    impl GatewayHost for FakeHost {
        fn register(
            &self,
            _: &Path,
            _: &str,
            _: Option<&SearchPath>,
            attempt: &GatewayReconciliationAttempt,
            progress: &dyn GatewayReconciliationProgress,
        ) -> Result<ReconciledGateway, GatewayError> {
            self.calls.lock().unwrap().push("register");
            let gateway = ReconciledGateway::new(
                "com.nessa.gateway".into(),
                "a".repeat(64),
                "550e8400-e29b-41d4-a716-446655440000".into(),
                "b".repeat(64),
                7,
                7420,
            );
            let target = gateway.audit_identity()?.target().clone();
            progress.intent_admitted(
                GatewayReconciliationIntent::new(attempt.clone(), target, None).expect("intent"),
            )?;
            for fact in [
                ReconciliationHistoryFact::ServiceDefinitionPublished,
                ReconciliationHistoryFact::ServiceDefinitionDurable,
                ReconciliationHistoryFact::BootstrapCommandRequested,
                ReconciliationHistoryFact::BootstrapCommandCompleted,
                ReconciliationHistoryFact::BootstrapCommandSucceeded,
            ] {
                progress.history_observed(fact);
            }
            Ok(gateway)
        }

        fn stop_agents(
            &self,
            session: &GatewayStopSession,
            journal: &dyn GatewayReconciliationJournalSession,
            plan: &AuditDeliveryReceipt,
        ) -> Result<LifecycleObservation, GatewayError> {
            self.calls.lock().unwrap().push("stop");
            if let Some(error) = self.stop.lock().unwrap().clone() {
                return Err(error);
            }
            let intended = session.request().intended().audit_identity()?;
            session.begin_proof()?;
            session.prove(intended.clone(), 1)?;
            session.claim(plan, &intended, 1)?;
            let command = LifecycleCommandResult::Accepted;
            session.command_result(command.clone())?;
            journal.effect_completion("stop-agents-on-desktop-quit", "signal-agents", &command)?;
            let observation = LifecycleObservation::new(2, Some(intended), true);
            journal.observation(
                &LifecycleObservationSource::Effect {
                    plan_id: "stop-agents-on-desktop-quit".into(),
                    step_id: "signal-agents".into(),
                },
                &observation,
            )?;
            session.fresh_observation(observation.clone())?;
            Ok(observation)
        }
    }

    impl FakeHost {
        fn calls(&self) -> Vec<&'static str> {
            self.calls.lock().unwrap().clone()
        }
    }

    /// A gateway that has already reconciled, so a stop has somewhere to go.
    fn reconciled_gateway(host: Arc<FakeHost>) -> Gateway {
        let gateway = Gateway::bootstrap(
            host,
            gateway::application::testing::system_login_shell(),
            gateway::application::testing::discard_startup_events(),
            gateway::application::testing::sequential_reconciliation_ids(),
            gateway::application::testing::discard_reconciliation_audit(),
            "/runtime".into(),
            "ci".into(),
        );
        tauri::async_runtime::block_on(gateway.start()).expect("the fake host registers");
        gateway
    }

    #[test]
    fn agents_are_asked_to_stop_only_when_the_settings_file_says_so() {
        let host = Arc::new(FakeHost::default());
        let gateway = reconciled_gateway(host.clone());
        let settings = in_memory();

        // The default is to leave background agents running.
        stop_agents_if_asked(&settings.store, Some(&gateway));
        assert_eq!(host.calls(), ["register"]);

        settings
            .store
            .update(&mut |chosen| chosen.stop_agents_on_quit = true)
            .expect("an absent file takes the change");
        stop_agents_if_asked(&settings.store, Some(&gateway));
        assert_eq!(host.calls(), ["register", "stop"]);
    }

    /// A development build has no registered service, and the file is not even
    /// read to decide that: there is nothing the answer could change.
    #[test]
    fn a_build_without_a_gateway_asks_nothing_and_reads_nothing() {
        let settings = in_memory();
        settings
            .store
            .update(&mut |chosen| chosen.stop_agents_on_quit = true)
            .expect("an absent file takes the change");
        // Any read from here on fails, which would turn into the defaults and
        // hide a file that was consulted when it should not have been.
        *settings.storage.read_error.lock().unwrap() = Some(std::io::ErrorKind::PermissionDenied);

        stop_agents_if_asked(&settings.store, None);

        assert_eq!(
            *settings.storage.read_error.lock().unwrap(),
            Some(std::io::ErrorKind::PermissionDenied),
            "the settings file was read on a launch with no gateway to stop"
        );
    }

    /// The app is already leaving. A refused stop is reported and nothing else:
    /// launchd owns the gateway's lifetime either way.
    #[test]
    fn a_refused_stop_does_not_hold_up_the_exit() {
        let host = Arc::new(FakeHost::default());
        *host.stop.lock().unwrap() = Some(GatewayError::Stop("delivery failed".into()));
        let gateway = reconciled_gateway(host.clone());
        let settings = in_memory();
        settings
            .store
            .update(&mut |chosen| chosen.stop_agents_on_quit = true)
            .expect("an absent file takes the change");

        stop_agents_if_asked(&settings.store, Some(&gateway));

        assert_eq!(host.calls(), ["register", "stop"]);
    }

    /// A settings file this build cannot parse must not read as "stop the
    /// agents": the defaults a failed load falls back to say to leave them
    /// running, which is the survivable answer on the way out.
    #[test]
    fn an_unreadable_settings_file_leaves_agents_running() {
        let host = Arc::new(FakeHost::default());
        let gateway = reconciled_gateway(host.clone());
        let settings = in_memory();
        settings.storage.put(&settings.path, b"{");

        stop_agents_if_asked(&settings.store, Some(&gateway));

        assert_eq!(host.calls(), ["register"]);
    }
}
