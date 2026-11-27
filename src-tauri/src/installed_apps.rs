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
use std::sync::Mutex;

use nessa_app_package::{verify_inline_package, InlineAsset, InlinePackage};
use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Emitter, Manager, State, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

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

fn only_desktop(window: &WebviewWindow) -> Result<(), String> {
    if window.label() == DESKTOP_WINDOW {
        Ok(())
    } else {
        Err(format!("only the {DESKTOP_WINDOW} window may call this"))
    }
}

/// An app window went: drop what it held and tell the desktop page.
pub fn window_destroyed(app: &AppHandle, label: &str) {
    let Some(held) = app.try_state::<HeldApps>() else {
        return;
    };
    if held.release(label) {
        let _ = app.emit_to(DESKTOP_WINDOW, host::INSTALLED_APPS_CHANGED, ());
    }
}

/// The system's Yes/Cancel for installing the reviewed package. Blocks a
/// worker thread, never the main one, until the person answers.
#[tauri::command]
pub async fn installed_app_confirm(
    window: WebviewWindow,
    app_id: String,
    package_version: String,
    digest: String,
) -> Result<bool, String> {
    use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
    only_desktop(&window)?;
    if window_label(&app_id).is_none() || digest.len() != 64 {
        return Err("not an app to install".into());
    }
    let short = &digest[..12];
    let (answer, asked) = tokio::sync::oneshot::channel();
    window
        .dialog()
        .message(format!(
            "Install {app_id} {package_version}?\n\nPackage {short}… is the one you reviewed. It opens in its own window with no network access."
        ))
        .title("Install app")
        .kind(MessageDialogKind::Info)
        .parent(&window)
        .buttons(MessageDialogButtons::OkCancelCustom(
            "Install".into(),
            "Cancel".into(),
        ))
        .show(move |yes| {
            let _ = answer.send(yes);
        });
    Ok(asked.await.unwrap_or(false))
}

/// Open `app_id` in its own window, or bring its window forward, drawing
/// `package` once the host has verified it is the one `digest` names.
#[tauri::command]
pub fn installed_app_open(
    window: WebviewWindow,
    held: State<'_, HeldApps>,
    app_id: String,
    package: HandedPackage,
    digest: String,
) -> Result<(), String> {
    only_desktop(&window)?;
    let label = window_label(&app_id).ok_or("not an app identity")?;
    held.hold(&app_id, package, &digest)
        .map_err(|refusal| match refusal {
            OpenRefusal::Invalid => "the package does not verify".to_owned(),
            OpenRefusal::NotThePackageNamed => "the package is not the one named".to_owned(),
        })?;
    let app = window.app_handle();
    if let Some(open) = app.get_webview_window(&label) {
        // Already open: draw the newly held bytes, and come forward.
        let _ = open.eval("location.reload()");
        let _ = open.show();
        let _ = open.set_focus();
        return Ok(());
    }
    let built = WebviewWindowBuilder::new(app, &label, WebviewUrl::App(APP_PAGE.into()))
        .title(&app_id)
        .inner_size(900.0, 640.0)
        .min_inner_size(320.0, 240.0)
        .center()
        .build();
    match built {
        Ok(_) => {
            let _ = app.emit_to(DESKTOP_WINDOW, host::INSTALLED_APPS_CHANGED, ());
            Ok(())
        }
        Err(error) => {
            held.release(&label);
            Err(format!("could not open {app_id}: {error}"))
        }
    }
}

/// Close `app_id`'s window; nothing when it has none.
#[tauri::command]
pub fn installed_app_stop(window: WebviewWindow, app_id: String) -> Result<(), String> {
    only_desktop(&window)?;
    let label = window_label(&app_id).ok_or("not an app identity")?;
    if let Some(open) = window.app_handle().get_webview_window(&label) {
        open.destroy()
            .map_err(|error| format!("could not stop {app_id}: {error}"))?;
    }
    Ok(())
}

/// The apps with a window open now.
#[tauri::command]
pub fn installed_app_running(
    window: WebviewWindow,
    held: State<'_, HeldApps>,
) -> Result<Vec<String>, String> {
    only_desktop(&window)?;
    Ok(held.running())
}

/// What the calling app window was opened to draw.
#[tauri::command]
pub fn installed_app_page(
    window: WebviewWindow,
    held: State<'_, HeldApps>,
) -> Result<AppPage, String> {
    held.page_for(window.label())
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

    #[test]
    fn a_label_is_made_only_from_an_app_identity() {
        assert_eq!(window_label("notes").as_deref(), Some("app-notes"));
        assert_eq!(window_label("nessa"), None);
        assert_eq!(window_label("../desktop"), None);
    }
}
