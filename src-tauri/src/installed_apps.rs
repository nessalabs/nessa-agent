//! Installed static apps on the desktop: the system's Install/Cancel before an
//! install, and each installed app as an app of its own.
//!
//! ```text
//! desktop page ──installed_app_confirm──▶ native dialog ──▶ Install | Cancel
//! desktop page ──installed_app_place (package, digest)──▶ verify again
//!     ──▶ launcher (app_launchers.rs): its own .app / folder, icon, menu entry
//! desktop page ──installed_app_open──▶ place, then run it: its own process
//!     (nessa-app-host), its own Dock or taskbar entry, no network
//! desktop page ──installed_app_stop──▶ that process ended
//! desktop page ──installed_app_remove──▶ launcher and menu entry gone
//! desktop page ──installed_app_keep_only (the gateway's list)──▶ any other
//!     launcher stopped and removed: one left by an uninstall that failed
//!     halfway, or made from another client
//! watcher ──running set changed──▶ `nessa://installed-apps-changed`
//! ```
//!
//! The gateway is the authority on what is installed; the desktop page reads
//! a package from it (`apps.read`) and hands it here. The host verifies those
//! bytes itself before writing a launcher, so a page cannot place bytes other
//! than a package whose digest it named. An app runs with no gateway
//! credential and no command to call, and keeps running with Nessa's main
//! window, or Nessa itself, closed.

use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use nessa_app_package::{verify_inline_package, InlineAsset, InlinePackage, VerifiedPackage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewWindow};

use crate::app_launchers::AppLaunchers;
use crate::composition::HostDependencies;
use crate::desktop_window::DESKTOP_WINDOW;
use crate::host;

/// How often the watcher looks at which apps are running. An app can be
/// opened from Finder, Launchpad or the menu, or quit from its own menu, and
/// none of those tell Nessa.
const WATCH_EVERY: Duration = Duration::from_millis(750);

/// A package as the desktop page hands it over.
#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HandedPackage {
    pub manifest: String,
    pub assets: Vec<HandedAsset>,
}

#[derive(Clone, Debug, Deserialize, Serialize, PartialEq, Eq)]
pub struct HandedAsset {
    pub path: String,
    pub bytes: String,
}

/// Why a package was not placed.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenRefusal {
    /// The bytes do not verify against their manifest.
    Invalid,
    /// They verify, but not as the app or digest named.
    NotThePackageNamed,
}

impl OpenRefusal {
    fn words(&self) -> String {
        match self {
            Self::Invalid => "the package does not verify".to_owned(),
            Self::NotThePackageNamed => "the package is not the one named".to_owned(),
        }
    }
}

/// `package`, verified as `app_id`'s with `digest`, or why not.
pub fn verify_handed(
    app_id: &str,
    package: HandedPackage,
    digest: &str,
) -> Result<VerifiedPackage, OpenRefusal> {
    let inline = InlinePackage {
        manifest_base64: package.manifest,
        assets: package
            .assets
            .into_iter()
            .map(|asset| InlineAsset {
                path: asset.path,
                bytes_base64: asset.bytes,
            })
            .collect(),
    };
    let verified = verify_inline_package(&inline).map_err(|_| OpenRefusal::Invalid)?;
    if verified.manifest().app_id().as_str() != app_id || verified.digest() != digest {
        return Err(OpenRefusal::NotThePackageNamed);
    }
    Ok(verified)
}

/// Whether `app_id` is an app identity at all.
fn is_app(app_id: &str) -> bool {
    nessa_types::app_identity::ExtensionAppId::new(app_id).is_ok()
}

/// The system's dialogs and the desktop page, as installed apps need them.
/// Composition gives the native one (`native_installed_apps`); tests stand in.
pub trait AppSystem: Send + Sync {
    /// Ask the person `question` with Install and Cancel, answering `answer`
    /// once with whether they chose Install (a dismissal is Cancel).
    fn ask_to_install(&self, question: String, answer: Box<dyn FnOnce(bool) + Send>);
    /// Tell the desktop page that apps started or stopped.
    fn changed(&self);
}

