//! The page's viewport, held still while the window resizes around it.
//!
//! macOS runs the resize gesture itself, and it holds the edge opposite the one
//! being dragged exactly where it is: pull the top edge down and the bottom of
//! the window does not move by a pixel. The composer sits against that bottom
//! edge, so it should not move either. It moved — by up to 90pt, in a sawtooth
//! that reset every time the page caught up.
//!
//! The reason is in WebKit, and it rules out fixing this in the page. A
//! `WKWebView` renders out of process and hangs the last frame the web process
//! committed off an internal, flipped subview (`WebViewImpl.mm` calls it
//! `m_layerHostingView`). Two properties of that subview decide everything:
//!
//! * the committed frame is positioned from the view's **top left**, so
//!   anything the page lays out against the bottom of the viewport is drawn at
//!   *committed height* below that corner — not at the window's bottom edge;
//! * it is clipped to the view's **current** bounds, so a view that has already
//!   been shrunk cuts off whatever part of the older, taller frame has not
//!   caught up yet.
//!
//! AppKit resizes the view synchronously on every mouse event of the drag; the
//! web process answers some frames later. In between, the corner every drawn
//! pixel hangs from has moved and the pixels have not — which is the
//! displacement, exactly. No arrangement of the page avoids it, because the two
//! properties together mean the correction would have to be applied to content
//! that has already been committed: bottom-anchoring it in CSS just moves it to
//! *old* height minus offset, and any counter-transform the page applies
//! arrives in the same commit as the relayout it was meant to compensate for.
//!
//! So the view is not resized. It is given a frame as large as the work area,
//! pinned to the bottom right of the window, and left there. The window grows
//! and shrinks over the top of it while the page's viewport — and with it the
//! corner every committed frame hangs from — stays exactly where it was, so
//! nothing already drawn can be displaced by a resize, however far behind the
//! web process falls. The bottom right is the corner to pin because that is the
//! one the panel is anchored to: it is stood in the lower right of the screen
//! and dragged by its top and left edges, which are the edges macOS moves.
//!
//! What the page gives up is learning the window's size from its own viewport,
//! which no longer changes. It is told instead, over the size events it already
//! subscribes to, and draws the panel that size against the bottom right of the
//! viewport (see `use-panel-frame.ts`). A slow relayout then shows as the
//! panel's *dragged* edge trailing the window's for a frame or two, at the edge
//! under the cursor, where it reads as the drag catching up rather than as the
//! contents coming loose.

/// Carries the window's size to the page, which can no longer measure it.
///
/// Nothing in the page can: its own viewport is the stage, and Tauri's window
/// size APIs read the webview's view, which is the very thing this module
/// stops resizing — `innerSize()` reports the stage too. AppKit's content rect
/// is the one description of the window left that a fixed webview cannot
/// falsify, so it is read here and sent.
use crate::host::{self, PanelSize};

/// The window's size now, for a page that has just loaded and has no size event
/// coming — a devtools reload mid-session, or a webview that mounted after the
/// panel was last fitted.
#[cfg(target_os = "macos")]
#[tauri::command]
pub fn panel_size(window: tauri::WebviewWindow) -> Result<PanelSize, String> {
    use objc2_app_kit::NSWindow;

    let handle = window.ns_window().map_err(|error| error.to_string())?;
    let ns_window: &NSWindow = unsafe { &*handle.cast::<NSWindow>() };
    ns_window
        .contentView()
        .map(|content| size_of(&content))
        .ok_or_else(|| String::from("the window has no content view"))
}

#[cfg(not(target_os = "macos"))]
#[tauri::command]
pub fn panel_size(window: tauri::WebviewWindow) -> Result<PanelSize, String> {
    #[cfg(target_os = "linux")]
    {
        let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
        return gtk_panel_size(&gtk_window)
            .ok_or_else(|| String::from("the window has no size"));
    }
    #[cfg(not(target_os = "linux"))]
    host::logical_inner_size(&window)
}

#[cfg(target_os = "macos")]
fn size_of(content: &objc2_app_kit::NSView) -> PanelSize {
    let bounds = content.bounds();
    PanelSize {
        width: bounds.size.width,
        height: bounds.size.height,
    }
}

