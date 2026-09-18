//! The floating panel: its size, its place on the screen, and showing it.
//!
//! The tray and the shortcut *request* a show or a hide. They do not know how
//! the frame is fitted. The frame is reapplied on every show, so moving
//! between displays re-places the panel rather than stranding it.

use std::io;

use serde::Serialize;
use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, State, WebviewUrl,
    WebviewWindow, WebviewWindowBuilder,
};

use crate::composition::HostDependencies;
use crate::host;
use crate::platform;
use crate::settings::{Onboarding, Panel, Settings, SettingsStore};

/// The panel itself. Named here because the window policies that single it out
/// — close dismisses it rather than quitting — live in the host's event handler.
pub const MAIN_WINDOW: &str = "main";
/// The floor the resize edge may not drag the panel below, whatever the
/// configured minimum width is: a panel shorter than this has no transcript.
pub const MIN_PANEL_HEIGHT: f64 = 320.0;
/// How far the panel sits from the edges of the work area.
pub const EDGE_PADDING: f64 = 20.0;

/// Reads the settings the app was launched with, falling back to the defaults
/// if they were never managed (which only happens if setup failed).
fn settings(app: &AppHandle) -> Settings {
    app.try_state::<Settings>()
        .map(|state| state.inner().clone())
        .unwrap_or_default()
}

/// Shows the panel if it is hidden, hides it if it is not. Returns whether it
/// is showing afterwards, which is what a surface teaching the shortcut needs
/// to know. Returns `None` when there is no panel to toggle at all — or when
/// the show failed and there is still nothing on screen — distinct from a hide,
/// so a caller reporting this onward cannot be mistaken for a press that
/// actually put the panel away, nor for one that brought it up.
pub fn toggle(app: &AppHandle) -> Option<bool> {
    let window = app.get_webview_window(MAIN_WINDOW)?;

    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        // Hiding the panel takes the key window away with it. While setup is on
        // screen that leaves focus nowhere, and setup's way out — Escape — is a
        // key handler in its page, which never sees a key it is not focused for.
        // The lesson that teaches this shortcut presses it twice.
        if let Some(setup) = app.get_webview_window(SETUP_WINDOW) {
            if setup.is_visible().unwrap_or(false) {
                platform::current().reveal_overlay(&setup);
            }
        }
        return Some(false);
    }

    if let Err(error) = show(&window, &settings(app)) {
        eprintln!("[nessa] could not summon the panel: {error}");
        return None;
    }
    Some(true)
}

/// The first-run setup window, which is a screen-covering takeover rather than
/// a panel.
pub const SETUP_WINDOW: &str = "setup";

/// The size setup opens at on hosts that do not place it themselves.
///
/// `place_overlay` replaces this frame with the whole screen where the host can
/// do that. Where it is an explicit no-op, this *is* the window somebody gets,
/// so it has to be a window rather than whatever default the window system
/// hands out for a size nobody asked for.
const SETUP_WIDTH: f64 = 960.0;
const SETUP_HEIGHT: f64 = 640.0;

/// The one place the setup window's shape is written down.
///
/// It is built here rather than declared in `tauri.conf.json` because setup is
/// also reopened during a session (`restart_onboarding`), and two declarations
/// of one window are two things to keep in step with nothing comparing them.
///
/// Built hidden: a window is on screen the moment it exists, and its page
/// reveals it once it has a frame to show (`reveal_setup_window`).
fn build_setup_window(app: &AppHandle) -> tauri::Result<WebviewWindow> {
    WebviewWindowBuilder::new(
        app,
        SETUP_WINDOW,
        WebviewUrl::App("index.html?surface=setup".into()),
    )
    .title("Welcome to Nessa")
    .inner_size(SETUP_WIDTH, SETUP_HEIGHT)
    .center()
    .resizable(false)
    .transparent(true)
    .decorations(false)
    .shadow(false)
    .always_on_top(true)
    // Deliberately not maximized: `place_overlay` gives it the whole screen,
    // menu bar included, and maximizing fits a window to the *visible* frame —
    // which is the screen minus exactly the parts this needs to cover.
    .skip_taskbar(true)
    .visible(false)
    .build()
}

