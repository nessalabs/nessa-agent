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
//!   check_in_background ──spawn──▶ run_check
//!                                   │      │
//!                     ReleaseSource ◀┘      └▶ CheckOutcome
//!                       │        │              │        │
//!              PluginReleases  fake       TrayOutcome   recorder
//!                       │                       │
//!                 PendingUpdate ──────▶ install_and_restart ◀── click
//! ```
//!
//! Both sides of a check are ports. The answer comes off the network and the
//! item goes onto a menu that needs a window server, so neither could be
//! reached from a test; behind traits, every outcome of a check — found,
//! current, refused — drives the real flow in [`tests`]. The arrows point at
//! the two implementations of each: the real one, and the one the tests use.
//!
//! `Checked` is what one check found and `Offer` is what the tray does about
//! it; [`offer`] is the whole of the rule connecting them, kept pure so the
//! "say nothing" cases are decided in one readable place.

use std::future::Future;
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

/// Whether the tray was already offering an update when a check started.
///
/// Asked as a fact, not acted on by whoever answers: an update found by an
/// earlier check is still in the menu and still installable, so a later check
/// finding the same release has nothing to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offering {
    /// The menu is the one the app started with.
    Nothing,
    /// An earlier check already put an update in the menu.
    AnUpdate,
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

/// The one rule: an update the person can actually install, and is not already
/// being offered one of, is the only thing worth saying. Being current says
/// nothing, and a check that failed says nothing either — there is no useful
/// action behind "we could not ask".
pub fn offer(checked: &Checked, offering: Offering) -> Offer {
    match (checked, offering) {
        (Checked::Newer { version }, Offering::Nothing) => Offer::Install {
            version: version.clone(),
        },
        (Checked::Newer { .. }, Offering::AnUpdate)
        | (Checked::Current | Checked::Failed(_), _) => Offer::Nothing,
    }
}

/// Where the question "is there a newer Nessa?" is asked.
///
/// The answer comes off the network, which is why this is a port: the real
/// implementation goes through the updater plugin, and a test substitutes one
/// that answers at once — including with the answers hardest to arrange for
/// real, such as an endpoint that refused.
///
/// It answers with [`Checked`] rather than the plugin's `Update`, which a test
/// cannot fabricate. Keeping the update off the port is also the honest
/// ownership: the release the tray item installs is the one the real source
/// found and kept, and no decision here ever reads it.
trait ReleaseSource {
    /// Asks once. A source that finds an update also retains it, so that the
    /// item offered below and the update a click installs are the same one.
    fn check(&self) -> impl Future<Output = Checked> + Send;
}

/// What a finished check is allowed to do: one tray item, one diagnostic line.
///
/// A port for the same reason as the source — the menu needs a window server
/// and the diagnostics are the process's stderr — and it reports the one fact
/// the rule needs rather than deciding anything with it.
trait CheckOutcome {
    /// Whether an update is in the menu already.
    fn offering(&self) -> Offering;

    /// Adds the item offering to install this version and restart.
    fn offer_update(&self, version: &str);

    /// Says, in the diagnostics only, that the check did not complete.
    fn report_failure(&self, reason: &str);
}

/// The real source: the release endpoint, through the updater plugin.
struct PluginReleases(AppHandle);

impl ReleaseSource for PluginReleases {
    fn check(&self) -> impl Future<Output = Checked> + Send {
        let app = self.0.clone();
        async move {
            let found = match app.updater() {
                Ok(updater) => updater.check().await,
                // A misconfigured or unbuildable updater is the same kind of
                // news as an unreachable endpoint: there is no update to offer
                // either way.
                Err(error) => Err(error),
            };

            match found {
                Ok(Some(update)) => {
                    let version = update.version.clone();
                    retain(&app, update);
                    Checked::Newer { version }
                }
                Ok(None) => Checked::Current,
                Err(error) => Checked::Failed(error.to_string()),
            }
        }
    }
}

/// The real outcome: the tray menu, and the app's stderr diagnostics.
struct TrayOutcome(AppHandle);

impl CheckOutcome for TrayOutcome {
    fn offering(&self) -> Offering {
        // The state exists from the moment a check found something, and the
        // menu item is added in the same breath. Its *slot* is emptied by a
        // click that starts an install, so emptiness would be the wrong
        // question here: the item is still in the menu while that runs.
        match self.0.try_state::<PendingUpdate>() {
            Some(_) => Offering::AnUpdate,
            None => Offering::Nothing,
        }
    }

    fn offer_update(&self, version: &str) {
        tray::offer_update(&self.0, version);
    }

    fn report_failure(&self, reason: &str) {
        // Survivable, and on purpose: this is the offline case as much as it is
        // the broken-endpoint case, and neither is the person's problem.
        eprintln!("[nessa] could not check for an update: {reason}");
    }
}

/// The update the tray item is offering, held until somebody clicks it.
///
/// Managed only once an update has actually been found, so the item and the
/// update it installs arrive together. The slot is emptied by the click that
/// starts the install, which is also what stops a second click from starting a
/// second download of the same release; a failed install puts it back.
struct PendingUpdate(Mutex<Option<Update>>);

/// Puts a found update where a later click will find it.
///
/// `manage` keeps the first value of a type and drops later ones, so the slot
/// is filled in place when it already exists — otherwise an update found after
/// an install failed would be silently thrown away.
fn retain(app: &AppHandle, update: Update) {
    if let Some(pending) = app.try_state::<PendingUpdate>() {
        if let Ok(mut slot) = pending.0.lock() {
            *slot = Some(update);
        }
        return;
    }
    app.manage(PendingUpdate(Mutex::new(Some(update))));
}