/// Where launchers are kept and what runs them: the computer's own
/// (`AppLaunchers`) in composition, a stand-in in tests.
pub trait Launchers: Send + Sync {
    /// Write `package`'s launcher; nothing when it already holds it.
    fn place(&self, package: &VerifiedPackage) -> Result<(), String>;
    /// Run `app`, or bring it forward; done once it runs.
    fn launch(&self, app: &str) -> Result<(), String>;
    /// End `app`'s process; nothing when it is not running.
    fn stop(&self, app: &str) -> Result<(), String>;
    /// Remove `app`'s launcher; refused while it runs, nothing when absent.
    fn remove(&self, app: &str) -> Result<(), String>;
    /// The apps with a launcher, in name order.
    fn placed(&self) -> Vec<String>;
    /// The apps running now, in name order.
    fn running(&self) -> Vec<String>;
}

/// Installed apps on this desktop: their launchers, and the system that asks.
pub struct InstalledApps {
    launchers: Arc<dyn Launchers>,
    system: Arc<dyn AppSystem>,
    /// One change to the launchers at a time, so a reconcile never removes a
    /// launcher placed while it ran.
    changing: Mutex<()>,
}

impl InstalledApps {
    pub fn new(launchers: Arc<dyn Launchers>, system: Arc<dyn AppSystem>) -> Self {
        Self {
            launchers,
            system,
            changing: Mutex::new(()),
        }
    }

    fn change(&self) -> std::sync::MutexGuard<'_, ()> {
        self.changing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The person's Install or Cancel for the package `digest` names.
    pub async fn confirm(
        &self,
        app_id: &str,
        package_version: &str,
        digest: &str,
    ) -> Result<bool, String> {
        if !is_app(app_id)
            || digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("not an app to install".into());
        }
        let short = &digest[..12];
        let (answer, asked) = tokio::sync::oneshot::channel();
        self.system.ask_to_install(
            format!(
                "Install {app_id} {package_version}?\n\nPackage {short}… is the one you reviewed. It opens as an app of its own, with no network access."
            ),
            Box::new(move |yes| {
                let _ = answer.send(yes);
            }),
        );
        Ok(asked.await.unwrap_or(false))
    }

    /// Write `app_id`'s launcher once `package` verifies as the one `digest`
    /// names, so it is in Applications or the menu.
    pub fn place(&self, app_id: &str, package: HandedPackage, digest: &str) -> Result<(), String> {
        let verified = verify_handed(app_id, package, digest).map_err(|refusal| refusal.words())?;
        let _change = self.change();
        self.launchers.place(&verified)
    }

    /// Place `app_id` as `place` does, then run it, or bring it forward.
    /// Done once it runs; an app that does not start is an error.
    pub fn open(&self, app_id: &str, package: HandedPackage, digest: &str) -> Result<(), String> {
        let verified = verify_handed(app_id, package, digest).map_err(|refusal| refusal.words())?;
        let _change = self.change();
        self.launchers.place(&verified)?;
        let launched = self.launchers.launch(app_id);
        self.system.changed();
        launched
    }

    /// End `app_id`'s process; nothing when it is not running.
    pub fn stop(&self, app_id: &str) -> Result<(), String> {
        if !is_app(app_id) {
            return Err("not an app identity".into());
        }
        let _change = self.change();
        let stopped = self.launchers.stop(app_id);
        self.system.changed();
        stopped
    }

    /// Remove `app_id`'s launcher, after it has stopped.
    pub fn remove(&self, app_id: &str) -> Result<(), String> {
        if !is_app(app_id) {
            return Err("not an app identity".into());
        }
        let _change = self.change();
        self.launchers.remove(app_id)
    }

    /// Stop and remove every launcher whose app is not in `installed`, the
    /// gateway's list read after the last change: an uninstall that failed
    /// halfway, or one made from another client, leaves no app behind. The
    /// apps removed, or the first refusal once every other was tried.
    pub fn keep_only(&self, installed: &[String]) -> Result<Vec<String>, String> {
        if let Some(odd) = installed.iter().find(|app| !is_app(app)) {
            return Err(format!("{odd} is not an app identity"));
        }
        let _change = self.change();
        let mut removed = Vec::new();
        let mut refused = None;
        for app in self.launchers.placed() {
            if installed.contains(&app) {
                continue;
            }
            let gone = self
                .launchers
                .stop(&app)
                .and_then(|()| self.launchers.remove(&app));
            match gone {
                Ok(()) => removed.push(app),
                Err(error) => {
                    refused.get_or_insert(error);
                }
            }
        }
        if !removed.is_empty() {
            self.system.changed();
        }
        match refused {
            Some(error) => Err(error),
            None => Ok(removed),
        }
    }

    pub fn running(&self) -> Vec<String> {
        self.launchers.running()
    }

    /// Tell the desktop page whenever the running set changes, for as long
    /// as Nessa runs.
    pub fn watch(self: &Arc<Self>) {
        let apps = Arc::downgrade(self);
        thread::Builder::new()
            .name("installed-apps-watch".into())
            .spawn(move || {
                let mut seen = Vec::new();
                while let Some(apps) = apps.upgrade() {
                    let now = apps.running();
                    if now != seen {
                        apps.system.changed();
                        seen = now;
                    }
                    drop(apps);
                    thread::sleep(WATCH_EVERY);
                }
            })
            .ok();
    }
}