/// Opens first-run setup at startup: built hidden, shaped into an overlay, and
/// left for its own page to reveal. A setup window that cannot be built is
/// survivable — the panel is still reachable from the tray.
pub fn open_setup_window(app: &AppHandle) {
    match build_setup_window(app) {
        Ok(window) => platform::current().place_overlay(&window),
        Err(error) => eprintln!("[nessa] could not open setup: {error}"),
    }
}

/// What handing setup over to the panel did, step by step.
///
/// Only the steps that can fail without the handoff failing. Showing the panel
/// is not among them: a handoff whose panel did not come up is an `Err` and
/// changes nothing else, so there is no shape of this struct in which the panel
/// is not already on screen.
#[derive(Clone, Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SetupHandoff {
    /// Whether the setup window is gone. False leaves a window on screen that
    /// its own page has to offer a way out of.
    pub setup_closed: bool,
    /// Why the setup window is still on screen, when it is.
    pub close_error: Option<String>,
    /// Why completion was not written down, when it was asked for and refused.
    /// The panel is up either way; the cost is a second run of setup.
    pub record_error: Option<String>,
}

/// Hand setup over to the panel: show the panel, write setup off, close setup.
///
/// One command rather than three, because the order between them is the whole
/// point and no webview can hold it. Setup's window used to run this sequence
/// itself and issued its last call — the completion write — *after* awaiting its
/// own close, so a teardown that won the race left `onboarding.completed` unset
/// on a machine somebody had just finished setting up.
///
/// The order here, in Rust, from the process that outlives the window:
///
/// ```text
///   show panel ──► record completion ──► close setup
///        │               │                    │
///     Err: stop      logged, kept going   reported back
/// ```
///
/// Showing the panel is the only step that can abandon the handoff: nothing is
/// written and setup stays on screen, because a completion recorded over a
/// panel nobody has seen work buries first run behind a failure the next launch
/// cannot see. A refused write does not stop the close — that would trade the
/// panel for a settings file — but it now happens while the window is still
/// alive, which is the point.
///
/// `completed` is the surface's own account of how setup ended: finished, or
/// left. Leaving stays free to change its mind, so only a finish is recorded.
#[tauri::command]
pub fn finish_setup(
    app: AppHandle,
    deps: State<'_, HostDependencies>,
    completed: bool,
) -> Result<SetupHandoff, String> {
    let settings_store = deps.settings.clone();
    hand_over(
        completed,
        || {
            let window = app
                .get_webview_window(MAIN_WINDOW)
                .ok_or_else(|| "there is no panel to summon".to_string())?;
            // Through `show` rather than `WebviewWindow::show`: the panel is
            // anchored to an edge of the work area and its webview fitted to the
            // window, and a plain show does neither. The first thing a person
            // saw after setup was otherwise a panel wherever the window system
            // happened to leave it.
            show(&window, &settings(&app))
                .map_err(|error| format!("could not show the panel: {error}"))
        },
        // Written through the settings store, which is the one durable store
        // the app has — the same load, change, save the tray's own toggle does.
        // The managed `Settings` snapshot is deliberately not updated in place:
        // nothing after startup reads this flag, and the launch that does reads
        // it off disk before anything is managed at all.
        || record_completion(&*settings_store),
        || match app.get_webview_window(SETUP_WINDOW) {
            // Closing it destroys it, which is what lets the panel back down to
            // its ordinary level (see the `Destroyed` handler in `main.rs`).
            Some(setup) => setup
                .close()
                .map_err(|error| format!("could not close setup: {error}")),
            // Already gone. The end state this asks for is the one that holds.
            None => Ok(()),
        },
    )
}

