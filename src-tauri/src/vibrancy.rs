//! The panel's frosted surface.
//!
//! macOS draws this natively, with an `NSVisualEffectView` behind the webview,
//! rather than with a CSS `backdrop-filter`. On a transparent, undecorated
//! window the CSS filter does not sample the behind-window content 1:1: it
//! stretches it, which showed up as a blue-to-black bleed running a few hundred
//! points down the panel regardless of what was actually behind it.

/// Must match the panel's CSS corner radius, so the blur is clipped to the
/// rounded corners instead of poking out past them.
const PANEL_RADIUS: f64 = 18.0;

#[cfg(target_os = "macos")]
pub fn set(window: &tauri::WebviewWindow, frosted: bool) -> Result<(), String> {
    use window_vibrancy::{
        apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
    };

    // Every change starts from a clean window. `apply_vibrancy` adds an effect
    // view rather than replacing one, so applying twice — as setup and the
    // frontend's first render both did — stacks two, and a single clear then
    // removes only one and leaves the panel frosted in the clear surface.
    // The bool says whether anything was there to clear, which is not
    // interesting here.
    clear_vibrancy(window).map_err(|error| error.to_string())?;

    if !frosted {
        // Removing the effect view leaves the NSWindow holding the opaque
        // background that effect was painted over, so the clear surface still
        // rendered as a filled panel. Put the window back to fully
        // transparent explicitly rather than trusting the removal to do it.
        return window
            .set_background_color(Some(tauri::window::Color(0, 0, 0, 0)))
            .map_err(|error| error.to_string());
    }

    apply_vibrancy(
        window,
        // The system material for floating utility surfaces; it adapts its
        // blur and tint to the light and dark appearance on its own.
        NSVisualEffectMaterial::HudWindow,
        // Stays blurred even when another app takes focus, so the panel does
        // not turn flat grey the moment it is not frontmost.
        Some(NSVisualEffectState::Active),
        Some(PANEL_RADIUS),
    )
    .map_err(|error| error.to_string())
}

/// Everywhere else the window simply stays transparent and the frontend paints
/// its own surface, so there is nothing to apply and nothing to fail.
#[cfg(not(target_os = "macos"))]
pub fn set(_window: &tauri::WebviewWindow, _frosted: bool) -> Result<(), String> {
    Ok(())
}

/// Lets the frontend follow its own surface toggle: the frosted surface is a
/// window-level effect, so the clear surface has to turn it off natively as
/// well as in CSS. The tray's check mark is reflected from the same call, which
/// keeps the frontend the single source of truth for the choice.
#[tauri::command]
pub fn set_frosted(
    app: tauri::AppHandle,
    window: tauri::WebviewWindow,
    frosted: bool,
) -> Result<(), String> {
    use tauri::Manager;

    set(&window, frosted)?;

    if let Some(item) = app.try_state::<crate::tray::SurfaceMenuItem>() {
        let _ = item.0.set_checked(!frosted);
    }

    Ok(())
}
