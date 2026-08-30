//! The menu bar item and the placement of the floating panel.

use tauri::{
    menu::{CheckMenuItem, CheckMenuItemBuilder, MenuBuilder, MenuItemBuilder},
    tray::{MouseButton, MouseButtonState, TrayIconBuilder, TrayIconEvent},
    AppHandle, Emitter, LogicalSize, Manager, PhysicalPosition, PhysicalSize, WebviewWindow, Wry,
};

use crate::settings::Settings;

/// The surface control lives in the tray menu rather than in the panel's
/// header: the header is only two lines tall and the panel is dismissed by the
/// tray icon anyway, so neither control earned a permanent seat there.
///
/// The frontend stays the source of truth — it remembers the choice across
/// launches — and this item is kept in step by `set_frosted`.
pub struct SurfaceMenuItem(pub CheckMenuItem<Wry>);

/// Raised when the tray item is chosen; the frontend flips its own surface
/// state and calls back through `set_frosted`.
pub const TOGGLE_SURFACE_EVENT: &str = "nessa://toggle-surface";

/// Raised whenever the panel is summoned, so the composer takes the caret
/// without the reader having to click into it first.
pub const FOCUS_COMPOSER_EVENT: &str = "nessa://focus-composer";

const MAIN_WINDOW: &str = "main";
const TRAY_ID: &str = "nessa-tray";
/// The floor the resize edge may not drag the panel below, whatever the
/// configured minimum width is: a panel shorter than this has no transcript.
const MIN_PANEL_HEIGHT: f64 = 320.0;
/// How far the panel sits from the edges of the work area.
const EDGE_PADDING: f64 = 20.0;

pub fn create(app: &AppHandle) -> tauri::Result<()> {
    let toggle = MenuItemBuilder::with_id("toggle", "Show Nessa").build(app)?;
    let transparent = CheckMenuItemBuilder::with_id("surface", "Transparent")
        .checked(false)
        .build(app)?;
    let quit = MenuItemBuilder::with_id("quit", "Quit Nessa").build(app)?;
    let menu = MenuBuilder::new(app)
        .items(&[&toggle, &transparent])
        .separator()
        .items(&[&quit])
        .build()?;

    app.manage(SurfaceMenuItem(transparent));

    let mut builder = TrayIconBuilder::with_id(TRAY_ID)
        .menu(&menu)
        // The menu belongs to the right button; the left button summons the
        // panel, which is what a menu bar app is for.
        .show_menu_on_left_click(false)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "toggle" => toggle_panel(app),
            // The frontend owns the choice, so the click is only a request.
            "surface" => {
                let _ = app.emit(TOGGLE_SURFACE_EVENT, ());
            }
            "quit" => app.exit(0),
            _ => {}
        })
        .on_tray_icon_event(|tray, event| {
            if let TrayIconEvent::Click {
                button: MouseButton::Left,
                button_state: MouseButtonState::Up,
                ..
            } = event
            {
                toggle_panel(tray.app_handle());
            }
        });

    if let Some(icon) = app.default_window_icon() {
        builder = builder.icon(icon.clone());
    }

    builder.build(app)?;
    Ok(())
}

pub fn toggle_panel(app: &AppHandle) {
    let Some(window) = app.get_webview_window(MAIN_WINDOW) else {
        return;
    };

    if window.is_visible().unwrap_or(false) {
        let _ = window.hide();
        return;
    }

    let _ = anchor_to_edge(&window, &settings(app));
    let _ = window.show();
    let _ = window.set_focus();
    let _ = window.emit(FOCUS_COMPOSER_EVENT, ());
}

/// Reads the settings the app was launched with, falling back to the defaults
/// if they were never managed (which only happens if setup failed).
fn settings(app: &AppHandle) -> Settings {
    app.try_state::<Settings>()
        .map(|state| state.inner().clone())
        .unwrap_or_default()
}

/// Applies the configured geometry once, at startup: the opening size, and the
/// floor the resize edge may not drag below. Everything after this is the
/// reader's own, so `anchor_to_edge` only re-fits and repositions.
pub fn apply_configured_size(window: &WebviewWindow, settings: &Settings) -> tauri::Result<()> {
    let panel = &settings.panel;
    let min_width = panel.min_width.max(1.0);

    window.set_min_size(Some(LogicalSize::new(min_width, MIN_PANEL_HEIGHT)))?;

    // A configured width below the configured minimum is a contradiction; the
    // minimum wins, since it is the one the resize edge will enforce anyway.
    let width = panel.width.max(min_width);
    // Without a configured height the panel fills the work area, which
    // `anchor_to_edge` does on every show — so only the width is set here.
    if let Some(height) = panel.height {
        window.set_size(LogicalSize::new(width, height.max(MIN_PANEL_HEIGHT)))?;
    } else {
        let current = window.outer_size()?;
        let scale = window.scale_factor()?;
        window.set_size(PhysicalSize::new(
            (width * scale).round() as u32,
            current.height,
        ))?;
    }

    Ok(())
}

/// Stands the panel in the lower right of the screen it is summoned on: it
/// keeps whatever size it currently has — the configured size on the first
/// show, the reader's own after a resize — clamped to what the work area can
/// hold. A panel with no configured height fills that area instead. The frame
/// is reapplied on every show, so moving between displays re-fits it.
fn anchor_to_edge(window: &WebviewWindow, settings: &Settings) -> tauri::Result<()> {
    let Some(monitor) = window.current_monitor()?.or(window.primary_monitor()?) else {
        return Ok(());
    };

    let scale = monitor.scale_factor();
    let area = monitor.work_area();
    let padding = (EDGE_PADDING * scale).round() as i32;
    let current = window.outer_size()?;

    // A display smaller than the padding would underflow; fitting on screen
    // matters more than the margin, so the work area wins.
    let available_height = area
        .size
        .height
        .saturating_sub(padding.unsigned_abs() * 2)
        .max(1);
    let available_width = area
        .size
        .width
        .saturating_sub(padding.unsigned_abs() * 2)
        .max(1);

    let height = match settings.panel.height {
        None => available_height,
        Some(_) => current.height.min(available_height).max(1),
    };
    let width = current.width.min(available_width).max(1);

    window.set_size(PhysicalSize::new(width, height))?;
    window.set_position(PhysicalPosition::new(
        area.position.x + area.size.width as i32 - width as i32 - padding,
        // Anchored to the bottom, so a panel shorter than the screen opens in
        // the lower right rather than hanging from the top.
        area.position.y + area.size.height as i32 - height as i32 - padding,
    ))?;
    Ok(())
}