/// The order of the handoff, with its three effects supplied.
///
/// Split from [`finish_setup`] so the ordering guarantee — and what each
/// failure does to the steps after it — is a thing a test can hold, rather than
/// something only a running window system could demonstrate.
fn hand_over(
    completed: bool,
    show_panel: impl FnOnce() -> Result<(), String>,
    record: impl FnOnce() -> Result<(), String>,
    close_setup: impl FnOnce() -> Result<(), String>,
) -> Result<SetupHandoff, String> {
    show_panel()?;

    let record_error = if completed { record().err() } else { None };
    if let Some(error) = &record_error {
        // Survivable: the panel is up, and the cost is one more run of setup.
        eprintln!("[nessa] {error}");
    }

    let close_error = close_setup().err();
    if let Some(error) = &close_error {
        eprintln!("[nessa] {error}");
    }

    Ok(SetupHandoff {
        setup_closed: close_error.is_none(),
        close_error,
        record_error,
    })
}

/// Put the setup window on screen, now that its page has something to show.
///
/// It is created hidden. A window is on screen the moment it exists, and a
/// webview has not painted anything the moment it is created — so a window
/// visible from the start shows whatever the window server has for it until
/// the first frame arrives, which is a flash of nothing at the very point the
/// opening is trying to begin from darkness.
#[tauri::command]
pub fn reveal_setup_window(window: WebviewWindow) {
    let _ = window.show();
    platform::current().reveal_overlay(&window);
}

/// The handoff's record step, as the caller reports it.
fn record_completion(settings: &dyn SettingsStore) -> Result<(), String> {
    set_onboarding(settings, Onboarding { completed: true })
        .map_err(|error| format!("could not record that setup finished: {error}"))
}

/// Writes the first-run flag to the settings file, leaving every other key as
/// the file has it.
///
/// Through [`SettingsStore::update`] rather than a load-change-save of its own:
/// the startup load answers an unreadable or malformed file with the defaults so
/// a launch can carry on, and saving *that* back would replace the person's real
/// panel geometry and quit policy with defaults — a silent loss, since the write
/// succeeds. `update` refuses instead, and the refusal is reported.
///
/// The store is a parameter rather than something looked up from the app: this
/// is the decision, and the decision is what a test has to be able to hold.
fn set_onboarding(settings: &dyn SettingsStore, onboarding: Onboarding) -> io::Result<()> {
    // The written value is the caller's to ignore: nothing here shows it back.
    settings
        .update(&mut |chosen| chosen.onboarding = onboarding.clone())
        .map(|_| ())
}

/// Opens first-run setup again, from the beginning.
///
/// Setup finishes by closing its own window, so there is usually nothing left
/// to show and a fresh one is built — by the same builder that opened the first
/// one. A window that is still open is reloaded rather than reused, because
/// setup holds its progress in memory and showing it again mid-flow would
/// resume it rather than restart it.
///
/// The persisted completion is cleared with it, so the tray item means what it
/// says: setup is genuinely un-finished again, and the next launch opens it —
/// rather than one window now over a file that still claims setup is done.
///
/// Debug builds only, with its one caller — the tray item that asks for it.
/// First-run setup only happens once now that it is persisted, so this is the
/// only way to see it a second time while working on it; it is not a feature
/// anybody asked for, so it does not ship until it is one.
#[cfg(debug_assertions)]
pub fn restart_onboarding(app: &AppHandle, settings: &dyn SettingsStore) {
    // Clearing it is what makes this a restart rather than a preview. A file
    // that will not take the change costs the next launch's setup, not this
    // window, so it is reported and the window opens anyway.
    if let Err(error) = set_onboarding(settings, Onboarding::default()) {
        eprintln!("[nessa] could not reopen setup for the next launch: {error}");
    }

    let Some(window) = app.get_webview_window(SETUP_WINDOW) else {
        // A fresh window is revealed by its page, the same as at startup.
        open_setup_window(app);
        return;
    };

    let _ = window.eval("window.location.reload()");
    let _ = window.show();
    let host = platform::current();
    host.place_overlay(&window);
    // Already on screen from an earlier run, so there is no first frame to wait
    // for: this one is revealed here rather than by the page.
    host.reveal_overlay(&window);
}

