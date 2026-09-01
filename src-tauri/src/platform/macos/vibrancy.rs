//! Native frost: an `NSVisualEffectView` behind the webview.
//!
//! On a transparent, undecorated macOS window a CSS `backdrop-filter` does
//! not sample the behind-window content 1:1: it stretches it, which showed up
//! as a blue-to-black bleed running a few hundred points down the panel
//! regardless of what was actually behind it. The system material does both
//! the blur and the tint correctly, and stays live when the window is not
//! frontmost.

use window_vibrancy::{
    apply_vibrancy, clear_vibrancy, NSVisualEffectMaterial, NSVisualEffectState,
};

/// Must match the panel's CSS corner radius, so the blur is clipped to the
/// rounded corners instead of poking out past them.
const PANEL_RADIUS: f64 = 18.0;

pub fn set(window: &tauri::WebviewWindow, frosted: bool) -> Result<(), String> {
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
