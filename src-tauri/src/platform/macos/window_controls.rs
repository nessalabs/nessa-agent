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

/// Moves the buttons into a column that starts `by` points from the window's
/// left edge (`traffic_lights::lead`). Tauri runs the page's command on the
/// main thread, where AppKit's views move, and the result goes back to the
/// page; asked from anywhere else, the move is queued there and only its
/// queueing can be answered.
pub fn lead(window: &tauri::WebviewWindow, by: f64) -> Result<(), String> {
    if let Some(main) = MainThreadMarker::new() {
        return lead_now(window, by, main);
    }
    let target = window.clone();
    window
        .run_on_main_thread(move || {
            let led = MainThreadMarker::new()
                .ok_or_else(|| {
                    String::from("the traffic lights are placed on the main thread only")
                })
                .and_then(|main| lead_now(&target, by, main));
            if let Err(error) = led {
                eprintln!("[nessa] could not move the traffic lights: {error}");
            }
        })
        .map_err(|error| error.to_string())
}

fn lead_now(window: &tauri::WebviewWindow, by: f64, main: MainThreadMarker) -> Result<(), String> {
    let handle = window.ns_window().map_err(|error| error.to_string())?;
    // SAFETY: the live window's NSWindow, borrowed for this call on the main thread.
    let ns_window = unsafe { &*handle.cast::<NSWindow>() };
    traffic_lights::lead(ns_window, by, main)
}