/// Places, fits, and focuses the panel, then hands the caret to the composer.
///
/// Fails only for the steps that decide whether the panel is on screen at all.
/// Placing and fitting are corrections to a panel that still appears, and the
/// composer's caret is a courtesy to a panel already showing; those are logged
/// or ignored rather than reported as a summon that did not happen.
pub fn show(window: &WebviewWindow, settings: &Settings) -> tauri::Result<()> {
    let _ = anchor_to_edge(window, settings);
    // The panel may have been summoned onto a display with more room than the
    // one it was last fitted for, and the viewport is sized for the work area
    // it is standing in. A failure costs the resize fix, not the show.
    if let Err(error) = platform::current().fit_viewport(window) {
        eprintln!("[nessa] could not fit the panel's viewport: {error}");
    }
    // Setup covers the menu bar, so a panel at its ordinary level would be
    // summoned behind the window teaching the shortcut that summoned it. The
    // window exists from startup and outlives being dismissed, so it is being on
    // screen that decides this — lifting the panel over an overlay that is not
    // there leaves it floating above the menu bar.
    let over_setup = window
        .app_handle()
        .get_webview_window(SETUP_WINDOW)
        .is_some_and(|setup| setup.is_visible().unwrap_or(false));
    platform::current().set_above_overlay(window, over_setup);
    window.show()?;
    window.set_focus()?;
    let _ = window.emit(host::FOCUS_COMPOSER, ());
    Ok(())
}

/// Applies the configured geometry once, at startup: the opening size, and the
/// floor the resize edge may not drag below. Everything after this is the
/// reader's own, so `anchor_to_edge` only re-fits and repositions.
pub fn apply_configured_size(window: &WebviewWindow, settings: &Settings) -> tauri::Result<()> {
    let opening = opening_size(&settings.panel);

    window.set_min_size(Some(LogicalSize::new(opening.min_width, MIN_PANEL_HEIGHT)))?;

    // Without a configured height the panel fills the work area, which
    // `anchor_to_edge` does on every show — so only the width is set here.
    if let Some(height) = opening.height {
        window.set_size(LogicalSize::new(opening.width, height))?;
    } else if let Some(size) =
        width_only_physical(opening.width, window.outer_size()?, window.scale_factor()?)
    {
        window.set_size(size)?;
    }

    Ok(())
}

/// The size the panel opens at, after resolving contradictions in the file
/// (a width below the minimum, a height below the transcript floor).
pub struct OpeningSize {
    pub width: f64,
    pub min_width: f64,
    pub height: Option<f64>,
}

/// A width-only fit that keeps the window's current height.
///
/// GTK asserts `height > 0` on `gtk_window_resize`. Before the window is
/// realized, `outer_size` reports height 0, so there is nothing safe to set
/// yet — `anchor_to_edge` on the first show fills the work area instead.
pub fn width_only_physical(
    opening_width: f64,
    current: PhysicalSize<u32>,
    scale: f64,
) -> Option<PhysicalSize<u32>> {
    (current.height > 0)
        .then(|| PhysicalSize::new((opening_width * scale).round() as u32, current.height))
}

/// GTK reports `outer_size` as 0×0 before the window is realized. Feeding
/// that to `frame_on` would open a 1px-wide strip, because the helper only
/// clamps to 1. Substitute the configured opening width instead; height 0 is
/// fine when the frame fills the work area.
pub fn realized_outer(
    current: PhysicalSize<u32>,
    opening_width: f64,
    scale: f64,
) -> PhysicalSize<u32> {
    let width = if current.width == 0 {
        (opening_width * scale).round() as u32
    } else {
        current.width
    };
    PhysicalSize::new(width.max(1), current.height)
}

pub fn opening_size(panel: &Panel) -> OpeningSize {
    let min_width = panel.min_width.max(1.0);
    // A configured width below the configured minimum is a contradiction; the
    // minimum wins, since it is the one the resize edge will enforce anyway.
    OpeningSize {
        width: panel.width.max(min_width),
        min_width,
        height: panel.height.map(|height| height.max(MIN_PANEL_HEIGHT)),
    }
}

