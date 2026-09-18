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
//!   composition ──release_source──▶ ReleaseSource ──┐
//!                                   │     │     │   │
//!                      PluginReleases  Simulated  fake
//!                                   │     │           │
//!   check_in_background ──spawn──▶ run_check ◀────────┘
//!                                        │
//!                                  CheckOutcome
//!                                   │         │
//!                            TrayOutcome    recorder
//!                                   │
//!        PendingUpdate  SimulatedUpdate ─▶ install_and_restart ◀── click
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
//!
//! `Simulated` is the third source and is compiled only into a debug build: it
//! answers "yes, `9.9.9`" from `NESSA_FAKE_UPDATE` without a network, so the
//! decision, the tray item, and the click can be watched in `pnpm app` with
//! nothing published. It cannot install anything and the click says so. The
//! *other* way to watch this locally — the real plugin, real HTTP, real
//! manifest parse, against a server on this machine — needs no code here at
//! all: it is a `--config` endpoint merge, `scripts/desktop/updater-harness.mjs
//! --check-only`, and `docs/codebase-structure.md` has both recipes.

#[cfg(debug_assertions)]
use std::env;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

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
pub trait ReleaseSource: Send + Sync {
    /// Asks once. A source that finds an update also retains it, so that the
    /// item offered below and the update a click installs are the same one.
    ///
    /// Boxed rather than `impl Future` because composition holds the chosen
    /// source behind a pointer: which one a build asks is a composition
    /// decision, and a trait a bundle can carry has to be object safe. Every
    /// implementation clones what it needs before the future starts, so the
    /// future borrows nothing from the source.
    fn check(&self) -> Pin<Box<dyn Future<Output = Checked> + Send>>;
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
    fn check(&self) -> Pin<Box<dyn Future<Output = Checked> + Send>> {
        let app = self.0.clone();
        Box::pin(async move {
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
        })
    }
}

/// The variable a debug build reads to pretend a release was published.
///
/// Set it to the version to announce — `NESSA_FAKE_UPDATE=9.9.9 pnpm app` — and
/// the check answers from it instead of asking the endpoint. It is the inner
/// loop for the decision, the tray item, and the click: no server, no artifact,
/// no signing key. Nothing it produces can be installed, and it says so.
#[cfg(debug_assertions)]
const SIMULATED_UPDATE: &str = "NESSA_FAKE_UPDATE";

/// What [`SIMULATED_UPDATE`] asked a debug build to do.
///
/// Decided here, away from the environment and the app handle, so the two ways
/// of getting the real endpoint back — not setting the variable, and setting it
/// to something that is not a version — are one readable rule with a test each.
#[cfg(debug_assertions)]
#[derive(Debug, Clone, PartialEq, Eq)]
enum Simulated {
    /// Announce this version. The release endpoint is not asked at all.
    Announce(String),
    /// A value was set that is not a version. The real endpoint answers, and
    /// the caller says on stderr that the value was ignored — a silent
    /// fall-through would read as "the simulation is broken".
    Ignored(String),
    /// Nothing was asked for: an ordinary debug run.
    Off,
}

/// The one rule for the variable: a `major.minor.patch` of digits is a version
/// to announce, anything else set is a typo worth reporting, and unset is a
/// normal run. Deliberately stricter than "non-empty" — the string goes
/// straight into the tray item's text, and "Update to banana" is not honest
/// about what a real check would ever produce.
#[cfg(debug_assertions)]
fn simulated(requested: Option<&str>) -> Simulated {
    let Some(requested) = requested else {
        return Simulated::Off;
    };
    let version = requested.trim();
    if version.is_empty() {
        return Simulated::Off;
    }

    let parts: Vec<&str> = version.split('.').collect();
    let numbered = parts.len() == 3
        && parts
            .iter()
            .all(|part| !part.is_empty() && part.bytes().all(|byte| byte.is_ascii_digit()));

    match numbered {
        true => Simulated::Announce(version.to_string()),
        false => Simulated::Ignored(version.to_string()),
    }
}

/// A source that answers from [`SIMULATED_UPDATE`] without a network.
///
/// Separate from the `FakeReleases` the tests use, and not the same thing: that
/// one replays any [`Checked`] a test hands it, including failures, and retains
/// nothing because no click ever follows it. This one exists to drive a running
/// app, so it does what the real source does — answer, and record what a later
/// click will find — and what it records is that there is nothing to install.
/// Widening the test double's `cfg` would put a test fixture in the dev binary
/// and still leave that second job undone.
#[cfg(debug_assertions)]
struct SimulatedReleases {
    app: AppHandle,
    version: String,
}

#[cfg(debug_assertions)]
impl ReleaseSource for SimulatedReleases {
    fn check(&self) -> Pin<Box<dyn Future<Output = Checked> + Send>> {
        let app = self.app.clone();
        let version = self.version.clone();
        Box::pin(async move {
            // The real source retains the update a click installs; this retains
            // the fact that no such update exists, which is what makes the
            // click able to say so instead of doing nothing.
            app.manage(SimulatedUpdate(version.clone()));
            eprintln!(
                "[nessa] {SIMULATED_UPDATE}={version}: offering a simulated update. \
                 The release endpoint was not asked and nothing was downloaded."
            );
            Checked::Newer { version }
        })
    }
}

