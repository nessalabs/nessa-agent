//! Whether a newer Nessa has been published, and installing it if one has.
//!
//! The check is deliberately quiet. It runs once, in the background, after the
//! app is up, and its only way of reaching anybody is a notice inside the
//! panel, above the composer. There is no dialog, no prompt, no toast: nothing
//! here takes the screen, so nothing here can land on top of first-run setup,
//! which does — setup is a different window, and the notice belongs to the
//! panel. An update found while the panel is closed simply waits in
//! [`Announced`] until the panel is next opened and asks.
//!
//! A check that does not complete is a normal outcome, not a fault. The machine
//! may be offline, the release endpoint may be down, a proxy may be in the way.
//! In every one of those cases the panel stays exactly as it was and the failure
//! goes to stderr with the app's other diagnostics — the person is running an
//! app that works, and an update they have not heard of is not news.
//!
//! An install that was actually asked for is the opposite: it is on screen
//! because somebody put it there, so its failure is on screen too. The download
//! reports its progress and its refusal to the panel, which turns the second
//! into a plain statement and a retry.
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
//!                            PanelOutcome   recorder
//!                                   │
//!                     Announced ──▶ panel ──▶ install_update ──▶ progress
//!                                                   │               │
//!                                             PendingUpdate    restart / failed
//! ```
//!
//! Both sides of a check are ports. The answer comes off the network and the
//! notice goes into a window that needs a window server, so neither could be
//! reached from a test; behind traits, every outcome of a check — found,
//! current, refused — drives the real flow in [`tests`]. The arrows point at
//! the two implementations of each: the real one, and the one the tests use.
//!
//! `Checked` is what one check found and `Offer` is what the panel is told
//! about it; [`offer`] is the whole of the rule connecting them, kept pure so
//! the "say nothing" cases are decided in one readable place. What the panel
//! then *does* — notice, tab, dismissal — is the panel's own decision and lives
//! in `src/panel/application/update-surface.ts`.
//!
//! `Simulated` is the third source and is compiled only into a debug build: it
//! answers "yes, `9.9.9`" from `NESSA_FAKE_UPDATE` without a network, so the
//! decision, the notice, and the install can be watched in `pnpm app` with
//! nothing published. It cannot install anything, and the install says so
//! through the same failure path a refused download uses. The *other* way to
//! watch this locally — the real plugin, real HTTP, real manifest parse,
//! against a server on this machine — needs no code here at all: it is a
//! `--config` endpoint merge, `scripts/desktop/updater-harness.mjs
//! --check-only`, and `docs/codebase-structure.md` has both recipes.

#[cfg(debug_assertions)]
use std::env;
use std::future::Future;
use std::pin::Pin;
use std::sync::{Arc, Mutex};

use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager};
use tauri_plugin_updater::{Update, UpdaterExt};

use crate::host;
use crate::panel;

/// A published release that is newer than the running build, as the panel
/// describes it to somebody.
///
/// Everything here is on screen: the two versions are the tab's first line, and
/// the notes are the rest of it. `notes` is `None` when the manifest published
/// none, which is the case that ships first — nothing fills the updater
/// manifest's `notes` field yet.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Release {
    /// The version this build is running, which the tab reads from.
    pub from: String,
    /// The published version, as the release manifest announced it.
    pub version: String,
    /// What the manifest published as notes, if it published any. Blank notes
    /// are `None`: a heading over nothing is worse than the empty state.
    pub notes: Option<String>,
}

/// What one check found.
///
/// The plugin's `Result<Option<Update>, Error>` is reduced to this at the
/// boundary so the decision below branches on named outcomes rather than on an
/// error's text. [`Checked::Failed`] carries the failure only to log it; no
/// decision reads that string.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Checked {
    /// A published release is newer than the running build.
    Newer(Release),
    /// The running build is the latest published one.
    Current,
    /// The check did not complete: offline, no endpoint, an unreadable
    /// manifest, a signature that did not verify.
    Failed(String),
}