/// The work area of a display, in physical pixels.
#[derive(Clone, Copy, Debug)]
pub struct WorkArea {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Frame {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Stands the panel in the lower right of a work area: it keeps whatever size
/// it currently has — the configured size on the first show, the reader's own
/// after a resize — clamped to what the work area can hold. A panel with no
/// configured height fills that area instead.
pub fn frame_on(
    area: WorkArea,
    current: PhysicalSize<u32>,
    fill_height: bool,
    scale: f64,
) -> Frame {
    let padding = (EDGE_PADDING * scale).round() as i32;

    // A display smaller than the padding would underflow; fitting on screen
    // matters more than the margin, so the work area wins.
    let available_height = area
        .height
        .saturating_sub(padding.unsigned_abs() * 2)
        .max(1);
    let available_width = area.width.saturating_sub(padding.unsigned_abs() * 2).max(1);

    let height = if fill_height {
        available_height
    } else {
        current.height.min(available_height).max(1)
    };
    let width = current.width.min(available_width).max(1);

    Frame {
        width,
        height,
        x: area.x + area.width as i32 - width as i32 - padding,
        // Anchored to the bottom, so a panel shorter than the screen opens in
        // the lower right rather than hanging from the top.
        y: area.y + area.height as i32 - height as i32 - padding,
    }
}

fn anchor_to_edge(window: &WebviewWindow, settings: &Settings) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };

    let area = monitor.work_area();
    let scale = monitor.scale_factor();
    let opening = opening_size(&settings.panel);
    let frame = frame_on(
        WorkArea {
            x: area.position.x,
            y: area.position.y,
            width: area.size.width,
            height: area.size.height,
        },
        realized_outer(window.outer_size()?, opening.width, scale),
        settings.panel.height.is_none(),
        scale,
    );

    window.set_size(PhysicalSize::new(frame.width, frame.height))?;
    window.set_position(PhysicalPosition::new(frame.x, frame.y))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;

    use super::*;

    /// The steps the handoff took, in the order it took them.
    #[derive(Default)]
    struct Steps(RefCell<Vec<&'static str>>);

    impl Steps {
        fn took(&self, step: &'static str) {
            self.0.borrow_mut().push(step);
        }

        fn order(&self) -> Vec<&'static str> {
            self.0.borrow().clone()
        }
    }

    #[test]
    fn shows_the_panel_then_records_then_closes_setup() {
        let steps = Steps::default();
        let handoff = hand_over(
            true,
            || {
                steps.took("show");
                Ok(())
            },
            || {
                steps.took("record");
                Ok(())
            },
            || {
                steps.took("close");
                Ok(())
            },
        )
        .expect("the panel came up");

        // The completion write happens before the window that asked for it is
        // destroyed. That ordering is the defect this command exists to fix.
        assert_eq!(steps.order(), ["show", "record", "close"]);
        assert!(handoff.setup_closed);
        assert!(handoff.close_error.is_none());
        assert!(handoff.record_error.is_none());
    }

    #[test]
    fn a_panel_that_will_not_show_records_nothing_and_leaves_setup_up() {
        let steps = Steps::default();
        let refused = hand_over(
            true,
            || Err("there is no panel to summon".to_string()),
            || {
                steps.took("record");
                Ok(())
            },
            || {
                steps.took("close");
                Ok(())
            },
        );

        // A completion written over a panel nobody has seen work buries first
        // run behind a failure the next launch cannot see; and the window has to
        // stay to say so.
        assert_eq!(
            refused.err().as_deref(),
            Some("there is no panel to summon")
        );
        assert_eq!(steps.order(), Vec::<&str>::new());
    }

    #[test]
    fn a_refused_write_still_closes_the_setup_window() {
        let steps = Steps::default();
        let handoff = hand_over(
            true,
            || {
                steps.took("show");
                Ok(())
            },
            || Err("could not record that setup finished: disk full".to_string()),
            || {
                steps.took("close");
                Ok(())
            },
        )
        .expect("the panel came up");

        // Survivable: the cost is one more run of setup, not the panel.
        assert_eq!(steps.order(), ["show", "close"]);
        assert!(handoff.setup_closed);
        assert_eq!(
            handoff.record_error.as_deref(),
            Some("could not record that setup finished: disk full")
        );
    }