/// Managed when a simulated update is offered, so the click can be honest.
#[cfg(debug_assertions)]
struct SimulatedUpdate(String);

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

/// Which release source this build asks. The updater's own adapter factory,
/// called from composition.
///
/// The choice is made here, once, while the app is being assembled, rather than
/// inside the spawned check: it is a decision about which outside thing the
/// host talks to, which is what composition is for.
///
/// The simulated source is compiled into a debug build only, and is gone rather
/// than disabled in a release one: a shipped app that can be told an update
/// exists is a shipped app an attacker can tell that to. The same shape as
/// `tray.rs`'s setup item.
pub fn release_source(app: &AppHandle) -> Arc<dyn ReleaseSource> {
    #[cfg(debug_assertions)]
    match simulated(env::var(SIMULATED_UPDATE).ok().as_deref()) {
        Simulated::Announce(version) => {
            return Arc::new(SimulatedReleases {
                app: app.clone(),
                version,
            })
        }
        Simulated::Ignored(value) => eprintln!(
            "[nessa] ignoring {SIMULATED_UPDATE}={value}: not a version like 9.9.9. \
             Checking the real release endpoint instead."
        ),
        Simulated::Off => {}
    }

    Arc::new(PluginReleases(app.clone()))
}

/// Asks the release endpoint once, off the startup path.
///
/// Called from `main`'s `setup` after the tray exists, because the tray menu is
/// where the answer goes. It returns immediately: the request itself happens on
/// the async runtime, so a slow or hanging endpoint delays nothing on screen —
/// not the panel, and not first-run setup.
///
/// The source is handed in rather than chosen here; see [`release_source`].
pub fn check_in_background(app: &AppHandle, source: Arc<dyn ReleaseSource>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_check(&*source, &TrayOutcome(app)).await;
    });
}

/// One check, from the source's answer through to the tray.
///
/// Everything outside the process is on one of the two ports, so this is the
/// whole of what a check does and all of it is exercised in [`tests`].
async fn run_check(source: &dyn ReleaseSource, outcome: &impl CheckOutcome) {
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
    // A simulated offer has no release behind it, so there is nothing here to
    // download, install, or restart onto — and the click must say that rather
    // than return silently, which from the menu is indistinguishable from a
    // broken item. Nothing is faked: no progress, no restart.
    #[cfg(debug_assertions)]
    if let Some(simulated) = app.try_state::<SimulatedUpdate>() {
        eprintln!(
            "[nessa] the offer of {} came from {SIMULATED_UPDATE}: there is no release to \
             install. Nothing was downloaded and the app is not restarting. Use the local \
             endpoint recipe to exercise a real download.",
            simulated.0
        );
        return;
    }

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
        fn check(&self) -> Pin<Box<dyn Future<Output = Checked> + Send>> {
            let checked = self.0.clone();
            Box::pin(async move { checked })
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

    /// The rule for `NESSA_FAKE_UPDATE`, in the build that is allowed to have
    /// one. There is no release counterpart to these: in a release build
    /// [`simulated`] and `SimulatedReleases` are not compiled at all, so a test
    /// calling them there would not compile either — `cargo clippy -p nessa-app
    /// --all-targets --release -- -D warnings` is what proves the release arm
    /// builds clean without them, and a release binary has nothing to consult.
    #[cfg(debug_assertions)]
    mod simulation {
        use super::*;

        #[test]
        fn a_version_is_announced_without_asking_the_endpoint() {
            assert_eq!(
                simulated(Some("9.9.9")),
                Simulated::Announce("9.9.9".to_string())
            );
            // Shells and `.env` files hand over the surrounding spaces too.
            assert_eq!(
                simulated(Some("  0.2.0  ")),
                Simulated::Announce("0.2.0".to_string())
            );
        }

        #[test]
        fn an_unset_or_empty_variable_is_an_ordinary_run() {
            assert_eq!(simulated(None), Simulated::Off);
            assert_eq!(simulated(Some("")), Simulated::Off);
            assert_eq!(simulated(Some("   ")), Simulated::Off);
        }

        #[test]
        fn a_value_that_is_not_a_version_falls_through_to_the_real_source() {
            // Reported rather than obeyed: the value becomes the tray item's
            // own text, and an item reading "Update to banana" claims something
            // no real check could ever have found.
            for garbage in ["banana", "9.9", "9.9.9.9", "v9.9.9", "9.9.x", "9..9"] {
                assert_eq!(
                    simulated(Some(garbage)),
                    Simulated::Ignored(garbage.to_string()),
                    "{garbage} should not be announced as a version"
                );
            }
        }

        #[test]
        fn an_announced_version_drives_the_same_offer_the_real_source_would() {
            // The simulation's whole claim is that it reaches the tray by the
            // ordinary path, so the decision is checked with the announced
            // version rather than trusted.
            let Simulated::Announce(version) = simulated(Some("9.9.9")) else {
                panic!("9.9.9 is a version");
            };
            let outcome = check(Checked::Newer {
                version: version.clone(),
            });

            assert_eq!(outcome.offered(), vec![version]);
            assert!(outcome.failures().is_empty());
        }
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