/// Sizes the webview to the stage and pins it to the window's bottom right.
///
/// Call this whenever the window may have gained room to grow into — at setup,
/// and on every show, since the panel can be summoned onto a different display
/// from the one it was last fitted for.
#[cfg(target_os = "macos")]
pub fn fit(window: &tauri::WebviewWindow) -> Result<(), String> {
    use objc2_app_kit::{NSAutoresizingMaskOptions, NSWindow};
    use objc2_foundation::{NSPoint, NSRect, NSSize};

    let handle = window.ns_window().map_err(|error| error.to_string())?;
    let ns_window: &NSWindow = unsafe { &*handle.cast::<NSWindow>() };
    let Some(content) = ns_window.contentView() else {
        return Err(String::from("the window has no content view"));
    };
    let Some(webview) = webview_in(&content) else {
        return Err(String::from("no webview under the window's content view"));
    };

    let bounds = content.bounds();
    // The stage is the largest the panel can be — the work area — but never
    // smaller than the window currently is, so a drag that pulls the frame past
    // the work area still lands on a viewport that covers it rather than on a
    // strip of nothing along the edge being dragged.
    let mut width = bounds.size.width;
    let mut height = bounds.size.height;
    if let Some(monitor) = current_monitor(window)? {
        let scale = monitor.scale_factor();
        let area = monitor.work_area();
        width = width.max(f64::from(area.size.width) / scale);
        height = height.max(f64::from(area.size.height) / scale);
    }

    // A view whose superview is flipped counts its margins from the other end,
    // and `WryWebViewParent` is a plain `NSView` today — but the whole point of
    // the frame is which corner it is pinned to, so it is asked rather than
    // assumed.
    let flipped = content.isFlipped();
    let origin = NSPoint::new(
        bounds.size.width - width,
        if flipped {
            bounds.size.height - height
        } else {
            0.0
        },
    );
    // Neither dimension is sizable: AppKit keeps the right and bottom margins it
    // is given — both zero — and lets the left and top ones absorb every resize,
    // which is what holds the pinned corner still without a line of per-frame
    // code.
    let mask = NSAutoresizingMaskOptions::ViewMinXMargin
        | if flipped {
            NSAutoresizingMaskOptions::ViewMinYMargin
        } else {
            NSAutoresizingMaskOptions::ViewMaxYMargin
        };

    webview.setAutoresizingMask(mask);
    webview.setFrame(NSRect::new(origin, NSSize::new(width, height)));

    // The page draws the panel from this, so it is sent with the placement
    // rather than left for the next resize: a panel summoned at a size it was
    // already at raises no resize at all.
    use tauri::Emitter;
    let _ = window.emit(host::PANEL_SIZED, size_of(&content));

    Ok(())
}

/// Carries the window's size to the page on every resize, and re-fits the stage
/// if the window is ever dragged past it.
///
/// The stage is sized for the work area, which is as large as the panel is ever
/// summoned; macOS will nonetheless let a drag pull the frame past it, and a
/// window larger than its own viewport draws nothing along the edge that
/// overhangs. Growing the stage costs one relayout, so it is done only when the
/// window has actually outgrown it — never on an ordinary resize step, which is
/// the case this whole module exists to leave alone.
#[cfg(target_os = "macos")]
pub fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
    use objc2::runtime::AnyObject;
    use objc2_app_kit::{NSWindow, NSWindowDidResizeNotification};
    use objc2_foundation::NSNotificationCenter;

    let handle = window.ns_window().map_err(|error| error.to_string())?;
    // Filtered to this one window, so the panel does not react to any other
    // window the app may come to own.
    let ns_window: &AnyObject = unsafe { &*handle.cast::<AnyObject>() };

    let target = window.clone();
    let block = block2::RcBlock::new(
        move |_: core::ptr::NonNull<objc2_foundation::NSNotification>| {
            let Ok(handle) = target.ns_window() else {
                return;
            };
            let ns_window: &NSWindow = unsafe { &*handle.cast::<NSWindow>() };
            let Some(content) = ns_window.contentView() else {
                return;
            };
            let Some(webview) = webview_in(&content) else {
                return;
            };

            // Every step, because this is the page's only account of the
            // window's size while it is being dragged.
            use tauri::Emitter;
            let _ = target.emit(host::PANEL_SIZED, size_of(&content));

            let stage = webview.frame().size;
            let window = content.bounds().size;
            if window.width <= stage.width && window.height <= stage.height {
                return;
            }

            // Losing the stage costs the edge being dragged, not the drag, so a
            // failure here is reported rather than fatal — and reported from
            // inside an AppKit callback, where a panic would take the app with
            // it.
            if let Err(error) = fit(&target) {
                eprintln!("[nessa] could not re-fit the panel's viewport: {error}");
            }
        },
    );

    // AppKit posts on the main thread, which is where the notification is
    // observed from, so no queue is asked for.
    let token = unsafe {
        NSNotificationCenter::defaultCenter().addObserverForName_object_queue_usingBlock(
            Some(NSWindowDidResizeNotification),
            Some(ns_window),
            None,
            &block,
        )
    };
    // The observer lives as long as the window, which lives as long as the app;
    // holding the token only to drop it at the end of `setup` would unregister
    // it immediately.
    core::mem::forget(token);

    Ok(())
}