    /// A settings file that cannot be read is now *refused* the flag rather
    /// than replaced with defaults carrying it. The refusal has to cost a
    /// second run of setup and nothing more: the panel is still shown and the
    /// setup window still closed, with the reason reported rather than
    /// swallowed.
    #[test]
    fn a_settings_file_that_will_not_take_the_flag_still_hands_the_panel_over() {
        let steps = Steps::default();
        let refusal = format!(
            "could not record that setup finished: {}",
            io::Error::new(
                io::ErrorKind::InvalidData,
                "settings.json is not valid settings: expected value at line 1 column 2",
            )
        );
        let handoff = hand_over(
            true,
            || {
                steps.took("show");
                Ok(())
            },
            || Err(refusal.clone()),
            || {
                steps.took("close");
                Ok(())
            },
        )
        .expect("a file this build cannot read is not a reason to withhold the panel");

        assert_eq!(steps.order(), ["show", "close"]);
        assert!(handoff.setup_closed);
        assert!(handoff.close_error.is_none());
        assert_eq!(handoff.record_error.as_deref(), Some(refusal.as_str()));
    }

    #[test]
    fn leaving_setup_records_nothing_and_still_hands_over() {
        let steps = Steps::default();
        let handoff = hand_over(
            false,
            || {
                steps.took("show");
                Ok(())
            },
            || {
                steps.took("record");
                Ok(())
            },
            || {
                steps.took("close");
                Ok(())
            },
        )
        .expect("the panel came up");

        // Escape, the corner mark and the dimmed screen are as easily a slip as
        // a decision: leaving stays free to run setup again.
        assert_eq!(steps.order(), ["show", "close"]);
        assert!(handoff.record_error.is_none());
    }

    /// The handoff's record step, wired to a settings store rather than to a
    /// closure a test wrote: the flag reaches the file, and every other key the
    /// file has survives it. This needed a running app before.
    #[test]
    fn the_handoff_records_completion_through_the_settings_store() {
        let settings = crate::settings::testing::in_memory();
        settings.storage.put(
            &settings.path,
            br#"{"panel":{"width":640},"stopAgentsOnQuit":true}"#,
        );
        let steps = Steps::default();

        let handoff = hand_over(
            true,
            || {
                steps.took("show");
                Ok(())
            },
            || record_completion(&settings.store),
            || {
                steps.took("close");
                Ok(())
            },
        )
        .expect("the panel came up");

        assert_eq!(steps.order(), ["show", "close"]);
        assert!(handoff.record_error.is_none());
        let saved = settings.store.load();
        assert!(saved.onboarding.completed);
        assert_eq!(saved.panel.width, 640.0);
        assert!(saved.stop_agents_on_quit);
    }

    /// Leaving setup writes nothing at all, which is what keeps it free to
    /// change its mind — checked against the file this time, not a counter.
    #[test]
    fn leaving_setup_writes_nothing_to_the_settings_file() {
        let settings = crate::settings::testing::in_memory();

        hand_over(
            false,
            || Ok(()),
            || record_completion(&settings.store),
            || Ok(()),
        )
        .expect("the panel came up");

        assert_eq!(settings.storage.get(&settings.path), None);
    }

    /// The refusal the handoff is built to survive, produced by a real settings
    /// store over a file this build cannot parse: the flag is refused, the
    /// person's bytes are still on disk, and the panel is handed over anyway.
    #[test]
    fn a_settings_file_that_will_not_parse_refuses_the_flag_and_keeps_its_bytes() {
        let settings = crate::settings::testing::in_memory();
        let original = br#"{ "panel": { "width": 640 }, "#.to_vec();
        settings.storage.put(&settings.path, &original);

        let handoff = hand_over(
            true,
            || Ok(()),
            || record_completion(&settings.store),
            || Ok(()),
        )
        .expect("a file this build cannot read is not a reason to withhold the panel");

        assert!(handoff.setup_closed);
        assert!(handoff
            .record_error
            .as_deref()
            .is_some_and(|error| error.starts_with("could not record that setup finished:")));
        assert_eq!(settings.storage.get(&settings.path), Some(original));
    }