/// Whether the panel had already been told about an update when a check
/// started.
///
/// Asked as a fact, not acted on by whoever answers: an update an earlier check
/// announced is still announced and still installable, so a later check finding
/// the same release has nothing to add. Whether the notice is still on screen
/// is the panel's business — somebody may have dismissed it — and re-announcing
/// a version they just dismissed is exactly the nagging the dismissal asked to
/// stop.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Offering {
    /// Nothing has been announced this launch.
    Nothing,
    /// An earlier check already announced an update.
    AnUpdate,
}

/// What the panel is told about a finished check.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Offer {
    /// Tell the panel this release is available.
    Announce(Release),
    /// Change nothing. Whoever is using the app sees no sign a check happened.
    Nothing,
}

/// The one rule: an update the person can actually install, and has not already
/// been told about, is the only thing worth saying. Being current says nothing,
/// and a check that failed says nothing either — there is no useful action
/// behind "we could not ask".
pub fn offer(checked: &Checked, offering: Offering) -> Offer {
    match (checked, offering) {
        (Checked::Newer(release), Offering::Nothing) => Offer::Announce(release.clone()),
        (Checked::Newer(_), Offering::AnUpdate) | (Checked::Current | Checked::Failed(_), _) => {
            Offer::Nothing
        }
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
/// ownership: the release an install downloads is the one the real source found
/// and kept, and no decision here ever reads it.
pub trait ReleaseSource: Send + Sync {
    /// Asks once. A source that finds an update also retains it, so that the
    /// release announced below and the one an install downloads are the same.
    ///
    /// Boxed rather than `impl Future` because composition holds the chosen
    /// source behind a pointer: which one a build asks is a composition
    /// decision, and a trait a bundle can carry has to be object safe. Every
    /// implementation clones what it needs before the future starts, so the
    /// future borrows nothing from the source.
    fn check(&self) -> Pin<Box<dyn Future<Output = Checked> + Send>>;
}

/// What a finished check is allowed to do: tell the panel, or write one
/// diagnostic line.
///
/// A port for the same reason as the source — the panel is a webview in a
/// window that needs a window server, and the diagnostics are the process's
/// stderr — and it reports the one fact the rule needs rather than deciding
/// anything with it.
trait CheckOutcome {
    /// Whether an update has been announced already.
    fn offering(&self) -> Offering;

    /// Tells the panel this release is available, and keeps it for a panel that
    /// is not listening yet.
    fn announce(&self, release: &Release);

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
                    let release = Release {
                        from: update.current_version.clone(),
                        version: update.version.clone(),
                        notes: published_notes(update.body.as_deref()),
                    };
                    retain(&app, update);
                    Checked::Newer(release)
                }
                Ok(None) => Checked::Current,
                Err(error) => Checked::Failed(error.to_string()),
            }
        })
    }
}

/// What the manifest actually published as notes.
///
/// A `notes` field that is absent, empty, or nothing but whitespace is the same
/// fact — this release published none — and the tab has an empty state for it.
/// Carrying a blank string instead would put a heading over nothing.
fn published_notes(body: Option<&str>) -> Option<String> {
    body.map(str::trim)
        .filter(|notes| !notes.is_empty())
        .map(str::to_string)
}

/// The variable a debug build reads to pretend a release was published.
///
/// Set it to the version to announce — `NESSA_FAKE_UPDATE=9.9.9 pnpm app` — and
/// the check answers from it instead of asking the endpoint. It is the inner
/// loop for the decision, the notice, and the install: no server, no artifact,
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
/// straight into the notice's text, and "Update available / banana" is not
/// honest about what a real check would ever produce.
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
/// nothing because no install ever follows it. This one exists to drive a
/// running app, so it does what the real source does — answer, and record what
/// a later install will find — and what it records is that there is nothing to
/// install. Widening the test double's `cfg` would put a test fixture in the
/// dev binary and still leave that second job undone.
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
            // The real source retains the update an install downloads; this
            // retains the fact that no such update exists, which is what makes
            // the install able to say so instead of doing nothing.
            app.manage(SimulatedUpdate(version.clone()));
            eprintln!(
                "[nessa] {SIMULATED_UPDATE}={version}: offering a simulated update. \
                 The release endpoint was not asked and nothing was downloaded."
            );
            Checked::Newer(Release {
                from: app.package_info().version.to_string(),
                version,
                // The same empty state a real release currently produces:
                // nothing fills the manifest's `notes` field yet.
                notes: None,
            })
        })
    }
}

