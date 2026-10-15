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
//! offset a release gives them, their middle is put on the row's. It places
//! them again whenever AppKit lays the titlebar out. AppKit does that in the
//! window's frame view's `layout`, and puts the buttons back where it wants
//! them without posting anything (a title change does), so the frame view
//! places them at the end of each of its layouts; the window events that
//! relayout follows place them too.
//! `tauri.conf.json` names no `trafficLightPosition`, so nothing else moves
//! them.
//!
//! Nothing here knows about Tauri, so `tests/traffic_lights.rs` drives it on a
//! plain `NSWindow` (`window_controls.rs` is the Tauri side).

use std::cell::Cell;
use std::ffi::CString;
use std::sync::atomic::{AtomicUsize, Ordering};

use objc2::rc::Retained;
use objc2::runtime::{AnyClass, AnyObject, ClassBuilder, Sel};
use objc2::{msg_send, sel};
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
/// The name of the frame view class that places the buttons after it lays
/// out: this prefix, then the name of the class it extends.
const HOLDING: &str = "NessaTrafficLightsHolding_";

/// How many layouts the held frame views have run: a placement that made AppKit
/// lay out again, and so on, would show here as a count that keeps climbing.
pub static LAYOUTS: AtomicUsize = AtomicUsize::new(0);

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

    let spacing = minimize.frame().origin.x - close.frame().origin.x;
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

/// Keeps the buttons placed for the window's life: again after each layout of
/// the window's frame view, whenever one of the titlebar's views changes
/// frame, and after each window event that relayout follows, once at once and
/// once on the next turn of the main queue, since AppKit can finish its layout
/// after posting.
pub fn watch(window: Retained<NSWindow>, main: MainThreadMarker) -> Result<(), String> {
    place(&window, main)?;
    hold_frame_view(&window)?;
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

/// Makes the window's frame view place the buttons at the end of each of its
/// layouts, the way key-value observing extends an object: the view becomes an
/// instance of a subclass of its own class whose `layout` calls the original,
/// then places them.
fn hold_frame_view(window: &NSWindow) -> Result<(), String> {
    let content = window
        .contentView()
        .ok_or_else(|| String::from("the window has no content view"))?;
    // SAFETY: on the main thread (`watch` is reached through `place`'s marker).
    let frame = unsafe { content.superview() }
        .ok_or_else(|| String::from("the window has no frame view"))?;
    let class = AnyObject::class(&frame);
    if class.name().to_string_lossy().starts_with(HOLDING) {
        return Ok(());
    }
    let name = CString::new(format!("{HOLDING}{}", class.name().to_string_lossy()))
        .map_err(|error| error.to_string())?;
    let holding = match AnyClass::get(&name) {
        Some(holding) => holding,
        None => {
            let mut builder = ClassBuilder::new(&name, class)
                .ok_or_else(|| String::from("the frame view's class cannot be extended"))?;
            // SAFETY: both match the methods they override: `- (void)layout`
            // and `- (Class)class`.
            unsafe {
                builder.add_method(
                    sel!(layout),
                    layout_then_place as unsafe extern "C-unwind" fn(*mut AnyObject, Sel),
                );
                builder.add_method(
                    sel!(class),
                    own_class
                        as unsafe extern "C-unwind" fn(*mut AnyObject, Sel) -> *const AnyClass,
                );
            }
            builder.register()
        }
    };
    // SAFETY: the subclass adds no instance variables, so the view's memory
    // is the same shape under it.
    unsafe { AnyObject::set_class(&frame, holding) };
    Ok(())
}

/// The class a holding frame view was before it was held.
fn original(view: &AnyObject) -> &'static AnyClass {
    let mut class = view.class();
    while !class.name().to_string_lossy().starts_with(HOLDING) {
        // Something extended the view again after it was held.
        class = class
            .superclass()
            .expect("a held view's class extends the holding class");
    }
    class
        .superclass()
        .expect("the holding class extends the frame view's class")
}

unsafe extern "C-unwind" fn layout_then_place(view: *mut AnyObject, _: Sel) {
    // SAFETY: AppKit sends `layout` to a live view.
    let view = unsafe { &*view };
    let class = original(view);
    LAYOUTS.fetch_add(1, Ordering::Relaxed);
    unsafe {
        let _: () = msg_send![super(view, class), layout];
    }
    let Some(main) = MainThreadMarker::new() else {
        return;
    };
    let window: Option<Retained<NSWindow>> = unsafe { msg_send![view, window] };
    if let Some(window) = window {
        report(place(&window, main));
    }
}

/// Answers `class` with the class the view was, as key-value observing does,
/// so AppKit's checks of the view's class see what they made.
unsafe extern "C-unwind" fn own_class(view: *mut AnyObject, _: Sel) -> *const AnyClass {
    // SAFETY: `class` is sent to a live view.
    original(unsafe { &*view })
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
