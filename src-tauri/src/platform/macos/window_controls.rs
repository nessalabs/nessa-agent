//! The desktop window's traffic lights, centred on the titlebar row.
//!
//! The page draws its titlebar controls (the sidebar toggle, Back, Forward)
//! on a row whose centre is [`ROW_CENTRE`] below the window's top
//! (`--desktop-titlebar-height` in `src/desktop/styles.css`, halved). The
//! native buttons have to sit on that same line.
//!
//! Tauri's `trafficLightPosition` cannot hold them there. It grows the
//! titlebar container by a fixed offset and leaves each button where AppKit
//! put it inside, so where the buttons land depends on AppKit's own titlebar
//! layout: an offset tuned on one macOS release puts them on another line
//! after an update redraws the buttons. It is also re-applied only when its
//! view next draws, so AppKit's own relayouts (focus, a Space change, leaving
//! full screen) are left standing until then.
//!
//! So this module is the one owner of their placement, and it places them by
//! their centre, measured from the buttons themselves: whatever size and
//! offset a release gives them, their middle is put on the row's. It runs when
//! the window is shown and again after every AppKit event that relays the
//! titlebar out. `tauri.conf.json` names no `trafficLightPosition`, so nothing
//! else moves them.

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSView, NSWindow, NSWindowDidBecomeKeyNotification, NSWindowDidBecomeMainNotification,
    NSWindowDidChangeBackingPropertiesNotification, NSWindowDidChangeScreenNotification,
    NSWindowDidEndLiveResizeNotification, NSWindowDidExitFullScreenNotification,
    NSWindowDidResignKeyNotification, NSWindowDidResizeNotification,
};
use objc2_foundation::{NSNotificationCenter, NSOperationQueue, NSPoint, NSRect, NSSize};
use std::sync::atomic::{AtomicBool, Ordering};

/// The titlebar row's centre, in points below the window's top edge.
const ROW_CENTRE: f64 = 24.0;
/// The close button's left edge, in points from the window's left edge. The
/// page leaves the buttons room up to `--desktop-window-controls-inset`.
const LEFT: f64 = 16.0;

/// `NSWindowButton`: close, minimize, zoom.
const BUTTONS: [usize; 3] = [0, 1, 2];
/// `NSWindowStyleMaskFullScreen`. In full screen the buttons live in the
/// menu bar's reveal, where AppKit places them.
const FULL_SCREEN: usize = 1 << 14;

/// Places the buttons now, and keeps them placed for the window's life.
/// Watching starts once, however often the window is shown again.
pub fn hold(window: &tauri::WebviewWindow) -> Result<(), String> {
    static WATCHING: AtomicBool = AtomicBool::new(false);
    place(window)?;
    if WATCHING.swap(true, Ordering::SeqCst) {
        return Ok(());
    }
    watch(window)
}

/// Puts each button's centre on the row's, keeping AppKit's own spacing.
pub fn place(window: &tauri::WebviewWindow) -> Result<(), String> {
    let handle = window.ns_window().map_err(|error| error.to_string())?;
    let ns_window: &NSWindow = unsafe { &*handle.cast::<NSWindow>() };
    let style: usize = unsafe { msg_send![ns_window, styleMask] };
    if style & FULL_SCREEN != 0 {
        return Ok(());
    }

    let buttons: Vec<Retained<NSView>> = BUTTONS
        .iter()
        .filter_map(|&kind| unsafe { msg_send![ns_window, standardWindowButton: kind] })
        .collect();
    let [close, minimize, _] = buttons.as_slice() else {
        return Err(String::from("the window has no traffic lights"));
    };
    // The titlebar's container: the buttons' own view, and the view that holds it.
    // SAFETY: read on the main thread, where AppKit owns these views.
    let Some(row) = (unsafe { close.superview() }) else {
        return Err(String::from("the traffic lights have no titlebar view"));
    };
    let Some(container) = (unsafe { row.superview() }) else {
        return Err(String::from("the titlebar view has no container"));
    };

    // The container spans the row from the window's top, twice the centre
    // tall, so the buttons can be centred inside it rather than offset from
    // wherever AppKit left its bottom edge.
    let window_height = ns_window.frame().size.height;
    let height = 2.0 * ROW_CENTRE;
    let frame = container.frame();
    container.setFrame(NSRect::new(
        NSPoint::new(frame.origin.x, window_height - height),
        NSSize::new(frame.size.width, height),
    ));
    // The row's centre, in the container, then in the buttons' own view:
    // wherever AppKit put that view, and whichever way each counts its y.
    let centre_y = if container.isFlipped() {
        ROW_CENTRE
    } else {
        height - ROW_CENTRE
    };
    let tallest = buttons
        .iter()
        .map(|button| button.frame().size.height)
        .fold(0.0, f64::max);
    let centre = row.convertPoint_fromView(NSPoint::new(0.0, centre_y), Some(&container));
    let bounds = row.bounds();
    if centre.y - tallest / 2.0 < bounds.origin.y
        || centre.y + tallest / 2.0 > bounds.origin.y + bounds.size.height
    {
        // The buttons' view would clip them on the row: it spans the container.
        row.setFrame(container.bounds());
    }
    let centre = row.convertPoint_fromView(NSPoint::new(0.0, centre_y), Some(&container));

    let spacing = minimize.frame().origin.x - close.frame().origin.x;
    for (index, button) in buttons.iter().enumerate() {
        let size = button.frame().size;
        button.setFrameOrigin(NSPoint::new(
            LEFT + index as f64 * spacing,
            centre.y - size.height / 2.0,
        ));
    }
    Ok(())
}

/// Re-places the buttons after each AppKit event that lays the titlebar out
/// again, and once more on the next turn of the main queue, since AppKit can
/// finish its own layout after posting.
fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
    let handle = window.ns_window().map_err(|error| error.to_string())?;
    // Filtered to this one window: the panel and setup keep AppKit's placement.
    let ns_window: &AnyObject = unsafe { &*handle.cast::<AnyObject>() };
    let center = NSNotificationCenter::defaultCenter();

    let notifications = unsafe {
        [
            NSWindowDidResizeNotification,
            NSWindowDidEndLiveResizeNotification,
            NSWindowDidBecomeKeyNotification,
            NSWindowDidResignKeyNotification,
            NSWindowDidBecomeMainNotification,
            NSWindowDidExitFullScreenNotification,
            NSWindowDidChangeScreenNotification,
            NSWindowDidChangeBackingPropertiesNotification,
        ]
    };
    for notification in notifications {
        let target = window.clone();
        let block = block2::RcBlock::new(
            move |_: core::ptr::NonNull<objc2_foundation::NSNotification>| {
                // Reported, not fatal, and never a panic inside an AppKit callback:
                // a button off the row costs alignment, not the window.
                if let Err(error) = place(&target) {
                    eprintln!("[nessa] could not place the traffic lights: {error}");
                }
                let later = target.clone();
                let again = block2::RcBlock::new(move || {
                    let _ = place(&later);
                });
                unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&again) };
            },
        );
        // AppKit posts on the main thread, which is where this is observed from.
        let token = unsafe {
            center.addObserverForName_object_queue_usingBlock(
                Some(notification),
                Some(ns_window),
                None,
                &block,
            )
        };
        // The window lives as long as the app; dropping the token would
        // unregister the observer at once.
        core::mem::forget(token);
    }
    Ok(())
}