/// Asks the release endpoint once, off the startup path.
///
/// Called from `main`'s `setup` after the tray exists, because the tray menu is
/// where the answer goes. It returns immediately: the request itself happens on
/// the async runtime, so a slow or hanging endpoint delays nothing on screen —
/// not the panel, and not first-run setup.
pub fn check_in_background(app: &AppHandle) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_check(&PluginReleases(app.clone()), &TrayOutcome(app)).await;
    });
}

/// One check, from the source's answer through to the tray.
///
/// Everything outside the process is on one of the two ports, so this is the
/// whole of what a check does and all of it is exercised in [`tests`].
async fn run_check(source: &impl ReleaseSource, outcome: &impl CheckOutcome) {
    // Read before the check, not after: a source that finds an update retains
    // it, and what it retains is the same thing this asks about.
    let offering = outcome.offering();
    let checked = source.check().await;

    if let Checked::Failed(reason) = &checked {
        outcome.report_failure(reason);
    }

    if let Offer::Install { version } = offer(&checked, offering) {
        outcome.offer_update(&version);
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

    /// A release endpoint that has already made up its mind.
    ///
    /// Answers with whatever the test handed it, which is the point: "the
    /// machine is offline" and "the manifest did not verify" are one line here
    /// and a published release or an unplugged cable otherwise.
    struct FakeReleases(Checked);

    impl ReleaseSource for FakeReleases {
        fn check(&self) -> impl Future<Output = Checked> + Send {
            let checked = self.0.clone();
            async move { checked }
        }
    }

    /// A tray that writes down what it was asked to do instead of doing it.
    ///
    /// It also answers [`CheckOutcome::offering`] from what it has already been
    /// asked to offer, exactly as the real one answers from the update a check
    /// retained — so a second check meets the state the first one left.
    #[derive(Default)]
    struct RecordedOutcome {
        offered: Mutex<Vec<String>>,
        failures: Mutex<Vec<String>>,
    }

    impl RecordedOutcome {
        fn offered(&self) -> Vec<String> {
            self.offered.lock().expect("offers").clone()
        }

        fn failures(&self) -> Vec<String> {
            self.failures.lock().expect("failures").clone()
        }
    }

    impl CheckOutcome for RecordedOutcome {
        fn offering(&self) -> Offering {
            match self.offered.lock().expect("offers").is_empty() {
                true => Offering::Nothing,
                false => Offering::AnUpdate,
            }
        }

        fn offer_update(&self, version: &str) {
            self.offered
                .lock()
                .expect("offers")
                .push(version.to_string());
        }

        fn report_failure(&self, reason: &str) {
            self.failures
                .lock()
                .expect("failures")
                .push(reason.to_string());
        }
    }

    fn check(found: Checked) -> RecordedOutcome {
        let outcome = RecordedOutcome::default();
        tauri::async_runtime::block_on(run_check(&FakeReleases(found), &outcome));
        outcome
    }

    #[test]
    fn a_newer_release_is_offered_by_version() {
        assert_eq!(
            offer(
                &Checked::Newer {
                    version: "0.2.0".to_string()
                },
                Offering::Nothing
            ),
            Offer::Install {
                version: "0.2.0".to_string()
            }
        );
    }

    #[test]
    fn the_latest_build_says_nothing() {
        assert_eq!(offer(&Checked::Current, Offering::Nothing), Offer::Nothing);
    }

    #[test]
    fn a_check_that_did_not_complete_says_nothing() {
        // The offline case. A failed check is not an error to report on
        // screen — the tray must look identical to a check that found nothing.
        assert_eq!(
            offer(
                &Checked::Failed(
                    "error sending request for url (https://github.com/...)".to_string()
                ),
                Offering::Nothing
            ),
            Offer::Nothing
        );
        assert_eq!(
            offer(
                &Checked::Failed("Updater does not have any endpoints set.".to_string()),
                Offering::Nothing
            ),
            Offer::Nothing
        );
    }

    #[test]
    fn a_found_release_reaches_the_tray_once() {
        let outcome = check(Checked::Newer {
            version: "0.3.1".to_string(),
        });

        assert_eq!(outcome.offered(), vec!["0.3.1".to_string()]);
        assert!(outcome.failures().is_empty());
    }

    #[test]
    fn a_current_build_leaves_the_tray_alone() {
        let outcome = check(Checked::Current);

        assert!(outcome.offered().is_empty());
        assert!(outcome.failures().is_empty());
    }

    #[test]
    fn a_refused_check_offers_nothing_and_is_still_reported() {
        // Silent on screen, not silent in the diagnostics: an endpoint that is
        // never reachable would otherwise look exactly like being up to date.
        let outcome = check(Checked::Failed("connection refused".to_string()));

        assert!(outcome.offered().is_empty());
        assert_eq!(outcome.failures(), vec!["connection refused".to_string()]);
    }

    #[test]
    fn a_second_check_does_not_stack_a_second_item() {
        let outcome = RecordedOutcome::default();
        let found = FakeReleases(Checked::Newer {
            version: "0.3.1".to_string(),
        });

        tauri::async_runtime::block_on(async {
            run_check(&found, &outcome).await;
            run_check(&found, &outcome).await;
        });

        assert_eq!(outcome.offered(), vec!["0.3.1".to_string()]);
    }
}
