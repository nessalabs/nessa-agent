//! The Tauri side of the desktop window's traffic lights: hands the window's
//! `NSWindow` to `traffic_lights.rs`, which places and keeps them.

use std::sync::atomic::{AtomicBool, Ordering};

use objc2::rc::Retained;
use objc2_app_kit::NSWindow;
use objc2_foundation::MainThreadMarker;

use super::traffic_lights;

/// Places the buttons now, and keeps them placed for the window's life.
/// Watching starts once, however often the window is shown again.
pub fn hold(window: &tauri::WebviewWindow) -> Result<(), String> {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    let main = MainThreadMarker::new()
        .ok_or_else(|| String::from("the traffic lights are placed on the main thread only"))?;
    let handle = window.ns_window().map_err(|error| error.to_string())?;
    // SAFETY: Tauri hands back the live window's NSWindow; retaining it keeps
    // it for the observers, which outlive this call.
    let ns_window: Retained<NSWindow> = unsafe { Retained::retain(handle.cast::<NSWindow>()) }
        .ok_or_else(|| String::from("the window has no NSWindow"))?;
    if WATCHING.swap(true, Ordering::SeqCst) {
        return traffic_lights::place(&ns_window, main);
    }
    let watched = traffic_lights::watch(ns_window, main);
    if watched.is_err() {
        WATCHING.store(false, Ordering::SeqCst);
    }
    watched
}