/// Managed when a simulated update is offered, so the install can be honest.
#[cfg(debug_assertions)]
struct SimulatedUpdate(String);

/// The real outcome: the panel's window, and the app's stderr diagnostics.
struct PanelOutcome(AppHandle);

impl CheckOutcome for PanelOutcome {
    fn offering(&self) -> Offering {
        // [`Announced`] is filled the moment a release is announced and is
        // never emptied, so this is exactly "has anything been announced this
        // launch". The notice's own comings and goings belong to the panel.
        match self.0.try_state::<Announced>() {
            Some(_) => Offering::AnUpdate,
            None => Offering::Nothing,
        }
    }

    fn announce(&self, release: &Release) {
        // Kept before it is sent, and on purpose: a check can finish before the
        // panel's page has loaded its listeners, and an update found while the
        // panel is closed has to wait somewhere until it is next opened. The
        // page asks for this on mount through [`available_update`], so the
        // event is the live path and the slot is the durable one — both read
        // the same single value.
        self.0.manage(Announced(release.clone()));
        // Only the panel. Setup owns the whole screen when it is up and has no
        // notice to show; sending this to every window would put an update
        // event into a first-run surface that has nowhere to put it.
        if let Err(error) = self
            .0
            .emit_to(panel::MAIN_WINDOW, host::UPDATE_AVAILABLE, release)
        {
            // Survivable: the panel asks for the same value when it next
            // mounts, so a lost event costs the notice appearing now, not at
            // all.
            eprintln!(
                "[nessa] could not tell the panel about {}: {error}",
                release.version
            );
        }
    }

    fn report_failure(&self, reason: &str) {
        // Survivable, and on purpose: this is the offline case as much as it is
        // the broken-endpoint case, and neither is the person's problem. A
        // *check* nobody asked for stays off the screen; an install somebody
        // did ask for does not — see [`install_update`].
        eprintln!("[nessa] could not check for an update: {reason}");
    }
}

/// The release the panel was told about, kept for a panel that was not
/// listening when it was told.
///
/// Managed once, by the announcement, and never replaced: [`Offering`] stops a
/// second announcement before one could be. It is a live slot rather than a
/// dependency — nothing reads it from outside the process — so it stays managed
/// state, like the pending update below.
struct Announced(Release);

/// The update the panel is offering, held until somebody asks to install it.
///
/// Managed only once an update has actually been found, so the announcement and
/// the update it installs arrive together. The slot is emptied by the request
/// that starts the install, which is also what stops a second request from
/// starting a second download of the same release; a failed install puts it
/// back, because the panel offers a retry.
struct PendingUpdate(Mutex<Option<Update>>);

/// Puts a found update where a later install will find it.
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
/// Called from `main`'s `setup`. It returns immediately: the request itself
/// happens on the async runtime, so a slow or hanging endpoint delays nothing
/// on screen — not the panel, and not first-run setup.
///
/// The source is handed in rather than chosen here; see [`release_source`].
pub fn check_in_background(app: &AppHandle, source: Arc<dyn ReleaseSource>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        run_check(&*source, &PanelOutcome(app)).await;
    });
}

/// One check, from the source's answer through to the panel.
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

    if let Offer::Announce(release) = offer(&checked, offering) {
        outcome.announce(&release);
    }
}

/// The release the panel should be showing, for a page that has just mounted.
///
/// The check can finish before the panel's page exists — and on a launch where
/// the panel is never opened, long before it does — so the event alone would
/// lose the announcement. The page asks this once on mount and gets the same
/// value the event carried.
#[tauri::command]
pub fn available_update(app: AppHandle) -> Option<Release> {
    app.try_state::<Announced>().map(|state| state.0.clone())
}