/// The webview under the window's content view.
///
/// It is found by walking rather than indexing: the frosted surface puts an
/// `NSVisualEffectView` under the webview (see `vibrancy.rs`), so which subview
/// comes first depends on whether the surface is currently frosted.
///
/// The class is looked up by name rather than by depending on `objc2-web-kit`.
/// It is certainly registered — wry built the view out of it — and naming the
/// crate for one `isKindOfClass` would add two more to the build that nothing
/// else in the binary needs.
#[cfg(target_os = "macos")]
fn webview_in(
    content: &objc2_app_kit::NSView,
) -> Option<objc2::rc::Retained<objc2_app_kit::NSView>> {
    use objc2::runtime::{AnyClass, NSObjectProtocol};

    let class = AnyClass::get(c"WKWebView")?;
    let subviews = content.subviews();
    (0..subviews.count())
        .map(|index| subviews.objectAtIndex(index))
        .find(|view| view.isKindOfClass(class))
}

/// The display the panel is on, falling back to the primary one — the same
/// choice `panel::anchor_to_edge` makes when it places the panel.
#[cfg(target_os = "macos")]
fn current_monitor(window: &tauri::WebviewWindow) -> Result<Option<tauri::Monitor>, String> {
    let current = window.current_monitor().map_err(|e| e.to_string())?;
    match current {
        Some(monitor) => Ok(Some(monitor)),
        None => window.primary_monitor().map_err(|e| e.to_string()),
    }
}

#[cfg(target_os = "linux")]
thread_local! {
    static FITTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

/// WebKitGTK hangs its last committed frame off the widget's top-left the
/// same way WKWebView does, so the same pin applies: the webview is sized to
/// the work area, placed in a `GtkFixed` at the window's bottom right, and
/// left there. The window grows and shrinks over it. A `GtkBox` child would
/// be stretched with the window and the page's viewport would move — which is
/// the jitter this module exists to prevent.
///
/// The Fixed *replaces* tao's default vbox as the window's child. Nesting it
/// inside the box would put three widgets between the webview and the
/// GtkWindow, and Tauri's undecorated resize handler does
/// `webview.parent().parent().downcast::<gtk::Window>().unwrap()` on every
/// click. A GtkBox there panics; GTK callbacks cannot unwind, so the process
/// aborts.
#[cfg(target_os = "linux")]
pub fn fit(window: &tauri::WebviewWindow) -> Result<(), String> {
    use gtk::glib::Cast;

    let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
    let Some(webview) = webview_in(gtk_window.upcast_ref()) else {
        return Err(String::from("no webview under the window"));
    };
    let fixed = ensure_fixed(&webview)?;
    let stage = stage_pixels(window, &gtk_window)?;
    place(&fixed, &webview, &gtk_window, stage);

    use tauri::Emitter;
    if let Some(size) = gtk_panel_size(&gtk_window) {
        let _ = window.emit(host::PANEL_SIZED, size);
    }
    Ok(())
}

#[cfg(target_os = "linux")]
pub fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
    use gtk::prelude::*;
    use tauri::Emitter;

    let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
    let target = window.clone();
    gtk_window.connect_size_allocate(move |gtk_window, _| {
        if FITTING.with(|flag| flag.get()) {
            return;
        }
        FITTING.with(|flag| flag.set(true));
        if let Some(size) = gtk_panel_size(gtk_window) {
            let _ = target.emit(host::PANEL_SIZED, size);
        }
        if let Err(error) = grow_stage_if_needed(&target) {
            eprintln!("[nessa] could not re-fit the panel's viewport: {error}");
        }
        FITTING.with(|flag| flag.set(false));
    });
    Ok(())
}

#[cfg(target_os = "linux")]
fn grow_stage_if_needed(window: &tauri::WebviewWindow) -> Result<(), String> {
    use gtk::glib::Cast;
    use gtk::prelude::*;

    let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
    let Some(webview) = webview_in(gtk_window.upcast_ref()) else {
        return Ok(());
    };
    let Ok(fixed) = webview
        .parent()
        .and_then(|parent| parent.downcast::<gtk::Fixed>().ok())
        .ok_or(())
    else {
        return Ok(());
    };
    let alloc = gtk_window.allocation();
    let (stage_w, stage_h) = webview.size_request();
    if alloc.width() <= stage_w && alloc.height() <= stage_h {
        place(&fixed, &webview, &gtk_window, (stage_w, stage_h));
        return Ok(());
    }
    fit(window)
}

