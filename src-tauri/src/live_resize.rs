//! Tells the frontend when the window is being live-resized.
//!
//! The panel's border glow has to stay lit for the length of a resize drag,
//! and the page cannot work out for itself when one is running: on a
//! borderless resizable window macOS claims the frame before the webview sees
//! it, so no `pointerdown` ever arrives, and the page is handed no pointer
//! events at all until the gesture is over. What does arrive is a stream of
//! size events — but only while the size is actually changing, which a
//! west-only drag stops doing the moment the pointer moves vertically, and a
//! paused drag stops doing altogether. Every signal derivable in the page is
//! therefore a guess at something AppKit knows exactly.
//!
//! So it is asked. `NSWindow` posts a notification either side of a live
//! resize, and those are forwarded verbatim. They also distinguish a drag from
//! a programmatic resize — the tray re-fitting the panel as it is summoned —
//! which no amount of counting size events reliably does.

/// Emitted as the user takes hold of the window's frame.
pub const STARTED: &str = "nessa://resize-started";
/// Emitted as they let go of it.
pub const ENDED: &str = "nessa://resize-ended";

#[cfg(target_os = "macos")]
pub fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{
        NSWindowDidEndLiveResizeNotification, NSWindowWillStartLiveResizeNotification,
    };
    use objc2_foundation::NSNotificationCenter;
    use tauri::Emitter;

    let handle = window.ns_window().map_err(|error| error.to_string())?;
    // The notifications are filtered to this one window, so the panel does not
    // react to any other window the app may come to own.
    let ns_window: &AnyObject = unsafe { &*handle.cast::<AnyObject>() };
    let center = NSNotificationCenter::defaultCenter();

    for (notification, event) in [
        (unsafe { NSWindowWillStartLiveResizeNotification }, STARTED),
        (unsafe { NSWindowDidEndLiveResizeNotification }, ENDED),
    ] {
        let target = window.clone();
        let block = block2::RcBlock::new(move |_: core::ptr::NonNull<objc2_foundation::NSNotification>| {
            // Nothing downstream can recover from the glow missing an edge of
            // the drag, and the alternative is a panic inside an AppKit
            // callback, so a failed emit is dropped.
            let _ = target.emit(event, ());
        });
        // AppKit posts on the main thread, which is where the notification is
        // observed from, so no queue is asked for.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(notification),
                Some(ns_window),
                None,
                &block,
            )
        };
        // The observer lives as long as the window, which lives as long as the
        // app; holding the token only to drop it at the end of `setup` would
        // unregister it immediately.
        core::mem::forget(token);
    }

    Ok(())
}

/// Elsewhere the window has a system frame of its own and the webview is given
/// the resize gesture like any other, so there is nothing to forward.
#[cfg(not(target_os = "macos"))]
pub fn watch(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}
