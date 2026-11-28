//! The desktop window: Nessa's main app window, beside the menu bar panel.
//!
//! It is declared hidden in `tauri.conf.json` and opened here. While it is on
//! screen the app has a Dock icon and a place in the app switcher; dismissing
//! it hides it and returns the app to the menu bar alone, the way Alfred's
//! preferences window comes and goes. The panel is unaffected either way.

use tauri::{AppHandle, LogicalSize, Manager, WebviewWindow, Window};

use crate::platform;

/// The window's label, shared with `tauri.conf.json` and `capabilities/desktop.json`.
pub const DESKTOP_WINDOW: &str = "desktop";

/// Shows the desktop window and brings it forward, giving the app its Dock icon.
pub fn open(app: &AppHandle) {
    let Some(window) = app.get_webview_window(DESKTOP_WINDOW) else {
        crate::host_log::warn("there is no desktop window to open");
        return;
    };
    // Dock presence first: an Accessory app cannot become the active app, so
    // focusing the window before the policy changes would leave it behind.
    platform::current().set_dock_presence(app, true);
    if let Err(error) = window.show() {
        crate::host_log::warn(format!("could not show the desktop window: {error}"));
        return;
    }
    crate::host_log::window_shown(DESKTOP_WINDOW);
    if let Err(error) = platform::current().hold_window_controls(&window) {
        crate::host_log::warn(format!("could not place the window controls: {error}"));
    }
    let _ = window.unminimize();
    let _ = window.set_focus();
}

/// Hides the desktop window instead of destroying it, so it can open again,
/// and takes the Dock icon away with it.
pub fn dismiss(window: &Window) {
    if let Err(error) = window.hide() {
        crate::host_log::warn(format!("could not hide the desktop window: {error}"));
        return;
    }
    platform::current().set_dock_presence(window.app_handle(), false);
}

/// Every zoom the page may ask for: `steps` in `src/host/window-zoom.json`,
/// the one list the page steps through too (`src/desktop/model/window-zoom.ts`).
fn zoom_steps() -> Vec<f64> {
    let zoom: serde_json::Value =
        serde_json::from_str(include_str!("../../src/host/window-zoom.json"))
            .expect("the window's zoom steps are JSON");
    zoom["steps"]
        .as_array()
        .expect("steps is a list")
        .iter()
        .map(|step| step.as_f64().expect("a step is a number"))
        .collect()
}

/// Only the desktop window zooms: the panel and an app's window keep their size.
fn may_zoom(label: &str) -> Result<(), String> {
    if label == DESKTOP_WINDOW {
        Ok(())
    } else {
        Err(String::from("only the desktop window zooms"))
    }
}

/// `zoom` as the page may ask for it: exactly one of the steps.
fn allowed_zoom(zoom: f64) -> Result<f64, String> {
    zoom_steps()
        .into_iter()
        .find(|step| *step == zoom)
        .ok_or_else(|| format!("{zoom} is not one of the window's zoom steps"))
}

/// The smallest the window may be made at `zoom`, from its smallest at 100%:
/// the page lays itself out in its own pixels, which a zoom makes larger, so
/// the same layout needs that much more of the screen.
fn min_size_at(zoom: f64, at_rest: (f64, f64)) -> (f64, f64) {
    (at_rest.0 * zoom, at_rest.1 * zoom)
}

/// The size a window of `size` grows to so that it is no smaller than `min`,
/// or none when it already is: setting a smallest size stops a later drag, and
/// does not move a window that is already under it.
fn grown_to(size: (f64, f64), min: (f64, f64)) -> Option<(f64, f64)> {
    let grown = (size.0.max(min.0), size.1.max(min.1));
    (grown != size).then_some(grown)
}

/// Zooms the desktop window's page, as a browser zooms one: every length the
/// page draws grows or shrinks by `zoom`. What the host owns and lines up
/// with the page follows: the window's own controls move to the taller or
/// shorter titlebar row, and the window's smallest size scales (a window
/// already under it grows to it), so a zoomed page is never squeezed below
/// the layout it was built for.
///
/// Only the desktop window zooms, and only to a step of
/// `src/host/window-zoom.json`; anything else is refused before the page is
/// touched. The page remembers the zoom and asks for it again at each launch.
#[tauri::command]
pub fn desktop_zoom(window: WebviewWindow, zoom: f64) -> Result<(), String> {
    may_zoom(window.label())?;
    let zoom = allowed_zoom(zoom)?;
    window.set_zoom(zoom).map_err(|error| error.to_string())?;
    // The page is zoomed from here on, and is told so whatever becomes of
    // what follows it: a control or a size the host could not move is the
    // host's to report, and refusing here would leave the page unaware of a
    // zoom it already has.
    if let Err(error) = platform::current().zoom_window_controls(&window, zoom) {
        crate::host_log::warn(format!(
            "could not move the desktop window's controls for its zoom: {error}"
        ));
    }
    let at_rest = window
        .config()
        .app
        .windows
        .iter()
        .find(|declared| declared.label == DESKTOP_WINDOW)
        .and_then(|declared| declared.min_width.zip(declared.min_height));
    if let Some(at_rest) = at_rest {
        if let Err(error) = keep_room(&window, min_size_at(zoom, at_rest)) {
            crate::host_log::warn(format!(
                "could not size the desktop window for its zoom: {error}"
            ));
        }
    }
    Ok(())
}

