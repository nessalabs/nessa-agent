//! Live-resize on Linux: size-allocate while a button is held.
//!
//! The compositor takes the resize grab the same way macOS does, so pointerup
//! on the handle is not a reliable end. Size-allocate bursts while a mouse
//! button is down are the drag; a short quiet after the last one is the
//! release. Programmatic refits (show, display change) have no button down, so
//! they do not pin the glow.
//!
//! The page draws a 12px west handle, but `startResizeDragging` is an async
//! invoke: it arrives after the button-press, and GTK's `_NET_WM_MOVERESIZE`
//! has to run from the press itself. Tauri already does that for a 5px inset
//! on the webview. `arm_west_handle` covers the rest of the handle, using the
//! window's left edge rather than the webview's — the view is larger than the
//! window and pinned to its bottom right.

use gtk::glib;
use gtk::prelude::*;
use std::cell::{Cell, RefCell};
use std::rc::Rc;
use std::time::Duration;
use tauri::Emitter;

use crate::host;

use super::viewport::webview_in;

pub fn watch(window: &tauri::WebviewWindow) -> Result<(), String> {
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

fn arm_west_handle(gtk_window: &gtk::ApplicationWindow) {
    use gtk::gdk::WindowEdge;
    use gtk::glib::{Cast, Propagation};

    const HANDLE: f64 = 12.0;
    let Some(webview) = webview_in(gtk_window.upcast_ref()) else {
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

fn pointer_button_down(window: &gtk::ApplicationWindow) -> bool {
    use gtk::gdk;
    use gtk::gdk::prelude::*;

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
