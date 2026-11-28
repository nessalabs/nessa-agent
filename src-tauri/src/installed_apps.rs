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
//! desktop page ──installed_app_credential (a dynamic app's, from
//!     apps.issueCredential)──▶ kept in its launcher, this user's alone, with
//!     where the gateway publishes its endpoint
//! watcher ──running set changed──▶ `nessa://installed-apps-changed`
//! ```
//!
//! The gateway is the authority on what is installed; the desktop page reads
//! a package from it (`apps.read`) and hands it here. The host verifies those
//! bytes itself before writing a launcher, so a page cannot place bytes other
//! than a package whose digest it named. An app's page has no gateway
//! credential and no command to call, and keeps running with Nessa's main
//! window, or Nessa itself, closed. A dynamic app's own server, which its app
//! host runs, signs in with the app's credential (`nessa_app_package::
//! start_server`).

use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use nessa_app_package::{
    verify_inline_package, AppCredentialRecord, InlineAsset, InlinePackage, ServerRuntime,
    VerifiedPackage,
};
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
/// How long before a dynamic app's credential ends Settings issues a new one
/// on Open, so an app opened from the Dock later still signs in.
const RENEW_BEFORE: Duration = Duration::from_secs(30 * 24 * 60 * 60);

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
    /// Keep a dynamic app's credential `record` in its placed launcher.
    fn keep_credential(&self, app: &str, record: &[u8]) -> Result<(), String>;
    /// The credential record `app`'s launcher holds, if any.
    fn credential(&self, app: &str) -> Option<Vec<u8>>;
}

/// Where the gateway this Nessa runs publishes its endpoint, for a dynamic
/// app's server to find it as any Nessa client on this computer does.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GatewayPlace {
    pub stage: String,
    pub data_dir: Option<String>,
    pub instance: Option<String>,
}

/// The time of day, for when a kept credential ends.
pub trait WallClock: Send + Sync {
    /// Now, in Unix seconds; None when the clock cannot be read.
    fn now(&self) -> Option<u64>;
}

/// The system's own clock.
pub struct SystemClock;

impl WallClock for SystemClock {
    fn now(&self) -> Option<u64> {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .ok()
            .map(|since| since.as_secs())
    }
}

/// Installed apps on this desktop: their launchers, and the system that asks.
pub struct InstalledApps {
    launchers: Arc<dyn Launchers>,
    system: Arc<dyn AppSystem>,
    gateway: GatewayPlace,
    clock: Arc<dyn WallClock>,
    /// One change to the launchers at a time, so a reconcile never removes a
    /// launcher placed while it ran.
    changing: Mutex<()>,
}

impl InstalledApps {
    pub fn new(
        launchers: Arc<dyn Launchers>,
        system: Arc<dyn AppSystem>,
        gateway: GatewayPlace,
        clock: Arc<dyn WallClock>,
    ) -> Self {
        Self {
            launchers,
            system,
            gateway,
            clock,
            changing: Mutex::new(()),
        }
    }

