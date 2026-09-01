//! The floating panel: its size, its place on the screen, and showing it.
//!
//! The tray and the shortcut *request* a show or a hide. They do not know how
//! the frame is fitted. The frame is reapplied on every show, so moving
//! between displays re-places the panel rather than stranding it.

use tauri::{
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow,
};

use crate::host;
use crate::settings::Settings;

const MAIN_WINDOW: &str = "main";
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

pub fn toggle(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };

    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    show(&window, &settings(app));
}

/// Places, fits, and focuses the panel, then hands the caret to the composer.
pub fn show(window: &WebviewWindow, settings: &Settings) {
    let _ = anchor_to_edge(window, settings);
    // The panel may have been summoned onto a display with more room than the
    // one it was last fitted for, and the viewport is sized for the work area
    // it is standing in. A failure costs the resize fix, not the show.
    if let Err(error) = crate::platform::current().fit_viewport(window) {
        eprintln!("[nessa] could not fit the panel's viewport: {error}");
    }
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit(host::FOCUS_COMPOSER, ());
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
    (current.height > 0).then(|| {
        PhysicalSize::new((opening_width * scale).round() as u32, current.height)
    })
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

pub fn opening_size(panel: &crate::settings::Panel) -> OpeningSize {
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
    let available_width = area
        .width
        .saturating_sub(padding.unsigned_abs() * 2)
        .max(1);

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
    use super::*;

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
        let panel = crate::settings::Panel {
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
        let panel = crate::settings::Panel {
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
