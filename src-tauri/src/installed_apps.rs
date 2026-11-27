//! Installed static apps on the desktop: the system's Yes/Cancel before an
//! install, and one window per open app.
//!
//! ```text
//! desktop page ──installed_app_confirm──▶ native dialog ──▶ Yes | Cancel
//! desktop page ──installed_app_open (package, digest)──▶ verify again
//!     ──▶ HeldApps (in memory, by window) ──▶ window `app-<id>` (installed-app.html)
//! app page ──installed_app_page──▶ the verified files its own window holds
//! desktop page ──installed_app_stop──▶ that window destroyed
//! window destroyed ──▶ held package dropped ──▶ `nessa://installed-apps-changed`
//! ```
//!
//! The gateway is the authority on what is installed; the desktop page reads
//! a package from it (`apps.read`) and hands it here to be shown. The host
//! verifies those bytes itself before any window opens, so a page cannot
//! hand an app window bytes other than a package whose digest it named. An
//! app window holds no gateway credential and may call nothing but
//! `installed_app_page`: the app's own code runs further in, in the sandbox
//! proxy's opaque frame (`app_sandbox.rs`), with no network.
//!
//! An app window outlives the desktop window's being closed: closing that
//! one only hides it (`desktop_window::on_close`).

use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

use nessa_app_package::{verify_inline_package, InlineAsset, InlinePackage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

use crate::composition::HostDependencies;
use crate::desktop_window::DESKTOP_WINDOW;
use crate::host;

/// Every app window's label starts with this, then the app's identity.
pub const APP_WINDOW_PREFIX: &str = "app-";

/// The page an app window loads, beside `desktop.html` in the bundle.
const APP_PAGE: &str = "installed-app.html";

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

/// What an app's window draws: verified bytes only.
#[derive(Clone, Debug, Serialize, PartialEq, Eq)]
#[serde(rename_all = "camelCase")]
pub struct AppPage {
    pub app_id: String,
    pub entrypoint: String,
    pub assets: Vec<HandedAsset>,
}

/// Why a package was not opened.
#[derive(Debug, PartialEq, Eq)]
pub enum OpenRefusal {
    /// The bytes do not verify against their manifest.
    Invalid,
    /// They verify, but not as the app or digest named.
    NotThePackageNamed,
}

/// The verified page each open app window draws, by app identity.
#[derive(Default)]
pub struct HeldApps {
    pages: Mutex<BTreeMap<String, AppPage>>,
}

impl HeldApps {
    /// Verify `package` and keep it for `app_id`'s window, replacing what that
    /// window held. Nothing is kept for a package that does not verify, or
    /// that is not the app and digest the caller named.
    pub fn hold(
        &self,
        app_id: &str,
        package: HandedPackage,
        digest: &str,
    ) -> Result<(), OpenRefusal> {
        let inline = InlinePackage {
            manifest_base64: package.manifest,
            assets: package
                .assets
                .iter()
                .map(|asset| InlineAsset {
                    path: asset.path.clone(),
                    bytes_base64: asset.bytes.clone(),
                })
                .collect(),
        };
        let verified = verify_inline_package(&inline).map_err(|_| OpenRefusal::Invalid)?;
        if verified.manifest().app_id().as_str() != app_id || verified.digest() != digest {
            return Err(OpenRefusal::NotThePackageNamed);
        }
        let page = AppPage {
            app_id: app_id.to_owned(),
            entrypoint: verified.manifest().entrypoint().to_owned(),
            assets: package.assets,
        };
        self.pages
            .lock()
            .map_err(|_| OpenRefusal::Invalid)?
            .insert(app_id.to_owned(), page);
        Ok(())
    }

    /// The page held for the app window labelled `label`.
    pub fn page_for(&self, label: &str) -> Option<AppPage> {
        let app = label.strip_prefix(APP_WINDOW_PREFIX)?;
        self.pages.lock().ok()?.get(app).cloned()
    }

    /// Forget the app window labelled `label`; true when it held one.
    pub fn release(&self, label: &str) -> bool {
        let Some(app) = label.strip_prefix(APP_WINDOW_PREFIX) else {
            return false;
        };
        self.pages
            .lock()
            .map(|mut pages| pages.remove(app).is_some())
            .unwrap_or(false)
    }

    /// The apps with a held page, in identity order.
    pub fn running(&self) -> Vec<String> {
        self.pages
            .lock()
            .map(|pages| pages.keys().cloned().collect())
            .unwrap_or_default()
    }
}

/// The window label for `app_id`, or none for one that is not an app identity.
pub fn window_label(app_id: &str) -> Option<String> {
    nessa_types::app_identity::ExtensionAppId::new(app_id)
        .ok()
        .map(|app| format!("{APP_WINDOW_PREFIX}{}", app.as_str()))
}

/// Whether a window came up new or was already there.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Shown {
    Opened,
    BroughtForward,
}