/// How far a download has got.
///
/// `total` is what the server declared, which it may not have: a response with
/// no length is a real case, and the panel draws an unmeasured bar for it
/// rather than inventing a percentage.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Downloaded {
    /// Bytes received so far.
    pub downloaded: u64,
    /// Bytes the server said to expect, if it said.
    pub total: Option<u64>,
}

/// What the last progress event said, so the next chunk can tell whether it has
/// anything to add.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
struct Reported {
    /// The whole percent that event drew, when the total was known.
    percent: Option<u64>,
    /// The byte count it carried.
    bytes: u64,
}

/// How much of an unmeasured download has to arrive before it is worth saying
/// so again. There is no bar to move, only a byte count, and a line of text
/// re-rendered thousands of times a second is worse than one re-rendered four
/// times a megabyte.
const UNMEASURED_STEP: u64 = 256 * 1024;

/// Whether this chunk moved anything a person could see.
///
/// The plugin calls back once per chunk, which for a real artifact is thousands
/// of calls for a bar with a hundred positions; every one of them would be an
/// IPC message and a React render. A measured download reports each whole
/// percent, an unmeasured one every [`UNMEASURED_STEP`], and both always report
/// the first chunk so the bar appears as soon as bytes do.
///
/// `None` means "nothing new to draw". `Some` is what the next call compares
/// against.
fn worth_reporting(received: u64, total: Option<u64>, reported: Reported) -> Option<Reported> {
    let percent = total
        .filter(|total| *total > 0)
        .map(|total| (received.min(total).saturating_mul(100)) / total);

    let first = reported.bytes == 0;
    let moved = match percent {
        Some(percent) => reported.percent != Some(percent),
        None => received.saturating_sub(reported.bytes) >= UNMEASURED_STEP,
    };

    match first || moved {
        true => Some(Reported {
            percent,
            bytes: received,
        }),
        false => None,
    }
}

/// Downloads and installs the announced update, then comes back up on it.
///
/// Called by the panel's update tab, which is on screen because somebody put it
/// there. That is what makes this the one path here that reports its failure to
/// the screen rather than to stderr alone: the tab turns it into a plain
/// statement and offers the retry this leaves possible, because the update goes
/// back in its slot.
#[tauri::command]
pub fn install_update(app: AppHandle) {
    // A simulated offer has no release behind it, so there is nothing here to
    // download, install, or restart onto — and the tab must say that rather
    // than sit at nought per cent for ever. It travels the same way a refused
    // download does. Nothing is faked: no progress, no restart.
    #[cfg(debug_assertions)]
    if let Some(simulated) = app.try_state::<SimulatedUpdate>() {
        refuse(
            &app,
            &format!(
                "the offer of {} came from {SIMULATED_UPDATE}: there is no release to install. \
                 Nothing was downloaded and the app is not restarting. Use the local endpoint \
                 recipe to exercise a real download.",
                simulated.0
            ),
        );
        return;
    }

    let Some(pending) = app.try_state::<PendingUpdate>() else {
        // Nothing was ever found, so nothing can be installed. The tab is only
        // reachable from an announcement, so this is a page asking for
        // something this launch never offered.
        refuse(&app, "there is no update to install");
        return;
    };
    let Some(update) = pending.0.lock().ok().and_then(|mut slot| slot.take()) else {
        // Already installing. The download is not started twice, and the tab is
        // already showing the first one's progress.
        return;
    };

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        let reporter = app.clone();
        let mut received: u64 = 0;
        let mut reported = Reported::default();
        let on_chunk = move |chunk: usize, total: Option<u64>| {
            received = received.saturating_add(chunk as u64);
            let Some(next) = worth_reporting(received, total, reported) else {
                return;
            };
            reported = next;
            // Survivable: a lost progress event costs one position of a bar.
            // The install itself is unaffected, and the next chunk redraws it.
            let _ = reporter.emit_to(
                panel::MAIN_WINDOW,
                host::UPDATE_PROGRESS,
                Downloaded {
                    downloaded: received,
                    total,
                },
            );
        };

        match update.download_and_install(on_chunk, || {}).await {
            // macOS and Linux install in place and leave the old binary
            // running; Windows hands over to an installer that exits this
            // process itself, so the restart below is never reached there.
            Ok(()) => app.restart(),
            Err(error) => {
                if let Some(pending) = app.try_state::<PendingUpdate>() {
                    if let Ok(mut slot) = pending.0.lock() {
                        *slot = Some(update);
                    }
                }
                refuse(&app, &error.to_string());
            }
        }
    });
}

