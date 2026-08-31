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
//!
//! On Linux the compositor takes the resize grab the same way macOS does, so
//! pointerup on the handle is not a reliable end. Size-allocate bursts while
//! a mouse button is down are the drag; a short quiet after the last one is
//! the release. Programmatic refits (show, display change) have no button
//! down, so they do not pin the glow.

#[cfg(any(target_os = "macos", target_os = "linux"))]
use crate::host;

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
        (unsafe { NSWindowWillStartLiveResizeNotification }, host::RESIZE_STARTED),
        (unsafe { NSWindowDidEndLiveResizeNotification }, host::RESIZE_ENDED),
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

/// Size-allocate while a button is held is a live resize; a short quiet after
/// the last one is the release. The compositor has the grab, so the page never
/// sees pointerup.
#[cfg(target_os = "linux")]
pub fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
    use gtk::glib;
    use gtk::prelude::*;
    use std::cell::{Cell, RefCell};
    use std::rc::Rc;
    use std::time::Duration;
    use tauri::Emitter;

    let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
    let live = Rc::new(Cell::new(false));
    let end = Rc::new(RefCell::new(None::<glib::SourceId>));
    let target = window.clone();

    gtk_window.connect_size_allocate({
        let live = live.clone();
        let end = end.clone();
        let target = target.clone();
        let gtk_window = gtk_window.clone();
        move |_, _| {
            if !pointer_button_down(&gtk_window) {
                return;
            }
            if !live.get() {
                live.set(true);
                let _ = target.emit(host::RESIZE_STARTED, ());
            }
            if let Some(id) = end.borrow_mut().take() {
                id.remove();
            }
            let live = live.clone();
            let target = target.clone();
            let end_slot = end.clone();
            *end.borrow_mut() = Some(glib::timeout_add_local_once(Duration::from_millis(160), move || {
                *end_slot.borrow_mut() = None;
                if live.get() {
                    live.set(false);
                    let _ = target.emit(host::RESIZE_ENDED, ());
                }
            }));
        }
    });

    arm_west_handle(&gtk_window);

    Ok(())
}

/// The page draws a 12px west handle, but `startResizeDragging` is an async
/// invoke: it arrives after the button-press, and GTK's `_NET_WM_MOVERESIZE`
/// has to run from the press itself. Tauri already does that for a 5px inset
/// on the webview. This covers the rest of the handle, using the window's
/// left edge rather than the webview's — the view is larger than the window
/// and pinned to its bottom right.
#[cfg(target_os = "linux")]
fn arm_west_handle(gtk_window: &gtk::ApplicationWindow) {
    use gtk::gdk::WindowEdge;
    use gtk::glib::{Cast, Propagation};
    use gtk::prelude::*;

    const HANDLE: f64 = 12.0;
    let Some(webview) = crate::viewport::webview_in(gtk_window.upcast_ref()) else {
        return;
    };
    let gtk_window = gtk_window.clone();
    webview.connect_button_press_event(move |_, event| {
        if event.button() != 1 {
            return Propagation::Proceed;
        }
        if gtk_window.is_decorated() || !gtk_window.is_resizable() || gtk_window.is_maximized()
        {
            return Propagation::Proceed;
        }
        let Some(gdk_window) = gtk_window.window() else {
            return Propagation::Proceed;
        };
        let (root_x, root_y) = event.root();
        let (window_x, _) = gdk_window.position();
        let client_x = root_x - window_x as f64;
        if !(0.0..HANDLE).contains(&client_x) {
            return Propagation::Proceed;
        }
        gtk_window.begin_resize_drag(
            WindowEdge::West,
            1,
            root_x as i32,
            root_y as i32,
            event.time(),
        );
        Propagation::Proceed
    });
}

#[cfg(target_os = "linux")]
fn pointer_button_down(window: &gtk::ApplicationWindow) -> bool {
    use gtk::gdk;
    use gtk::gdk::prelude::*;
    use gtk::prelude::*;

    let Some(gdk_window) = window.window() else {
        return false;
    };
    let Some(seat) = window.display().default_seat() else {
        return false;
    };
    let Some(pointer) = seat.pointer() else {
        return false;
    };
    let (_, _, _, mask) = gdk_window.device_position(&pointer);
    mask.contains(gdk::ModifierType::BUTTON1_MASK)
}

/// Elsewhere the window has a system frame of its own and the webview is given
/// the resize gesture like any other, so there is nothing to forward.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn watch(_window: &tauri::WebviewWindow) -> Result<(), String> {
    Ok(())
}