/// The window server and the system's dialogs, as installed apps need them.
/// Composition gives the native one (`native_app_windows`); tests stand in.
pub trait AppWindows: Send + Sync {
    /// Ask the person `question` with Install and Cancel, answering `answer`
    /// once with whether they chose Install (a dismissal is Cancel).
    fn ask_to_install(&self, question: String, answer: Box<dyn FnOnce(bool) + Send>);
    /// Show the app window `label`, titled `title`: open it on the app page,
    /// or reload and bring forward the one already open.
    fn show(&self, label: &str, title: &str) -> Result<Shown, String>;
    /// Close the window `label` for good; nothing when there is none.
    fn close(&self, label: &str) -> Result<(), String>;
    /// Tell the desktop page that app windows opened or closed.
    fn changed(&self);
}

/// Installed apps' windows on this desktop: what each holds, and the window
/// server that shows them.
pub struct InstalledAppWindows {
    held: HeldApps,
    windows: Arc<dyn AppWindows>,
}

impl InstalledAppWindows {
    pub fn new(windows: Arc<dyn AppWindows>) -> Self {
        Self {
            held: HeldApps::default(),
            windows,
        }
    }

    /// The person's Install or Cancel for the package `digest` names.
    pub async fn confirm(
        &self,
        app_id: &str,
        package_version: &str,
        digest: &str,
    ) -> Result<bool, String> {
        if window_label(app_id).is_none()
            || digest.len() != 64
            || !digest.bytes().all(|byte| byte.is_ascii_hexdigit())
        {
            return Err("not an app to install".into());
        }
        let short = &digest[..12];
        let (answer, asked) = tokio::sync::oneshot::channel();
        self.windows.ask_to_install(
            format!(
                "Install {app_id} {package_version}?\n\nPackage {short}… is the one you reviewed. It opens in its own window with no network access."
            ),
            Box::new(move |yes| {
                let _ = answer.send(yes);
            }),
        );
        Ok(asked.await.unwrap_or(false))
    }

    /// Hold `package` for `app_id`'s window once it verifies as the one
    /// `digest` names, then show that window. A window that could not open
    /// holds nothing.
    pub fn open(&self, app_id: &str, package: HandedPackage, digest: &str) -> Result<(), String> {
        let label = window_label(app_id).ok_or("not an app identity")?;
        self.held
            .hold(app_id, package, digest)
            .map_err(|refusal| match refusal {
                OpenRefusal::Invalid => "the package does not verify".to_owned(),
                OpenRefusal::NotThePackageNamed => "the package is not the one named".to_owned(),
            })?;
        match self.windows.show(&label, app_id) {
            Ok(Shown::Opened) => {
                self.windows.changed();
                Ok(())
            }
            Ok(Shown::BroughtForward) => Ok(()),
            Err(error) => {
                if self.held.release(&label) {
                    self.windows.changed();
                }
                Err(format!("could not open {app_id}: {error}"))
            }
        }
    }

    /// Close `app_id`'s window. What it held goes when the window does
    /// (`window_went`), so a window that would not close stays running.
    pub fn stop(&self, app_id: &str) -> Result<(), String> {
        let label = window_label(app_id).ok_or("not an app identity")?;
        self.windows
            .close(&label)
            .map_err(|error| format!("could not stop {app_id}: {error}"))
    }

    /// The window `label` went: drop what it held and say so.
    pub fn window_went(&self, label: &str) {
        if self.held.release(label) {
            self.windows.changed();
        }
    }

    pub fn running(&self) -> Vec<String> {
        self.held.running()
    }

    pub fn page_for(&self, label: &str) -> Option<AppPage> {
        self.held.page_for(label)
    }
}

/// The window server and dialogs Tauri talks to.
struct NativeAppWindows(AppHandle);

impl AppWindows for NativeAppWindows {
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