/// Tells the panel an install did not happen, and why.
///
/// Both halves matter. The panel turns this into the statement and the retry a
/// person can act on; stderr keeps the technical reason next to the app's other
/// diagnostics, where a bug report can find it.
fn refuse(app: &AppHandle, reason: &str) {
    eprintln!("[nessa] could not install the update: {reason}");
    let _ = app.emit_to(panel::MAIN_WINDOW, host::UPDATE_FAILED, reason);
}

#[cfg(test)]
mod tests {
    use super::*;

    fn release(version: &str) -> Release {
        Release {
            from: "0.1.0".to_string(),
            version: version.to_string(),
            notes: None,
        }
    }

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

    /// A panel that writes down what it was told instead of showing it.
    ///
    /// It also answers [`CheckOutcome::offering`] from what it has already been
    /// told, exactly as the real one answers from the announcement it kept — so
    /// a second check meets the state the first one left.
    #[derive(Default)]
    struct RecordedOutcome {
        announced: Mutex<Vec<Release>>,
        failures: Mutex<Vec<String>>,
    }

    impl RecordedOutcome {
        fn announced(&self) -> Vec<Release> {
            self.announced.lock().expect("announcements").clone()
        }

        fn versions(&self) -> Vec<String> {
            self.announced()
                .into_iter()
                .map(|release| release.version)
                .collect()
        }

        fn failures(&self) -> Vec<String> {
            self.failures.lock().expect("failures").clone()
        }
    }

    impl CheckOutcome for RecordedOutcome {
        fn offering(&self) -> Offering {
            match self.announced.lock().expect("announcements").is_empty() {
                true => Offering::Nothing,
                false => Offering::AnUpdate,
            }
        }