    fn change(&self) -> std::sync::MutexGuard<'_, ()> {
        self.changing.lock().unwrap_or_else(PoisonError::into_inner)
    }

    /// The person's Install or Cancel for `package`, once it verifies as
    /// the one `digest` names. What the dialog says the app does is read
    /// from those verified bytes, never from what the page says of them.
    pub async fn confirm(
        &self,
        app_id: &str,
        package: HandedPackage,
        digest: &str,
    ) -> Result<bool, String> {
        let verified = verify_handed(app_id, package, digest).map_err(|refusal| refusal.words())?;
        let manifest = verified.manifest();
        let capabilities: Vec<String> = manifest
            .requested_capabilities()
            .iter()
            .map(|capability| capability.as_str().to_owned())
            .collect();
        let allowed = capability_sentence(&capabilities)?;
        let runs = match manifest.server().map(|server| server.runtime()) {
            None => "It opens as an app of its own, with no network access.".to_owned(),
            Some(runtime) => {
                let name = match runtime {
                    ServerRuntime::Node => "Node.js",
                    ServerRuntime::Bun => "Bun",
                };
                format!(
                    "It opens as an app of its own, and runs its own program with {name} while it is open. That program can use this computer as you can: your files and the network."
                )
            }
        };
        let version = manifest.package_version();
        let short = &digest[..12];
        let (answer, asked) = tokio::sync::oneshot::channel();
        self.system.ask_to_install(
            format!(
                "Install {app_id} {version}?\n\nPackage {short}… is the one you reviewed. {runs}{allowed}"
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

    /// Keep `credential`, the one the gateway just issued `app_id` and that
    /// lasts until `expires_at`, in its launcher with where the gateway is,
    /// for its server to sign in with.
    pub fn hand_credential(
        &self,
        app_id: &str,
        credential: String,
        expires_at: Option<u64>,
    ) -> Result<(), String> {
        if !is_app(app_id) {
            return Err("not an app identity".into());
        }
        let record = AppCredentialRecord {
            credential,
            stage: self.gateway.stage.clone(),
            data_dir: self.gateway.data_dir.clone(),
            instance: self.gateway.instance.clone(),
            expires_at,
        };
        // What the server will refuse to read is refused here, unkept.
        let bytes = record
            .encode()
            .and_then(|bytes| AppCredentialRecord::decode(&bytes).map(|_| bytes))
            .map_err(|refusal| refusal.to_string())?;
        // Kept even while the app runs: issuing it already ended the one
        // before, so the next run must find this one. Settings asks before
        // issuing whether the app runs (`installed_app_running`).
        let _change = self.change();
        self.launchers.keep_credential(app_id, &bytes)
    }

    /// Whether `app_id`'s launcher holds a credential that still works for
    /// [`RENEW_BEFORE`] more: the desktop page issues a new one when not.
    /// A clock that cannot be read says not, so the page issues one.
    pub fn has_credential(&self, app_id: &str) -> bool {
        let Some(now) = self.clock.now() else {
            return false;
        };
        is_app(app_id)
            && self
                .launchers
                .credential(app_id)
                .and_then(|bytes| AppCredentialRecord::decode(&bytes).ok())
                .is_some_and(|record| record.lasts_past(now, RENEW_BEFORE.as_secs()))
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
pub fn native_installed_apps(
    app: &AppHandle,
    stage: &str,
    gateway: GatewayPlace,
) -> Option<Arc<InstalledApps>> {
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
        gateway,
        Arc::new(SystemClock),
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
    package: HandedPackage,
    digest: String,
) -> Result<bool, String> {
    only_desktop(&window)?;
    let apps = installed(&deps)?;
    apps.confirm(&app_id, package, &digest).await
}

/// What installing lets the app do in Nessa, as the Install dialog says it:
/// empty for an app that asks for nothing. A name the gateway does not
/// define is refused, so the dialog never leaves one out.
fn capability_sentence(capabilities: &[String]) -> Result<String, String> {
    let mut verbs = Vec::new();
    for capability in capabilities {
        let verb = match capability.as_str() {
            "list" => "list",
            "read" => "read",
            "create" => "start",
            "subscribe" => "follow",
            "stop" => "stop",
            "send" => "send messages in",
            _ => return Err("not an app to install".into()),
        };
        if verbs.contains(&verb) {
            return Err("not an app to install".into());
        }
        verbs.push(verb);
    }
    let Some((last, rest)) = verbs.split_last() else {
        return Ok(String::new());
    };
    let joined = if rest.is_empty() {
        (*last).to_owned()
    } else {
        format!("{} and {last}", rest.join(", "))
    };
    let runs = if capabilities
        .iter()
        .any(|capability| capability == "create" || capability == "send")
    {
        " Agents in them can read and change files on this computer."
    } else {
        ""
    };
    Ok(format!(
        "\n\nIt can also {joined} its own Nessa conversations.{runs} It never sees yours."
    ))
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

/// Keep the credential the gateway issued `app_id` in its launcher.
#[tauri::command]
pub async fn installed_app_credential(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
    credential: String,
    expires_at: Option<u64>,
) -> Result<(), String> {
    only_desktop(&window)?;
    blocking(&deps, move |apps| {
        apps.hand_credential(&app_id, credential, expires_at)
    })
    .await
}

/// Whether `app_id`'s launcher holds a credential that still works.
#[tauri::command]
pub async fn installed_app_has_credential(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
) -> Result<bool, String> {
    only_desktop(&window)?;
    blocking(&deps, move |apps| Ok(apps.has_credential(&app_id))).await
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
        starter_with(app, None)
    }

    fn starter_with(app: &str, server: Option<ServerRuntime>) -> (HandedPackage, String) {
        let inline = scaffold_inline_package(app.into(), "1.0.0".into(), server).unwrap();
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

    /// The time every test reads: 2026-11-28, as Unix seconds.
    const NOW: u64 = 1_795_852_800;

    struct Fixed(Option<u64>);

    impl WallClock for Fixed {
        fn now(&self) -> Option<u64> {
            self.0
        }
    }

    fn clock() -> Arc<dyn WallClock> {
        Arc::new(Fixed(Some(NOW)))
    }

    fn place() -> GatewayPlace {
        GatewayPlace {
            stage: "dev".into(),
            data_dir: Some(std::env::temp_dir().to_string_lossy().into_owned()),
            instance: None,
        }
    }

    fn host() -> (tempfile::TempDir, Arc<System>, InstalledApps) {
        host_at(clock())
    }

    fn host_at(clock: Arc<dyn WallClock>) -> (tempfile::TempDir, Arc<System>, InstalledApps) {
        let dir = tempfile::tempdir().unwrap();
        let built = dir.path().join("nessa-app-host");
        std::fs::write(&built, b"host").unwrap();
        let launchers =
            AppLaunchers::new(Layout::Folder, dir.path().join("apps"), None, built, None);
        let system = Arc::new(System::default());
        let apps = InstalledApps::new(Arc::new(launchers), system.clone(), place(), clock);
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
        let (package, digest) = starter("notes");
        *system.answer.lock().unwrap() = Some(true);
        assert_eq!(
            apps.confirm("notes", package.clone(), &digest).await,
            Ok(true)
        );
        let asked = system.asked.lock().unwrap()[0].clone();
        assert!(asked.contains(&digest[..12]), "{asked}");
        assert!(asked.starts_with("Install notes 1.0.0?"), "{asked}");
        assert!(asked.contains("with no network access."), "{asked}");
        *system.answer.lock().unwrap() = Some(false);
        assert_eq!(
            apps.confirm("notes", package.clone(), &digest).await,
            Ok(false)
        );
        // Dismissed, the dialog is a Cancel.
        *system.answer.lock().unwrap() = None;
        assert_eq!(
            apps.confirm("notes", package.clone(), &digest).await,
            Ok(false)
        );
        // Nothing is asked about bytes that are not the package named.
        assert!(apps
            .confirm("other", package.clone(), &digest)
            .await
            .is_err());
        assert!(apps
            .confirm("notes", package, &"0".repeat(64))
            .await
            .is_err());
        assert_eq!(system.asked.lock().unwrap().len(), 3);
    }

    #[test]
    fn the_dialog_names_what_an_app_may_do_and_nothing_unknown() {
        let asks = |names: &[&str]| {
            names
                .iter()
                .map(|name| (*name).to_owned())
                .collect::<Vec<_>>()
        };
        assert_eq!(capability_sentence(&[]), Ok(String::new()));
        assert!(capability_sentence(&asks(&["list", "create", "send"]))
            .unwrap()
            .ends_with(
                "It can also list, start and send messages in its own Nessa conversations. Agents in them can read and change files on this computer. It never sees yours."
            ));
        assert!(capability_sentence(&asks(&["read"]))
            .unwrap()
            .ends_with("It can also read its own Nessa conversations. It never sees yours."));
        assert!(capability_sentence(&asks(&["everything"])).is_err());
        assert!(capability_sentence(&asks(&["read", "read"])).is_err());
    }

    #[test]
    fn a_clock_that_cannot_be_read_has_the_credential_issued_again() {
        let (_dir, _system, apps) = host_at(Arc::new(Fixed(None)));
        let (package, digest) = starter_with("notes", Some(ServerRuntime::Node));
        apps.place("notes", package, &digest).unwrap();
        apps.hand_credential("notes", "secret".into(), Some(NOW + 365 * 24 * 60 * 60))
            .unwrap();
        assert!(!apps.has_credential("notes"));
    }

    #[test]
    fn a_dynamic_app_says_it_runs_a_program_and_keeps_its_credential_private() {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(a_dynamic_app_says_so());
        let (dir, _system, apps) = host();
        // No launcher, nowhere to keep it.
        assert!(apps
            .hand_credential("notes", "secret".into(), None)
            .is_err());
        let (package, digest) = starter_with("notes", Some(ServerRuntime::Node));
        apps.place("notes", package.clone(), &digest).unwrap();
        assert!(!apps.has_credential("notes"));
        // One that ends within a month is due a new one.
        let month = RENEW_BEFORE.as_secs();
        apps.hand_credential("notes", "soon".into(), Some(NOW + month))
            .unwrap();
        assert!(!apps.has_credential("notes"));
        apps.hand_credential("notes", "later".into(), Some(NOW + month + 1))
            .unwrap();
        assert!(apps.has_credential("notes"));
        apps.hand_credential("notes", "secret".into(), Some(NOW + 365 * 24 * 60 * 60))
            .unwrap();
        assert!(apps.has_credential("notes"));
        let kept = dir
            .path()
            .join("apps")
            .join("notes")
            .join("credential.json");
        let record = AppCredentialRecord::decode(&std::fs::read(&kept).unwrap()).unwrap();
        assert_eq!(record.credential, "secret");
        assert_eq!(record.stage, "dev");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&kept).unwrap().permissions().mode();
            assert_eq!(mode & 0o777, 0o600);
        }
        // A newer one replaces it; one no server could read is refused.
        apps.hand_credential("notes", "newer".into(), None).unwrap();
        let record = AppCredentialRecord::decode(&std::fs::read(&kept).unwrap()).unwrap();
        assert_eq!(record.credential, "newer");
        assert!(apps.hand_credential("notes", "a\nb".into(), None).is_err());
        // Placed again with another host, the launcher keeps it.
        std::fs::write(dir.path().join("nessa-app-host"), b"a newer host").unwrap();
        apps.place("notes", package, &digest).unwrap();
        let record = AppCredentialRecord::decode(&std::fs::read(&kept).unwrap()).unwrap();
        assert_eq!(record.credential, "newer");
        // Placed again as a static app, it keeps none: nothing would use it.
        let (plain, plain_digest) = starter("notes");
        apps.place("notes", plain, &plain_digest).unwrap();
        assert!(!kept.exists());
        assert!(!apps.has_credential("notes"));
        assert!(!apps.has_credential("Not An App"));
    }

    async fn a_dynamic_app_says_so() {
        let (_dir, system, apps) = host();
        let (package, digest) = starter_with("notes", Some(ServerRuntime::Bun));
        *system.answer.lock().unwrap() = Some(true);
        assert_eq!(apps.confirm("notes", package, &digest).await, Ok(true));
        let asked = system.asked.lock().unwrap()[0].clone();
        assert!(
            asked.contains("runs its own program with Bun while it is open. That program can use this computer as you can: your files and the network."),
            "{asked}"
        );
        assert!(
            asked.contains(
                "It can also list, read, start and send messages in its own Nessa conversations."
            ),
            "{asked}"
        );
        assert!(!asked.contains("no network access"), "{asked}");
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
        fn keep_credential(&self, _app: &str, _record: &[u8]) -> Result<(), String> {
            Ok(())
        }
        fn credential(&self, _app: &str) -> Option<Vec<u8>> {
            None
        }
    }

    fn faked() -> (Arc<Fake>, Arc<System>, InstalledApps) {
        let launchers = Arc::new(Fake::default());
        let system = Arc::new(System::default());
        let apps = InstalledApps::new(launchers.clone(), system.clone(), place(), clock());
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