/// The dialogs and desktop page Tauri talks to.
struct NativeAppSystem(AppHandle);

impl AppSystem for NativeAppSystem {
    fn ask_to_install(&self, question: String, answer: Box<dyn FnOnce(bool) + Send>) {
        use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
        let dialog = self
            .0
            .dialog()
            .message(question)
            .title("Install app")
            .kind(MessageDialogKind::Info)
            .buttons(MessageDialogButtons::OkCancelCustom(
                "Install".into(),
                "Cancel".into(),
            ));
        let dialog = match self.0.get_webview_window(DESKTOP_WINDOW) {
            Some(desktop) => dialog.parent(&desktop),
            None => dialog,
        };
        dialog.show(answer);
    }

    fn changed(&self) {
        let _ = self
            .0
            .emit_to(DESKTOP_WINDOW, host::INSTALLED_APPS_CHANGED, ());
    }
}

/// The native installed apps, for composition: launchers where this platform
/// keeps apps, watched while Nessa runs.
pub fn native_installed_apps(app: &AppHandle, stage: &str) -> Option<Arc<InstalledApps>> {
    let paths = app.path();
    let config = crate::local_data::config_root(app, stage)?;
    let launchers = AppLaunchers::native(
        &paths.home_dir().ok()?,
        &config,
        &paths.data_dir().ok()?,
        stage,
    )
    .ok()?;
    let apps = Arc::new(InstalledApps::new(
        Arc::new(launchers),
        Arc::new(NativeAppSystem(app.clone())),
    ));
    apps.watch();
    Some(apps)
}

fn only_desktop(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == DESKTOP_WINDOW {
        Ok(())
    } else {
        Err(format!("only the {DESKTOP_WINDOW} window may call this"))
    }
}

fn installed(deps: &HostDependencies) -> Result<Arc<InstalledApps>, String> {
    deps.installed_apps
        .clone()
        .ok_or_else(|| "apps can't be placed on this computer".into())
}

/// The system's Install/Cancel for installing the reviewed package.
#[tauri::command]
pub async fn installed_app_confirm(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
    package_version: String,
    digest: String,
) -> Result<bool, String> {
    only_desktop(&window)?;
    let apps = installed(&deps)?;
    apps.confirm(&app_id, &package_version, &digest).await
}

/// Run `work` off the main thread: placing copies a binary, Open waits for
/// the app to start and Stop for it to end.
async fn blocking<T: Send + 'static>(
    deps: &HostDependencies,
    work: impl FnOnce(&InstalledApps) -> Result<T, String> + Send + 'static,
) -> Result<T, String> {
    let apps = installed(deps)?;
    tauri::async_runtime::spawn_blocking(move || work(&apps))
        .await
        .map_err(|error| format!("could not finish: {error}"))?
}

/// Write `app_id`'s launcher for the verified `package`.
#[tauri::command]
pub async fn installed_app_place(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
    package: HandedPackage,
    digest: String,
) -> Result<(), String> {
    only_desktop(&window)?;
    blocking(&deps, move |apps| apps.place(&app_id, package, &digest)).await
}

/// Open `app_id` as an app of its own, or bring it forward; done once it runs.
#[tauri::command]
pub async fn installed_app_open(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
    package: HandedPackage,
    digest: String,
) -> Result<(), String> {
    only_desktop(&window)?;
    blocking(&deps, move |apps| apps.open(&app_id, package, &digest)).await
}

/// End `app_id`'s process; nothing when it is not running.
#[tauri::command]
pub async fn installed_app_stop(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
) -> Result<(), String> {
    only_desktop(&window)?;
    blocking(&deps, move |apps| apps.stop(&app_id)).await
}

/// Remove `app_id`'s launcher; it must have stopped.
#[tauri::command]
pub async fn installed_app_remove(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
) -> Result<(), String> {
    only_desktop(&window)?;
    blocking(&deps, move |apps| apps.remove(&app_id)).await
}

