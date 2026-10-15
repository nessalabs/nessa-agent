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
//! offset a release gives them, their middle is put on the row's. AppKit
//! moves them back to its own spot whenever it lays the titlebar out, and
//! sometimes says nothing (a title change moves them with no layout and no
//! frame-change notice). So rather than wait to be told, it checks them each
//! time the main run loop is about to sleep, after AppKit's work for that turn
//! and before the screen is drawn, and moves only a button that is off the
//! row. The titlebar's frame changes and the window events relayout follows
//! place them at once too.
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
use objc2_core_foundation::{
    kCFRunLoopCommonModes, CFRunLoop, CFRunLoopActivity, CFRunLoopObserver,
};
use objc2_foundation::{
    MainThreadMarker, NSNotification, NSNotificationCenter, NSNotificationName, NSOperationQueue,
    NSPoint, NSRect, NSSize,
};

include!(concat!(env!("OUT_DIR"), "/titlebar.rs"));

/// The titlebar row's centre, in points below the window's top edge: half of
/// the page's `--desktop-titlebar-height`, which the build script reads from
/// the stylesheet (`titlebar_geometry.rs`).
pub const ROW_CENTRE: f64 = TITLEBAR_HEIGHT / 2.0;
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

/// Where the check runs among the main run loop's other work before it
/// sleeps: after AppKit's own layout and display, just before Core Animation
/// commits the frame (2000000), so a button AppKit moved is back before the
/// screen shows it.
const BEFORE_COMMIT: isize = 1_999_999;

thread_local! {
    /// Set while the buttons are being placed: the frame changes that placing
    /// posts are this placement's own, not AppKit's.
    static PLACING: Cell<bool> = const { Cell::new(false) };
    /// AppKit's own distance from one button to the next, read before the
    /// buttons are first moved: read again later, it can catch AppKit halfway
    /// through moving the three.
    static SPACING: Cell<Option<f64>> = const { Cell::new(None) };
    /// The last error `report` printed, while it lasts.
    static REPORTED: Cell<Option<String>> = const { Cell::new(None) };
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
    // Hidden in the menu bar or in the Dock, nothing shows the buttons, and
    // the run loop check costs nothing.
    if style & FULL_SCREEN != 0 || !window.isVisible() || window.isMiniaturized() {
        return Ok(());
    }
    let buttons = buttons(window);
    let [close, minimize, _] = buttons.as_slice() else {
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

    // The row's centre, in the container, then in the buttons' own view:
    // wherever AppKit put that view, and whichever way each counts its y.
    let centre_y = if container.isFlipped() {
        ROW_CENTRE
    } else {
        height - ROW_CENTRE
    };
    let centre_in_row = || row.convertPoint_fromView(NSPoint::new(0.0, centre_y), Some(&container));
    let tallest = buttons
        .iter()
        .map(|button| button.frame().size.height)
        .fold(0.0, f64::max);
    let centre = centre_in_row();
    let bounds = row.bounds();
    if centre.y - tallest / 2.0 < bounds.origin.y
        || centre.y + tallest / 2.0 > bounds.origin.y + bounds.size.height
    {
        // The buttons' view would clip them on the row: it spans the container.
        set_frame(&row, container.bounds());
    }
    let centre = centre_in_row();

    let spacing = match SPACING.get() {
        Some(spacing) => spacing,
        None => {
            let width = close.frame().size.width;
            let spacing = minimize.frame().origin.x - close.frame().origin.x;
            if spacing > width && spacing < 3.0 * width {
                SPACING.set(Some(spacing));
                spacing
            } else {
                // Read mid-move: not kept, and AppKit's usual gap for now.
                width + 9.0
            }
        }
    };
    for (index, button) in buttons.iter().enumerate() {
        let origin = NSPoint::new(
            LEFT + index as f64 * spacing,
            centre.y - button.frame().size.height / 2.0,
        );
        let now = button.frame().origin;
        if (now.x - origin.x).abs() > SAME || (now.y - origin.y).abs() > SAME {
            button.setFrameOrigin(origin);
        }
    }
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

/// Keeps the buttons placed for the window's life: each time the main run loop
/// is about to sleep, whenever one of the titlebar's views changes frame, and
/// after each window event that relayout follows, once at once and once on
/// the next turn of the main queue, since AppKit can finish its layout after
/// posting.
pub fn watch(window: Retained<NSWindow>, main: MainThreadMarker) -> Result<(), String> {
    // First, and the one step that can fail: once the run loop checks the
    // buttons, nothing below needs to succeed for them to reach the row, even
    // if AppKit has not attached the titlebar's views yet.
    check_before_each_sleep(window.clone(), main)?;
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
    report(place(&window, main));

    // The titlebar's views may not exist yet; the run loop check covers them
    // until they do.
    let buttons = buttons(&window);
    if let Some((close, (row, container))) = buttons
        .first()
        .and_then(|close| titlebar_views(close).ok().map(|views| (close, views)))
    {
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
    }
    Ok(())
}

/// Places the buttons each time the main run loop is about to sleep, in every
/// mode (live resize and menu tracking included). Placing moves nothing when
/// the buttons are already on the row, so a quiet turn costs a few frame reads.
fn check_before_each_sleep(
    window: Retained<NSWindow>,
    main: MainThreadMarker,
) -> Result<(), String> {
    let check = block2::RcBlock::new(move |_: *mut CFRunLoopObserver, _: CFRunLoopActivity| {
        report(place(&window, main));
    });
    // SAFETY: the default allocator, and a block that lives as long as the
    // observer holds it.
    let observer = unsafe {
        CFRunLoopObserver::with_handler(
            None,
            CFRunLoopActivity::BeforeWaiting.0,
            true,
            BEFORE_COMMIT,
            Some(&check),
        )
    }
    .ok_or_else(|| String::from("the run loop observer could not be made"))?;
    let run_loop = CFRunLoop::main().ok_or_else(|| String::from("there is no main run loop"))?;
    // The run loop keeps the observer for the app's life.
    run_loop.add_observer(Some(&observer), unsafe { kCFRunLoopCommonModes });
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
/// a panic inside an AppKit callback. Placing runs every turn of the run loop,
/// so an error is reported when it starts, not on every turn it lasts.
fn report(placed: Result<(), String>) {
    let error = placed.err();
    if error.is_some() && error != REPORTED.take() {
        eprintln!(
            "[nessa] could not place the traffic lights: {}",
            error.as_deref().unwrap_or_default()
        );
    }
    REPORTED.set(error);
}
