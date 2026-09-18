//! Whether a newer Nessa has been published, and installing it if one has.
//!
//! The check is deliberately quiet. It runs once, in the background, after the
//! app is up, and its only way of reaching anybody is one extra item in the
//! tray menu. There is no dialog, no prompt, no toast: nothing here takes the
//! screen, so nothing here can land on top of first-run setup, which does.
//!
//! A check that does not complete is a normal outcome, not a fault. The machine
//! may be offline, the release endpoint may be down, a proxy may be in the way.
//! In every one of those cases the tray stays exactly as it was and the failure
//! goes to stderr with the app's other diagnostics — the person is running an
//! app that works, and an update they have not heard of is not news.
//!
//! ```text
//!   check_in_background ──spawn──▶ check ──▶ Checked ──offer──▶ Offer
//!                                              │                  │
//!                                   PendingUpdate            tray::offer_update
//!                                              │                  │
//!                                              └──install_and_restart◀── click
//! ```
//!
//! `Checked` is what one check found and `Offer` is what the tray does about
//! it; [`offer`] is the whole of the rule connecting them, kept pure so the
//! "say nothing" cases are testable without a network or a published release.

use std::sync::Mutex;

use tauri::{AppHandle, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::tray;

/// What one check found.
///
/// The plugin's `Result<Option<Update>, Error>` is reduced to this at the
/// boundary so the decision below branches on named outcomes rather than on an
/// error's text. [`Checked::Failed`] carries the failure only to log it; no
/// decision reads that string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// A published release is newer than the running build.
    Newer {
        /// The published version, as the release manifest announced it.
        version: String,
    },
    /// The running build is the latest published one.
    Current,
    /// The check did not complete: offline, no endpoint, an unreadable
    /// manifest, a signature that did not verify.
    Failed(String),
}

/// What the tray does about a finished check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Grow the menu by an item offering to install this version and restart.
    Install {
        /// The version the item names.
        version: String,
    },
    /// Change nothing. Whoever is using the app sees no sign a check happened.
    Nothing,
}

/// The one rule: an update the person can actually install is the only thing
/// worth saying. Being current says nothing, and a check that failed says
/// nothing either — there is no useful action behind "we could not ask".
pub fn offer(checked: &Checked) -> Offer {
    match checked {
        Checked::Newer { version } => Offer::Install {
            version: version.clone(),
        },
        Checked::Current | Checked::Failed(_) => Offer::Nothing,
    }
}

/// The update the tray item is offering, held until somebody clicks it.
///
/// Managed only once an update has actually been found, so the item and the
/// update it installs arrive together. The slot is emptied by the click that
/// starts the install, which is also what stops a second click from starting a
/// second download of the same release; a failed install puts it back.
struct PendingUpdate(Mutex<Option<Update>>);

/// Asks the release endpoint once, off the startup path.
///
/// Called from `main`'s `setup` after the tray exists, because the tray menu is
/// where the answer goes. It returns immediately: the request itself happens on
/// the async runtime, so a slow or hanging endpoint delays nothing on screen —
/// not the panel, and not first-run setup.
pub fn check_in_background(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        check(&app).await;
    });
}

/// One check, from the plugin's answer through to the tray.
async fn check(app: &AppHandle) {
    let found = match app.updater() {
        Ok(updater) => updater.check().await,
        // A misconfigured or unbuildable updater is the same kind of news as an
        // unreachable endpoint: there is no update to offer either way.
        Err(error) => Err(error),
    };

    let (checked, update) = match found {
        Ok(Some(update)) => (
            Checked::Newer {
                version: update.version.clone(),
            },
            Some(update),
        ),
        Ok(None) => (Checked::Current, None),
        Err(error) => (Checked::Failed(error.to_string()), None),
    };

    if let Checked::Failed(reason) = &checked {
        // Survivable, and on purpose: this is the offline case as much as it is
        // the broken-endpoint case, and neither is the person's problem.
        eprintln!("[nessa] could not check for an update: {reason}");
    }

    if let Offer::Install { version } = offer(&checked) {
        // `offer` only asks to install what the check actually returned, so the
        // update is here whenever this arm is.
        if let Some(update) = update {
            app.manage(PendingUpdate(Mutex::new(Some(update))));
            tray::offer_update(app, &version);
        }
    }
}

/// Downloads and installs the offered update, then comes back up on it.
///
/// Called by the tray item. Failures are reported and survivable: the app keeps
/// running on the version it has, and the item stays in the menu to be tried
/// again.
pub fn install_and_restart(app: &AppHandle) {
    let Some(pending) = app.try_state::<PendingUpdate>() else {
        return;
    };
    let Some(update) = pending.0.lock().ok().and_then(|mut slot| slot.take()) else {
        // Already installing. The download is not started twice.
        return;
    };

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        match update.download_and_install(|_, _| {}, || {}).await {
            // macOS and Linux install in place and leave the old binary
            // running; Windows hands over to an installer that exits this
            // process itself, so the restart below is never reached there.
            Ok(()) => app.restart(),
            Err(error) => {
                eprintln!("[nessa] could not install the update: {error}");
                if let Some(pending) = app.try_state::<PendingUpdate>() {
                    if let Ok(mut slot) = pending.0.lock() {
                        *slot = Some(update);
                    }
                }
            }
        }
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_newer_release_is_offered_by_version() {
        assert_eq!(
            offer(&Checked::Newer {
                version: "0.2.0".to_string()
            }),
            Offer::Install {
                version: "0.2.0".to_string()
            }
        );
    }

    #[test]
    fn the_latest_build_says_nothing() {
        assert_eq!(offer(&Checked::Current), Offer::Nothing);
    }

    #[test]
    fn a_check_that_did_not_complete_says_nothing() {
        // The offline case. A failed check is not an error to report on
        // screen — the tray must look identical to a check that found nothing.
        assert_eq!(
            offer(&Checked::Failed(
                "error sending request for url (https://github.com/...)".to_string()
            )),
            Offer::Nothing
        );
        assert_eq!(
            offer(&Checked::Failed(
                "Updater does not have any endpoints set.".to_string()
            )),
            Offer::Nothing
        );
    }
}