    /// Clearing the flag is what makes the debug-only tray item a restart
    /// rather than a preview: the next launch opens setup again, because the
    /// file no longer claims it is done.
    #[test]
    fn clearing_the_first_run_flag_un_finishes_setup() {
        let settings = crate::settings::testing::in_memory();
        record_completion(&settings.store).expect("an absent file takes the change");
        assert!(settings.store.load().onboarding.completed);

        set_onboarding(&settings.store, Onboarding::default()).expect("the file takes the change");

        assert!(!settings.store.load().onboarding.completed);
    }

    #[test]
    fn a_window_that_will_not_close_is_reported_rather_than_failing_the_handoff() {
        let handoff = hand_over(
            true,
            || Ok(()),
            || Ok(()),
            || Err("could not close setup: the window server said no".to_string()),
        )
        .expect("the panel is up, whatever this window does");

        assert!(!handoff.setup_closed);
        assert_eq!(
            handoff.close_error.as_deref(),
            Some("could not close setup: the window server said no")
        );
        assert!(handoff.record_error.is_none());
    }

    fn area(width: u32, height: u32) -> WorkArea {
        WorkArea {
            x: 0,
            y: 0,
            width,
            height,
        }
    }

    #[test]
    fn fills_the_work_area_when_height_is_not_configured() {
        let frame = frame_on(area(1920, 1080), PhysicalSize::new(420, 900), true, 1.0);
        assert_eq!(frame.width, 420);
        assert_eq!(frame.height, 1080 - 40);
        assert_eq!(frame.x, 1920 - 420 - 20);
        assert_eq!(frame.y, 20);
    }

    #[test]
    fn keeps_the_current_height_when_height_is_configured() {
        let frame = frame_on(area(1920, 1080), PhysicalSize::new(500, 640), false, 1.0);
        assert_eq!(frame.width, 500);
        assert_eq!(frame.height, 640);
        assert_eq!(frame.y, 1080 - 640 - 20);
    }

    #[test]
    fn a_display_smaller_than_the_padding_does_not_underflow() {
        let frame = frame_on(area(30, 30), PhysicalSize::new(420, 900), true, 1.0);
        assert_eq!(frame.width, 1);
        assert_eq!(frame.height, 1);
    }

    #[test]
    fn a_width_below_the_minimum_opens_at_the_minimum() {
        let panel = Panel {
            width: 200.0,
            height: None,
            min_width: 420.0,
        };
        let opening = opening_size(&panel);
        assert_eq!(opening.width, 420.0);
        assert_eq!(opening.min_width, 420.0);
        assert!(opening.height.is_none());
    }

    #[test]
    fn a_height_below_the_transcript_floor_opens_at_the_floor() {
        let panel = Panel {
            width: 420.0,
            height: Some(100.0),
            min_width: 420.0,
        };
        assert_eq!(opening_size(&panel).height, Some(MIN_PANEL_HEIGHT));
    }

    #[test]
    fn an_unrealized_window_opens_at_the_configured_width() {
        let size = realized_outer(PhysicalSize::new(0, 0), 420.0, 1.0);
        let frame = frame_on(area(1920, 1080), size, true, 1.0);
        assert_eq!(frame.width, 420);
        assert_eq!(frame.height, 1080 - 40);
    }

    #[test]
    fn a_zero_height_window_is_not_resized_width_only() {
        assert_eq!(
            width_only_physical(420.0, PhysicalSize::new(420, 0), 1.0),
            None
        );
    }

    #[test]
    fn a_realized_window_keeps_its_height_on_a_width_only_fit() {
        assert_eq!(
            width_only_physical(420.0, PhysicalSize::new(800, 900), 2.0),
            Some(PhysicalSize::new(840, 900))
        );
    }

    #[test]
    fn window_min_height_is_the_transcript_floor() {
        let conf: serde_json::Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).unwrap();
        let min_height = conf["app"]["windows"][0]["minHeight"].as_f64().unwrap();
        assert_eq!(min_height, MIN_PANEL_HEIGHT);
    }
}
