//! Whether the host could put itself together, and what the person sees when it
//! could not (ADR 221).
//!
//! `setup` never returns an error to Tauri: Tauri panics on one, inside a
//! macOS callback that cannot unwind, and the process aborts with no message.
//! A refused setup instead opens the panel on one plain sentence and
//! **Try again**, with the reason behind **Details**.

use crate::host;
use std::{fmt, io};
use tauri::{AppHandle, Manager, State};

/// What setup could not do. Carried for **Details** only; nothing branches on
/// which one it was.
#[derive(Debug)]
pub enum StartupRefusal {
    /// A Tauri plugin would not register.
    Plugin {
        plugin: &'static str,
        detail: String,
    },
    /// The gateway's settings could not be read, so its port, instance and data
    /// root are unknown and must not be guessed.
    Settings(io::Error),
    /// The home directory could not be resolved.
    Home(String),
    /// No gateway port is defined for this stage.
    Port { stage: String },
    /// The gateway's settings describe a service that cannot exist.
    ServiceConfiguration(String),
    /// The app's bundled resources could not be found.
    Resources(String),
    /// The platform's standard folders could not be resolved.
    PlatformPaths(io::Error),
}

impl fmt::Display for StartupRefusal {
    fn fmt(&self, output: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Plugin { plugin, detail } => {
                write!(output, "the {plugin} plugin did not register: {detail}")
            }
            Self::Settings(error) => write!(output, "settings could not be read: {error}"),
            Self::Home(error) => write!(output, "the home folder is unknown: {error}"),
            Self::Port { stage } => {
                write!(output, "no gateway port is defined for stage {stage}")
            }
            Self::ServiceConfiguration(error) => {
                write!(output, "the gateway settings are not valid: {error}")
            }
            Self::Resources(error) => {
                write!(output, "the app's resources could not be found: {error}")
            }
            Self::PlatformPaths(error) => {
                write!(
                    output,
                    "the platform's folders could not be resolved: {error}"
                )
            }
        }
    }
}

/// Managed by `setup` on both of its paths, before it returns. The page's
/// `host_startup` is a synchronous command, so Tauri answers it on the main
/// thread after `setup` has run: the first question always has an answer.
pub enum HostStartup {
    Ready,
    Refused(StartupRefusal),
}

impl HostStartup {
    fn view(&self) -> host::HostStartup {
        match self {
            Self::Ready => host::HostStartup::Ready,
            Self::Refused(refusal) => host::HostStartup::Refused {
                details: refusal.to_string(),
            },
        }
    }
}

#[tauri::command]
pub fn host_startup(startup: State<'_, HostStartup>) -> host::HostStartup {
    startup.view()
}

/// **Try again** on the refusal: the whole host is put together again from the
/// start, which is the only retry a refused setup has.
#[tauri::command]
pub fn restart_nessa(app: AppHandle) {
    app.restart();
}

/// **Quit** on the refusal. With no tray, and a refusal that trying again may
/// not clear, the page has to offer a way out that needs no keyboard shortcut.
#[tauri::command]
pub fn quit_nessa(app: AppHandle) {
    app.exit(0);
}

/// Put the panel on screen for the refusal. Nothing else runs: no gateway, no
/// tray, no shortcut, because each of them needs what setup could not build.
/// Without a tray the panel has to be findable, so it joins the taskbar.
pub fn show_refusal(app: &AppHandle) {
    let Some(window) = app.get_webview_window(crate::panel::MAIN_WINDOW) else {
        eprintln!("[nessa] no panel to show the startup refusal in");
        return;
    };
    let shown = window
        .set_skip_taskbar(false)
        .and_then(|()| window.show())
        .and_then(|()| window.set_focus());
    if let Err(error) = shown {
        eprintln!("[nessa] could not show the startup refusal: {error}");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_refusal_reaches_the_page_as_its_details() {
        let refused = HostStartup::Refused(StartupRefusal::Settings(io::Error::new(
            io::ErrorKind::PermissionDenied,
            "local storage must be private and owned by the current OS user",
        )));
        assert_eq!(
            serde_json::to_value(refused.view()).unwrap(),
            serde_json::json!({
                "state": "refused",
                "details": "settings could not be read: local storage must be private and owned by the current OS user",
            })
        );
        assert_eq!(
            serde_json::to_value(HostStartup::Ready.view()).unwrap(),
            serde_json::json!({"state": "ready"})
        );
    }
}
