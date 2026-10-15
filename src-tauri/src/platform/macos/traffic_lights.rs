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
//! after an update redraws the buttons.
//!
//! So this module is the one owner of their placement, and it places them by
//! their centre, measured from the buttons themselves: whatever size and
//! offset a release gives them, their middle is put on the row's. It never
//! moves a button inside the view that holds the three. AppKit moves them back
//! to its own spot in that view whenever it likes (a title change does, with
//! no layout and no frame-change notice), so a button moved there does not
//! stay. It moves that view instead, by the distance from AppKit's spot to the
//! row's, and AppKit's spot then lands on the row. It places them again when
//! the titlebar's views change frame and after the window events that relayout
//! follows.
//! `tauri.conf.json` names no `trafficLightPosition`, so nothing else moves
//! them.
//!
//! Nothing here knows about Tauri, so `tests/traffic_lights.rs` drives it on a
//! plain `NSWindow` (`window_controls.rs` is the Tauri side).

use std::cell::Cell;

use objc2::msg_send;
use objc2::rc::Retained;
use objc2::runtime::AnyObject;
use objc2_app_kit::{
    NSView, NSViewFrameDidChangeNotification, NSWindow, NSWindowDidBecomeKeyNotification,
    NSWindowDidBecomeMainNotification, NSWindowDidChangeBackingPropertiesNotification,
    NSWindowDidChangeScreenNotification, NSWindowDidDeminiaturizeNotification,
    NSWindowDidEndLiveResizeNotification, NSWindowDidExitFullScreenNotification,
    NSWindowDidResignKeyNotification, NSWindowDidResizeNotification,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSNotificationName, NSOperationQueue,
    NSPoint, NSRect, NSSize,
};

/// The titlebar row's centre, in points below the window's top edge: half of
/// `--desktop-titlebar-height` (`tests/traffic_lights.rs` holds the two equal).
pub const ROW_CENTRE: f64 = 24.0;
/// The close button's left edge, in points from the window's left edge. The
/// page leaves the buttons room up to `--desktop-window-controls-inset`.
pub const LEFT: f64 = 16.0;

/// `NSWindowButton`: close, minimize, zoom.
const BUTTONS: [usize; 3] = [0, 1, 2];
/// `NSWindowStyleMaskFullScreen`. In full screen the buttons live in the
/// menu bar's reveal, where AppKit places them.
const FULL_SCREEN: usize = 1 << 14;
/// Below this, two frames are the same frame. Writing a frame AppKit already
/// has would post a frame change and place the buttons again for nothing.
const SAME: f64 = 0.01;
thread_local! {
    /// Set while the buttons are being placed: the frame changes that placing
    /// posts are this placement's own, not AppKit's.
    static PLACING: Cell<bool> = const { Cell::new(false) };
}

/// The window's three buttons, close first.
pub fn buttons(window: &NSWindow) -> Vec<Retained<NSView>> {
    BUTTONS
        .iter()
        .filter_map(|&kind| unsafe { msg_send![window, standardWindowButton: kind] })
        .collect()
}

/// Puts each button's centre on the row's, keeping AppKit's own spacing.
pub fn place(window: &NSWindow, _main: MainThreadMarker) -> Result<(), String> {
    if PLACING.get() {
        return Ok(());
    }
    PLACING.set(true);
    let placed = place_now(window);
    PLACING.set(false);
    placed
}