#[cfg(target_os = "linux")]
fn stage_pixels(
    window: &tauri::WebviewWindow,
    gtk_window: &gtk::ApplicationWindow,
) -> Result<(i32, i32), String> {
    use gtk::prelude::*;

    let alloc = gtk_window.allocation();
    let mut width = alloc.width().max(1);
    let mut height = alloc.height().max(1);
    if let Some(monitor) = current_monitor(window)? {
        let scale = gtk_window.scale_factor().max(1);
        let area = monitor.work_area();
        width = width.max(area.size.width as i32 / scale);
        height = height.max(area.size.height as i32 / scale);
    }
    Ok((width, height))
}

#[cfg(target_os = "linux")]
fn gtk_panel_size(gtk_window: &gtk::ApplicationWindow) -> Option<PanelSize> {
    use gtk::prelude::*;

    // GTK3 allocations are already in application pixels, which match CSS
    // pixels in the webview. Dividing by scale_factor again would shrink the
    // panel to half size on a 2× display.
    let alloc = gtk_window.allocation();
    PanelSize::from_logical(alloc.width() as f64, alloc.height() as f64)
}

#[cfg(target_os = "linux")]
fn place(
    fixed: &gtk::Fixed,
    webview: &gtk::Widget,
    gtk_window: &gtk::ApplicationWindow,
    stage: (i32, i32),
) {
    use gtk::prelude::*;

    let alloc = gtk_window.allocation();
    let x = alloc.width() - stage.0;
    let y = alloc.height() - stage.1;
    webview.set_size_request(stage.0, stage.1);
    fixed.move_(webview, x, y);
}

#[cfg(target_os = "linux")]
fn ensure_fixed(webview: &gtk::Widget) -> Result<gtk::Fixed, String> {
    use gtk::glib::Cast;
    use gtk::prelude::*;

    let Some(parent) = webview.parent() else {
        return Err(String::from("the webview has no parent to pin inside"));
    };
    if let Ok(fixed) = parent.clone().downcast::<gtk::Fixed>() {
        return Ok(fixed);
    }

    // Two widgets above the webview, the upper one a GtkWindow: that is the
    // shape Tauri's click-to-resize handler unwraps. Replacing the vbox —
    // not packing Fixed inside it — is what keeps the shape.
    let Some(window_widget) = parent.parent() else {
        return Err(String::from("the webview's parent has no window above it"));
    };
    let Ok(window) = window_widget.downcast::<gtk::Container>() else {
        return Err(String::from(
            "the webview's grandparent is not a container",
        ));
    };

    if let Ok(old) = parent.clone().downcast::<gtk::Container>() {
        old.remove(webview);
    }
    window.remove(&parent);

    let fixed = gtk::Fixed::new();
    window.add(&fixed);
    fixed.put(webview, 0, 0);
    fixed.show_all();
    Ok(fixed)
}

#[cfg(target_os = "linux")]
pub(crate) fn webview_in(root: &gtk::Widget) -> Option<gtk::Widget> {
    use gtk::glib::Cast;
    use gtk::prelude::*;

    if root.type_().name() == "WebKitWebView" {
        return Some(root.clone());
    }
    let container = root.clone().downcast::<gtk::Container>().ok()?;
    for child in container.children() {
        if let Some(found) = webview_in(&child) {
            return Some(found);
        }
    }
    None
}

#[cfg(target_os = "linux")]
fn current_monitor(
    window: &tauri::WebviewWindow,
) -> Result<Option<tauri::Monitor>, String> {
    let current = window.current_monitor().map_err(|e| e.to_string())?;
    match current {
        Some(monitor) => Ok(Some(monitor)),
        None => window.primary_monitor().map_err(|e| e.to_string()),
    }
}

/// Elsewhere the webview simply fills its window and is repainted by a compositor
/// that does not hand the page's last frame to a corner, so there is no viewport
/// to hold still. Size still has to be reported, because the page draws the
/// panel from `--nessa-window-*` rather than from its own viewport.
#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn fit(window: &tauri::WebviewWindow) -> Result<(), String> {
    use tauri::Emitter;
    let _ = window.emit(host::PANEL_SIZED, host::logical_inner_size(window)?);
    Ok(())
}

#[cfg(not(any(target_os = "macos", target_os = "linux")))]
pub fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
    use tauri::Emitter;
    let target = window.clone();
    window.on_window_event(move |event| {
        if let tauri::WindowEvent::Resized(_) = event {
            if let Ok(size) = host::logical_inner_size(&target) {
                let _ = target.emit(host::PANEL_SIZED, size);
            }
        }
    });
    Ok(())
}