        fn announce(&self, release: &Release) {
            self.announced
                .lock()
                .expect("announcements")
                .push(release.clone());
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
    fn a_newer_release_is_announced_whole() {
        // The panel draws `from → version` and the notes, so all three have to
        // survive the decision rather than only the version the tray used to
        // name.
        let found = Release {
            from: "0.1.0".to_string(),
            version: "0.2.0".to_string(),
            notes: Some("Setup stops running every launch".to_string()),
        };

        assert_eq!(
            offer(&Checked::Newer(found.clone()), Offering::Nothing),
            Offer::Announce(found)
        );
    }

    #[test]
    fn the_latest_build_says_nothing() {
        assert_eq!(offer(&Checked::Current, Offering::Nothing), Offer::Nothing);
    }

    #[test]
    fn a_check_that_did_not_complete_says_nothing() {
        // The offline case. A failed check is not an error to report on
        // screen — the panel must look identical to a check that found nothing.
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
    fn a_found_release_reaches_the_panel_once() {
        let outcome = check(Checked::Newer(release("0.3.1")));

        assert_eq!(outcome.versions(), vec!["0.3.1".to_string()]);
        assert!(outcome.failures().is_empty());
    }

    #[test]
    fn a_current_build_leaves_the_panel_alone() {
        let outcome = check(Checked::Current);

        assert!(outcome.announced().is_empty());
        assert!(outcome.failures().is_empty());
    }

    #[test]
    fn a_refused_check_announces_nothing_and_is_still_reported() {
        // Silent on screen, not silent in the diagnostics: an endpoint that is
        // never reachable would otherwise look exactly like being up to date.
        let outcome = check(Checked::Failed("connection refused".to_string()));

        assert!(outcome.announced().is_empty());
        assert_eq!(outcome.failures(), vec!["connection refused".to_string()]);
    }

    #[test]
    fn a_manifest_with_no_usable_notes_announces_none() {
        // The state that ships first: nothing fills the manifest's `notes`
        // field yet, and a field present but blank is the same fact.
        assert_eq!(published_notes(None), None);
        assert_eq!(published_notes(Some("")), None);
        assert_eq!(published_notes(Some("  \n\t ")), None);
    }

    #[test]
    fn published_notes_are_carried_without_their_surrounding_blank_lines() {
        assert_eq!(
            published_notes(Some("\nSetup stops running every launch\n\n")),
            Some("Setup stops running every launch".to_string())
        );
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
            // Reported rather than obeyed: the value becomes the notice's own
            // text, and a notice reading "Update available / banana" claims
            // something no real check could ever have found.
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
            // The simulation's whole claim is that it reaches the panel by the
            // ordinary path, so the decision is checked with the announced
            // version rather than trusted.
            let Simulated::Announce(version) = simulated(Some("9.9.9")) else {
                panic!("9.9.9 is a version");
            };
            let outcome = check(Checked::Newer(release(&version)));

            assert_eq!(outcome.versions(), vec![version]);
            assert!(outcome.failures().is_empty());
        }
    }

    #[test]
    fn a_second_check_does_not_announce_the_same_release_again() {
        let outcome = RecordedOutcome::default();
        let found = FakeReleases(Checked::Newer(release("0.3.1")));

        tauri::async_runtime::block_on(async {
            run_check(&found, &outcome).await;
            run_check(&found, &outcome).await;
        });

        assert_eq!(outcome.versions(), vec!["0.3.1".to_string()]);
    }

    #[test]
    fn the_first_chunk_is_always_worth_drawing() {
        // Bytes have arrived and the bar is still empty; waiting a whole
        // percent to say so is the one case where the throttle would be felt.
        assert_eq!(
            worth_reporting(1, Some(10_000_000), Reported::default()),
            Some(Reported {
                percent: Some(0),
                bytes: 1
            })
        );
        assert_eq!(
            worth_reporting(1, None, Reported::default()),
            Some(Reported {
                percent: None,
                bytes: 1
            })
        );
    }

    #[test]
    fn a_measured_download_reports_each_whole_percent_and_no_more() {
        let at_one_percent = Reported {
            percent: Some(1),
            bytes: 100_000,
        };

        // Still the same percent: thousands of chunks land inside one position
        // of the bar, and none of them has anything new to draw.
        assert_eq!(
            worth_reporting(100_001, Some(10_000_000), at_one_percent),
            None
        );
        assert_eq!(
            worth_reporting(199_999, Some(10_000_000), at_one_percent),
            None
        );
        assert_eq!(
            worth_reporting(200_000, Some(10_000_000), at_one_percent),
            Some(Reported {
                percent: Some(2),
                bytes: 200_000
            })
        );
    }

    #[test]
    fn a_download_that_overruns_its_declared_length_stays_at_a_hundred() {
        // A server that under-declares its length must not produce 103%.
        assert_eq!(
            worth_reporting(
                1_100,
                Some(1_000),
                Reported {
                    percent: Some(99),
                    bytes: 990
                }
            ),
            Some(Reported {
                percent: Some(100),
                bytes: 1_100
            })
        );
    }

    #[test]
    fn an_unmeasured_download_reports_by_the_byte_step() {
        // No declared length, so there is no bar to move — only the byte count,
        // which is throttled by size instead of by percent.
        let started = Reported {
            percent: None,
            bytes: UNMEASURED_STEP,
        };

        assert_eq!(worth_reporting(UNMEASURED_STEP + 1, None, started), None);
        assert_eq!(
            worth_reporting(UNMEASURED_STEP * 2, None, started),
            Some(Reported {
                percent: None,
                bytes: UNMEASURED_STEP * 2
            })
        );
    }

    #[test]
    fn a_declared_length_of_nothing_is_treated_as_no_length_at_all() {
        // `Content-Length: 0` alongside a body that is arriving would divide by
        // zero; it reads as unmeasured instead.
        assert_eq!(
            worth_reporting(
                UNMEASURED_STEP + 1,
                Some(0),
                Reported {
                    percent: None,
                    bytes: 1
                }
            ),
            Some(Reported {
                percent: None,
                bytes: UNMEASURED_STEP + 1
            })
        );
    }
}