fn place_now(window: &NSWindow) -> Result<(), String> {
    let style: usize = unsafe { msg_send![window, styleMask] };
    if style & FULL_SCREEN != 0 {
        return Ok(());
    }
    let buttons = buttons(window);
    let Some(close) = buttons.first() else {
        return Err(String::from("the window has no traffic lights"));
    };
    let (row, container) = titlebar_views(close)?;

    // The container spans the row from the window's top, twice the centre
    // tall, so the buttons can be centred inside it rather than offset from
    // wherever AppKit left its bottom edge.
    let height = 2.0 * ROW_CENTRE;
    let frame = container.frame();
    set_frame(
        &container,
        NSRect::new(
            NSPoint::new(frame.origin.x, window.frame().size.height - height),
            NSSize::new(frame.size.width, height),
        ),
    );

    // The buttons stay where AppKit puts them inside their own view, so
    // AppKit laying them out again (a title change does, without a word) puts
    // them back on the same spot. That view moves instead, by however far
    // AppKit's spot is from the row's: measured, whatever a release draws.
    let size = container.bounds().size;
    let now = row.frame();
    set_frame(&row, NSRect::new(now.origin, size));
    let close_in_container = container.convertRect_fromView(close.bounds(), Some(close));
    let top = if container.isFlipped() {
        close_in_container.origin.y
    } else {
        height - (close_in_container.origin.y + close_in_container.size.height)
    };
    let low = top + close_in_container.size.height / 2.0 - ROW_CENTRE;
    let right = LEFT - close_in_container.origin.x;
    let now = row.frame();
    let y = if container.isFlipped() {
        now.origin.y - low
    } else {
        now.origin.y + low
    };
    set_frame(
        &row,
        NSRect::new(NSPoint::new(now.origin.x + right, y), now.size),
    );
    Ok(())
}

/// The buttons' own view, and the titlebar container that holds it.
fn titlebar_views(close: &NSView) -> Result<(Retained<NSView>, Retained<NSView>), String> {
    // SAFETY: on the main thread (`place` takes its marker), where AppKit owns these views.
    let Some(row) = (unsafe { close.superview() }) else {
        return Err(String::from("the traffic lights have no titlebar view"));
    };
    let Some(container) = (unsafe { row.superview() }) else {
        return Err(String::from("the titlebar view has no container"));
    };
    Ok((row, container))
}

fn set_frame(view: &NSView, frame: NSRect) {
    let now = view.frame();
    let moved = (now.origin.x - frame.origin.x).abs() > SAME
        || (now.origin.y - frame.origin.y).abs() > SAME
        || (now.size.width - frame.size.width).abs() > SAME
        || (now.size.height - frame.size.height).abs() > SAME;
    if moved {
        view.setFrame(frame);
    }
}

/// Keeps the buttons placed for the window's life: again whenever one of the
/// titlebar's views changes frame, and after each window event that relayout follows, once at once and once on the next turn
/// of the main queue, since AppKit can finish its layout after posting.
pub fn watch(window: Retained<NSWindow>, main: MainThreadMarker) -> Result<(), String> {
    place(&window, main)?;
    let buttons = buttons(&window);
    let close = buttons
        .first()
        .ok_or_else(|| String::from("the window has no traffic lights"))?;
    let (row, container) = titlebar_views(close)?;

    let views: [&NSView; 3] = [&container, &row, close];
    for view in views {
        view.setPostsFrameChangedNotifications(true);
        let target = window.clone();
        observe(
            unsafe { NSViewFrameDidChangeNotification },
            view,
            move || report(place(&target, main)),
        );
    }

    let events = unsafe {
        [
            NSWindowDidResizeNotification,
            NSWindowDidEndLiveResizeNotification,
            NSWindowDidBecomeKeyNotification,
            NSWindowDidResignKeyNotification,
            NSWindowDidBecomeMainNotification,
            NSWindowDidDeminiaturizeNotification,
            NSWindowDidExitFullScreenNotification,
            NSWindowDidChangeScreenNotification,
            NSWindowDidChangeBackingPropertiesNotification,
        ]
    };
    for event in events {
        let target = window.clone();
        observe(event, &window, move || {
            report(place(&target, main));
            let later = target.clone();
            let again = block2::RcBlock::new(move || report(place(&later, main)));
            unsafe { NSOperationQueue::mainQueue().addOperationWithBlock(&again) };
        });
    }
    Ok(())
}

/// Runs `then` each time `object` posts `name`, for the app's life.
fn observe(name: &NSNotificationName, object: &AnyObject, then: impl Fn() + 'static) {
    let block = block2::RcBlock::new(move |_: core::ptr::NonNull<NSNotification>| then());
    // AppKit posts these on the main thread, where they are observed.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(name),
            Some(object),
            None,
            &block,
        )
    };
    // The window lives as long as the app; dropping the token would
    // unregister the observer at once.
    core::mem::forget(token);
}

/// A button off the row costs alignment, not the window: reported, and never
/// a panic inside an AppKit callback.
fn report(placed: Result<(), String>) {
    if let Err(error) = placed {
        eprintln!("[nessa] could not place the traffic lights: {error}");
    }
}