/// Stop and remove every launcher whose app the gateway no longer lists.
#[tauri::command]
pub async fn installed_app_keep_only(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_ids: Vec<String>,
) -> Result<Vec<String>, String> {
    only_desktop(&window)?;
    blocking(&deps, move |apps| apps.keep_only(&app_ids)).await
}

/// The apps running now.
#[tauri::command]
pub async fn installed_app_running(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<Vec<String>, String> {
    only_desktop(&window)?;
    blocking(&deps, |apps| Ok(apps.running())).await
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::app_launchers::Layout;
    use nessa_app_package::scaffold_inline_package;
    use std::sync::Mutex;

    fn starter(app: &str) -> (HandedPackage, String) {
        let inline = scaffold_inline_package(app.into(), "1.0.0".into()).unwrap();
        let digest = verify_inline_package(&inline).unwrap().digest();
        (
            HandedPackage {
                manifest: inline.manifest_base64,
                assets: inline
                    .assets
                    .into_iter()
                    .map(|asset| HandedAsset {
                        path: asset.path,
                        bytes: asset.bytes_base64,
                    })
                    .collect(),
            },
            digest,
        )
    }

    #[test]
    fn only_the_named_verified_package_passes() {
        let (package, digest) = starter("notes");
        assert_eq!(
            verify_handed("notes", package.clone(), &"0".repeat(64)).err(),
            Some(OpenRefusal::NotThePackageNamed)
        );
        assert_eq!(
            verify_handed("other", package.clone(), &digest).err(),
            Some(OpenRefusal::NotThePackageNamed)
        );
        let mut tampered = package.clone();
        let html = tampered
            .assets
            .iter_mut()
            .find(|asset| asset.path == "index.html")
            .unwrap();
        html.bytes = "PHA+dGFtcGVyZWQ8L3A+".into();
        assert_eq!(
            verify_handed("notes", tampered, &digest).err(),
            Some(OpenRefusal::Invalid)
        );
        assert_eq!(
            verify_handed("notes", package, &digest).unwrap().digest(),
            digest
        );
    }

    /// A dialog that answers as told, and a page that counts.
    #[derive(Default)]
    struct System {
        changes: Mutex<usize>,
        answer: Mutex<Option<bool>>,
        asked: Mutex<Vec<String>>,
    }

    impl AppSystem for System {
        fn ask_to_install(&self, question: String, answer: Box<dyn FnOnce(bool) + Send>) {
            self.asked.lock().unwrap().push(question);
            // None is the person dismissing the dialog without answering.
            match *self.answer.lock().unwrap() {
                Some(yes) => answer(yes),
                None => drop(answer),
            }
        }
        fn changed(&self) {
            *self.changes.lock().unwrap() += 1;
        }
    }

    fn host() -> (tempfile::TempDir, Arc<System>, InstalledApps) {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("nessa-app-host");
        std::fs::write(&built, b"host").unwrap();
        let launchers =
            AppLaunchers::new(Layout::Folder, dir.path().join("apps"), None, built, None);
        let system = Arc::new(System::default());
        let apps = InstalledApps::new(Arc::new(launchers), system.clone());
        (dir, system, apps)
    }

    #[test]
    fn install_is_yes_only_when_the_person_chooses_install() {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(install_is_yes_only_on_install());
    }

    async fn install_is_yes_only_on_install() {
        let (_dir, system, apps) = host();
        let (_, digest) = starter("notes");
        *system.answer.lock().unwrap() = Some(true);
        assert_eq!(apps.confirm("notes", "1.0.0", &digest).await, Ok(true));
        assert!(system.asked.lock().unwrap()[0].contains(&digest[..12]));
        *system.answer.lock().unwrap() = Some(false);
        assert_eq!(apps.confirm("notes", "1.0.0", &digest).await, Ok(false));
        // Dismissed, the dialog is a Cancel.
        *system.answer.lock().unwrap() = None;
        assert_eq!(apps.confirm("notes", "1.0.0", &digest).await, Ok(false));
        // Nothing is asked about what is not an app and a digest.
        assert!(apps.confirm("nessa", "1.0.0", &digest).await.is_err());
        assert!(apps.confirm("notes", "1.0.0", "short").await.is_err());
        assert_eq!(system.asked.lock().unwrap().len(), 3);
    }

    #[test]
    fn only_a_verified_package_gets_a_launcher() {
        let (dir, _system, apps) = host();
        let (package, digest) = starter("notes");
        assert!(apps
            .place("notes", package.clone(), &"0".repeat(64))
            .is_err());
        assert!(!dir.path().join("apps").join("notes").exists());
        apps.place("notes", package, &digest).unwrap();
        assert!(dir
            .path()
            .join("apps")
            .join("notes")
            .join("app.json")
            .is_file());
        apps.remove("notes").unwrap();
        assert!(!dir.path().join("apps").join("notes").exists());
        assert!(apps.remove("nessa").is_err(), "not an app identity");
    }

    /// Launchers kept in memory, which fail where told to.
    #[derive(Default)]
    struct Fake {
        placed: Mutex<Vec<String>>,
        running: Mutex<Vec<String>>,
        stuck: Mutex<Vec<String>>,
        fails_to_start: Mutex<bool>,
    }

    impl Launchers for Fake {
        fn place(&self, package: &VerifiedPackage) -> Result<(), String> {
            let app = package.manifest().app_id().as_str().to_owned();
            let mut placed = self.placed.lock().unwrap();
            if !placed.contains(&app) {
                placed.push(app);
                placed.sort();
            }
            Ok(())
        }
        fn launch(&self, app: &str) -> Result<(), String> {
            if *self.fails_to_start.lock().unwrap() {
                return Err(format!("{app} did not start"));
            }
            self.running.lock().unwrap().push(app.to_owned());
            Ok(())
        }
        fn stop(&self, app: &str) -> Result<(), String> {
            if self.stuck.lock().unwrap().iter().any(|stuck| stuck == app) {
                return Err(format!("{app} did not stop"));
            }
            self.running
                .lock()
                .unwrap()
                .retain(|running| running != app);
            Ok(())
        }
        fn remove(&self, app: &str) -> Result<(), String> {
            if self
                .running
                .lock()
                .unwrap()
                .iter()
                .any(|running| running == app)
            {
                return Err(format!("{app} is still open"));
            }
            self.placed.lock().unwrap().retain(|placed| placed != app);
            Ok(())
        }
        fn placed(&self) -> Vec<String> {
            self.placed.lock().unwrap().clone()
        }
        fn running(&self) -> Vec<String> {
            self.running.lock().unwrap().clone()
        }
    }

    fn faked() -> (Arc<Fake>, Arc<System>, InstalledApps) {
        let launchers = Arc::new(Fake::default());
        let system = Arc::new(System::default());
        let apps = InstalledApps::new(launchers.clone(), system.clone());
        (launchers, system, apps)
    }

    #[test]
    fn launchers_the_gateway_no_longer_lists_are_stopped_and_removed() {
        let (launchers, system, apps) = faked();
        for app in ["calendar", "notes", "todo"] {
            let (package, digest) = starter(app);
            apps.place(app, package, &digest).unwrap();
        }
        let (package, digest) = starter("notes");
        apps.open("notes", package, &digest).unwrap();
        let before = *system.changes.lock().unwrap();

        // Uninstalled elsewhere, or halfway here: only calendar is installed.
        assert_eq!(
            apps.keep_only(&["calendar".to_owned()]).unwrap(),
            ["notes", "todo"]
        );
        assert_eq!(launchers.placed(), ["calendar"]);
        assert!(launchers.running().is_empty());
        assert!(*system.changes.lock().unwrap() > before);
        // Nothing more to remove is nothing.
        assert!(apps.keep_only(&["calendar".to_owned()]).unwrap().is_empty());
        assert!(apps.keep_only(&["Not An App".to_owned()]).is_err());
    }

    #[test]
    fn one_app_that_will_not_stop_leaves_the_others_removed() {
        let (launchers, _system, apps) = faked();
        for app in ["calendar", "notes", "todo"] {
            let (package, digest) = starter(app);
            apps.open(app, package, &digest).unwrap();
        }
        launchers.stuck.lock().unwrap().push("notes".to_owned());
        let refused = apps.keep_only(&[]).unwrap_err();
        assert!(refused.contains("notes did not stop"), "{refused}");
        assert_eq!(launchers.placed(), ["notes"]);
    }

    #[test]
    fn an_app_that_does_not_start_is_an_open_that_failed() {
        let (launchers, _system, apps) = faked();
        *launchers.fails_to_start.lock().unwrap() = true;
        let (package, digest) = starter("notes");
        let refused = apps.open("notes", package, &digest).unwrap_err();
        assert!(refused.contains("did not start"), "{refused}");
        assert!(apps.running().is_empty());
    }
}