    fn show(&self, label: &str, title: &str) -> Result<Shown, String> {
        if let Some(open) = self.0.get_webview_window(label) {
            // Already open: draw the newly held bytes, and come forward.
            let _ = open.eval("location.reload()");
            let _ = open.show();
            let _ = open.set_focus();
            return Ok(Shown::BroughtForward);
        }
        WebviewWindowBuilder::new(&self.0, label, WebviewUrl::App(APP_PAGE.into()))
            .title(title)
            .inner_size(900.0, 640.0)
            .min_inner_size(320.0, 240.0)
            .center()
            .build()
            .map(|_| Shown::Opened)
            .map_err(|error| error.to_string())
    }

    fn close(&self, label: &str) -> Result<(), String> {
        match self.0.get_webview_window(label) {
            Some(open) => open.destroy().map_err(|error| error.to_string()),
            None => Ok(()),
        }
    }

    fn changed(&self) {
        let _ = self
            .0
            .emit_to(DESKTOP_WINDOW, host::INSTALLED_APPS_CHANGED, ());
    }
}

/// The native app windows, for composition.
pub fn native_app_windows(app: &AppHandle) -> Arc<InstalledAppWindows> {
    Arc::new(InstalledAppWindows::new(Arc::new(NativeAppWindows(
        app.clone(),
    ))))
}

fn only_desktop(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == DESKTOP_WINDOW {
        Ok(())
    } else {
        Err(format!("only the {DESKTOP_WINDOW} window may call this"))
    }
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
    let apps = deps.installed_apps.clone();
    apps.confirm(&app_id, &package_version, &digest).await
}

/// Open `app_id` in its own window, or bring its window forward, drawing
/// `package` once the host has verified it is the one `digest` names.
#[tauri::command]
pub fn installed_app_open(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
    package: HandedPackage,
    digest: String,
) -> Result<(), String> {
    only_desktop(&window)?;
    deps.installed_apps.open(&app_id, package, &digest)
}

/// Close `app_id`'s window; nothing when it has none.
#[tauri::command]
pub fn installed_app_stop(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
    app_id: String,
) -> Result<(), String> {
    only_desktop(&window)?;
    deps.installed_apps.stop(&app_id)
}