/// Makes `min` the window's smallest size, and grows a window under it to it.
fn keep_room(window: &WebviewWindow, min: (f64, f64)) -> tauri::Result<()> {
    window.set_min_size(Some(LogicalSize::new(min.0, min.1)))?;
    let size = window
        .inner_size()?
        .to_logical::<f64>(window.scale_factor()?);
    if let Some((width, height)) = grown_to((size.width, size.height), min) {
        window.set_size(LogicalSize::new(width, height))?;
    }
    Ok(())
}

/// What closing the desktop window does, by whether there is a menu bar item
/// to open it again and whether the platform has a Dock to reopen it from.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum OnClose {
    /// Hidden, Dock icon and all: the menu bar item opens it again.
    Dismiss,
    /// Hidden, its Dock icon kept: clicking the Dock (`RunEvent::Reopen`)
    /// opens it again, and the app menu can still quit.
    Hide,
    /// Nothing could open it again, so the app quits rather than run on with
    /// no window and no way back.
    Quit,
}

/// Decides what closing the desktop window does. A window let close is gone
/// for good — `open` finds only windows that exist — so it is never let close
/// while the app runs on.
pub fn on_close(tray: bool, dock: bool) -> OnClose {
    match (tray, dock) {
        (true, _) => OnClose::Dismiss,
        (false, true) => OnClose::Hide,
        (false, false) => OnClose::Quit,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_a_step_of_the_shared_list_is_a_zoom() {
        let steps = zoom_steps();
        assert!(steps.contains(&1.0), "100% is a step");
        assert!(
            steps.windows(2).all(|pair| pair[0] < pair[1]),
            "smallest first"
        );
        for step in &steps {
            assert_eq!(allowed_zoom(*step), Ok(*step));
        }
        for refused in [0.0, -1.0, 1.05, 3.0, f64::NAN, f64::INFINITY] {
            assert!(allowed_zoom(refused).is_err(), "{refused} is refused");
        }
    }

    #[test]
    fn only_the_desktop_window_zooms() {
        assert_eq!(may_zoom(DESKTOP_WINDOW), Ok(()));
        for other in ["panel", "", "desktop-2"] {
            assert!(may_zoom(other).is_err(), "{other:?} is refused");
        }
    }

    #[test]
    fn the_smallest_window_scales_with_the_zoom() {
        assert_eq!(min_size_at(1.0, (800.0, 480.0)), (800.0, 480.0));
        assert_eq!(min_size_at(1.25, (800.0, 480.0)), (1000.0, 600.0));
        assert_eq!(min_size_at(0.5, (800.0, 480.0)), (400.0, 240.0));
    }

    #[test]
    fn a_window_under_its_new_smallest_size_grows_to_it_and_no_other_moves() {
        assert_eq!(grown_to((1100.0, 760.0), (1000.0, 600.0)), None);
        assert_eq!(grown_to((1000.0, 600.0), (1000.0, 600.0)), None);
        assert_eq!(
            grown_to((900.0, 760.0), (1000.0, 600.0)),
            Some((1000.0, 760.0))
        );
        assert_eq!(
            grown_to((800.0, 480.0), (1200.0, 720.0)),
            Some((1200.0, 720.0))
        );
    }

    #[test]
    fn with_a_tray_it_is_dismissed() {
        assert_eq!(on_close(true, true), OnClose::Dismiss);
        assert_eq!(on_close(true, false), OnClose::Dismiss);
    }

    #[test]
    fn without_a_tray_the_dock_can_reopen_it_so_it_is_only_hidden() {
        assert_eq!(on_close(false, true), OnClose::Hide);
    }

    #[test]
    fn without_a_tray_or_a_dock_nothing_could_reopen_it_so_the_app_quits() {
        assert_eq!(on_close(false, false), OnClose::Quit);
    }
}
