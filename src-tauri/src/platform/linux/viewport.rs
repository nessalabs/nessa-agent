//! The page's viewport, held still while the window resizes around it.
//!
//! WebKitGTK hangs its last committed frame off the widget's top-left the
//! same way WKWebView does, so the same pin applies: the webview is sized to
//! the work area, placed in a `GtkFixed` at the window's bottom right, and
//! left there. The window grows and shrinks over it. A `GtkBox` child would
//! be stretched with the window and the page's viewport would move — which is
//! the jitter this module exists to prevent.
//!
//! The Fixed *replaces* tao's default vbox as the window's child. Nesting it
//! inside the box would put three widgets between the webview and the
//! GtkWindow, and Tauri's undecorated resize handler does
//! `webview.parent().parent().downcast::<gtk::Window>().unwrap()` on every
//! click. A GtkBox there panics; GTK callbacks cannot unwind, so the process
//! aborts.

use crate::host::{self, PanelSize};
use crate::platform::current_monitor;

thread_local! {
    static FITTING: std::cell::Cell<bool> = const { std::cell::Cell::new(false) };
}

pub fn panel_size(window: &tauri::WebviewWindow) -> Result<PanelSize, String> {
    let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
    gtk_panel_size(&gtk_window).ok_or_else(|| String::from("the window has no size"))
}

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

/// Drop the previous frame so a bubble that slid up does not leave a ghost.
/// WebKitGTK skips paint on transparent frost; queueing a draw with the
/// clear colour put back is what empties those pixels.
pub fn repaint(window: &tauri::WebviewWindow) -> Result<(), String> {
    use gtk::gdk;
    use gtk::glib::Cast;
    use gtk::prelude::*;
    use webkit2gtk::WebViewExt;

    let gtk_window = window.gtk_window().map_err(|error| error.to_string())?;
    let Some(webview) = webview_in(gtk_window.upcast_ref()) else {
        return Err(String::from("no webview under the window"));
    };
    let clear = gdk::RGBA::new(0.0, 0.0, 0.0, 0.0);
    if let Ok(wk) = webview.clone().downcast::<webkit2gtk::WebView>() {
        wk.set_background_color(&clear);
    }
    webview.queue_draw();
    gtk_window.queue_draw();
    // An opacity nudge invalidates the offscreen without a visible hide.
    let opacity = webview.opacity();
    webview.set_opacity(0.999);
    webview.queue_draw();
    webview.set_opacity(if opacity == 0.0 { 1.0 } else { opacity });
    Ok(())
}

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

fn gtk_panel_size(gtk_window: &gtk::ApplicationWindow) -> Option<PanelSize> {
    use gtk::prelude::*;

    // GTK3 allocations are already in application pixels, which match CSS
    // pixels in the webview. Dividing by scale_factor again would shrink the
    // panel to half size on a 2× display.
    let alloc = gtk_window.allocation();
    PanelSize::from_logical(alloc.width() as f64, alloc.height() as f64)
}

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
    keep_transparent(gtk_window.upcast_ref(), fixed, webview);
}

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
    keep_transparent(&window, &fixed, webview);
    window.add(&fixed);
    fixed.put(webview, 0, 0);
    keep_transparent(&window, &fixed, webview);
    watch_background(webview);
    fixed.show_all();
    Ok(fixed)
}

/// WebKitGTK resets the webview's background to opaque when the page
/// promotes a new compositing layer — focusing the composer, streaming a
/// bubble. The CSS frost then has a white backing and nothing to sample.
/// Putting the clear colour back on every draw is what keeps the window
/// transparent after the first paint.
fn watch_background(webview: &gtk::Widget) {
    use gtk::gdk;
    use gtk::glib::{Cast, Propagation};
    use gtk::prelude::*;
    use std::cell::Cell;
    use webkit2gtk::WebViewExt;

    thread_local! {
        static HOOKED: Cell<bool> = const { Cell::new(false) };
    }
    if HOOKED.with(Cell::get) {
        return;
    }
    HOOKED.with(|flag| flag.set(true));

    let clear = gdk::RGBA::new(0.0, 0.0, 0.0, 0.0);
    webview.connect_draw(move |webview, _| {
        if let Ok(wk) = webview.clone().downcast::<webkit2gtk::WebView>() {
            wk.set_background_color(&clear);
        }
        Propagation::Proceed
    });
}

/// Reparenting into a new GtkFixed drops the rgba visual and the webview's
/// transparent background that wry set on the original vbox. Without them
/// the panel paints opaque white and the Linux CSS frost has nothing to
/// sample. Re-apply both on the window, the Fixed, and the webview.
fn keep_transparent(window: &gtk::Container, fixed: &gtk::Fixed, webview: &gtk::Widget) {
    use gtk::gdk;
    use gtk::glib::Cast;
    use gtk::prelude::*;
    use webkit2gtk::WebViewExt;

    let clear = gdk::RGBA::new(0.0, 0.0, 0.0, 0.0);
    for widget in [
        window.upcast_ref::<gtk::Widget>(),
        fixed.upcast_ref::<gtk::Widget>(),
        webview,
    ] {
        widget.set_app_paintable(true);
        if let Some(screen) = widget.screen() {
            if let Some(visual) = screen.rgba_visual() {
                widget.set_visual(Some(&visual));
            }
        }
    }

    if let Ok(wk) = webview.clone().downcast::<webkit2gtk::WebView>() {
        wk.set_background_color(&clear);
    }
}

pub(super) fn webview_in(root: &gtk::Widget) -> Option<gtk::Widget> {
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