/// The apps with a window open now.
#[tauri::command]
pub fn installed_app_running(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<Vec<String>, String> {
    only_desktop(&window)?;
    Ok(deps.installed_apps.running())
}

/// What the calling app window was opened to draw.
#[tauri::command]
pub fn installed_app_page(
    window: WebviewWindow,
    deps: State<'_, HostDependencies>,
) -> Result<AppPage, String> {
    deps.installed_apps
        .page_for(window.label())
        .ok_or_else(|| "this window holds no app".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use nessa_app_package::scaffold_inline_package;

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
    fn only_the_named_verified_package_is_held_for_its_window() {
        let held = HeldApps::default();
        let (package, digest) = starter("notes");
        assert_eq!(
            held.hold("notes", package.clone(), &"0".repeat(64)),
            Err(OpenRefusal::NotThePackageNamed)
        );
        assert_eq!(
            held.hold("other", package.clone(), &digest),
            Err(OpenRefusal::NotThePackageNamed)
        );
        let mut tampered = package.clone();
        tampered.assets[0].bytes = "PHA+dGFtcGVyZWQ8L3A+".into();
        assert_eq!(
            held.hold("notes", tampered, &digest),
            Err(OpenRefusal::Invalid)
        );
        assert!(held.running().is_empty());

        held.hold("notes", package.clone(), &digest).unwrap();
        assert_eq!(held.running(), vec!["notes".to_owned()]);
        let page = held.page_for("app-notes").unwrap();
        assert_eq!(page.entrypoint, "index.html");
        assert_eq!(page.assets, package.assets);
        assert!(held.page_for("desktop").is_none());
        assert!(held.page_for("app-calendar").is_none());
    }

    #[test]
    fn a_destroyed_window_releases_only_its_own_app() {
        let held = HeldApps::default();
        for app in ["calendar", "notes"] {
            let (package, digest) = starter(app);
            held.hold(app, package, &digest).unwrap();
        }
        assert!(held.release("app-calendar"));
        assert!(!held.release("app-calendar"));
        assert!(!held.release("desktop"));
        assert_eq!(held.running(), vec!["notes".to_owned()]);
    }

    /// A window server that does what it is told, or refuses, and counts.
    #[derive(Default)]
    struct Windows {
        open: Mutex<Vec<String>>,
        refuse_open: Mutex<bool>,
        changes: Mutex<usize>,
        answer: Mutex<Option<bool>>,
        asked: Mutex<Vec<String>>,
    }

    impl AppWindows for Windows {
        fn ask_to_install(&self, question: String, answer: Box<dyn FnOnce(bool) + Send>) {
            self.asked.lock().unwrap().push(question);
            // None is the person dismissing the dialog without answering.
            match *self.answer.lock().unwrap() {
                Some(yes) => answer(yes),
                None => drop(answer),
            }
        }
        fn show(&self, label: &str, _title: &str) -> Result<Shown, String> {
            if *self.refuse_open.lock().unwrap() {
                return Err("no window server".into());
            }
            let mut open = self.open.lock().unwrap();
            if open.iter().any(|each| each == label) {
                return Ok(Shown::BroughtForward);
            }
            open.push(label.to_owned());
            Ok(Shown::Opened)
        }
        fn close(&self, label: &str) -> Result<(), String> {
            self.open.lock().unwrap().retain(|each| each != label);
            Ok(())
        }
        fn changed(&self) {
            *self.changes.lock().unwrap() += 1;
        }
    }

    fn host() -> (Arc<Windows>, InstalledAppWindows) {
        let windows = Arc::new(Windows::default());
        (windows.clone(), InstalledAppWindows::new(windows))
    }

    #[test]
    fn install_is_yes_only_when_the_person_chooses_install() {
        tokio::runtime::Builder::new_current_thread()
            .build()
            .unwrap()
            .block_on(install_is_yes_only_on_install());
    }

    async fn install_is_yes_only_on_install() {
        let (windows, apps) = host();
        let (_, digest) = starter("notes");
        *windows.answer.lock().unwrap() = Some(true);
        assert_eq!(apps.confirm("notes", "1.0.0", &digest).await, Ok(true));
        assert!(windows.asked.lock().unwrap()[0].contains(&digest[..12]));
        *windows.answer.lock().unwrap() = Some(false);
        assert_eq!(apps.confirm("notes", "1.0.0", &digest).await, Ok(false));
        // Dismissed, the dialog is a Cancel.
        *windows.answer.lock().unwrap() = None;
        assert_eq!(apps.confirm("notes", "1.0.0", &digest).await, Ok(false));
        // Nothing is asked about what is not an app and a digest.
        assert!(apps.confirm("nessa", "1.0.0", &digest).await.is_err());
        assert!(apps.confirm("notes", "1.0.0", "short").await.is_err());
        assert_eq!(windows.asked.lock().unwrap().len(), 3);
    }

    #[test]
    fn an_app_runs_from_its_window_opening_until_its_window_goes() {
        let (windows, apps) = host();
        let (package, digest) = starter("notes");
        apps.open("notes", package.clone(), &digest).unwrap();
        assert_eq!(apps.running(), vec!["notes".to_owned()]);
        assert_eq!(*windows.open.lock().unwrap(), vec!["app-notes".to_owned()]);
        // Opened again, the same window comes forward: no second window.
        apps.open("notes", package, &digest).unwrap();
        assert_eq!(windows.open.lock().unwrap().len(), 1);
        assert_eq!(*windows.changes.lock().unwrap(), 1);

        apps.stop("notes").unwrap();
        assert!(windows.open.lock().unwrap().is_empty());
        // The host hears the window go, and only then is the app stopped.
        assert_eq!(apps.running(), vec!["notes".to_owned()]);
        apps.window_went("app-notes");
        assert!(apps.running().is_empty());
        assert_eq!(*windows.changes.lock().unwrap(), 2);
    }

    #[test]
    fn a_window_that_could_not_open_holds_nothing() {
        let (windows, apps) = host();
        let (package, digest) = starter("notes");
        *windows.refuse_open.lock().unwrap() = true;
        assert!(apps.open("notes", package, &digest).is_err());
        assert!(apps.running().is_empty());
        assert!(apps.page_for("app-notes").is_none());
    }

    #[test]
    fn a_label_is_made_only_from_an_app_identity() {
        assert_eq!(window_label("notes").as_deref(), Some("app-notes"));
        assert_eq!(window_label("nessa"), None);
        assert_eq!(window_label("../desktop"), None);
    }
}
