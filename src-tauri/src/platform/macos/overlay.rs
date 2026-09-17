//! Putting the setup window over the whole screen.
//!
//! Setup dims what is behind it, and a dim that stops at the menu bar is not a
//! dim — it is a grey rectangle on a bright desktop. A maximized window is
//! fitted to the *visible* frame, which is the screen minus the menu bar and
//! the Dock, so covering everything means setting the frame from the screen
//! itself and raising the window above the bar that would otherwise draw over
//! it.

use objc2_app_kit::{NSApplication, NSMainMenuWindowLevel, NSWindow, NSWindowCollectionBehavior};
use objc2_foundation::MainThreadMarker;
use tauri::WebviewWindow;

/// Cover the screen this window is on, above the menu bar, and let clicks
/// through it.
///
/// This is the dim: a sheet that takes the desktop away for the opening and
/// then goes. A maximized window would be fitted to the *visible* frame — the
/// screen minus the menu bar and the Dock — which is precisely what it has to
/// cover, so the frame comes from the screen itself.
///
/// The level goes first, and that ordering is the trick: AppKit constrains an
/// ordinary window's frame to the visible area, so a frame set while the
/// window is still at an ordinary level is clipped back to the shape this is
/// trying to escape.
///
/// It is also made to ignore the mouse entirely. It is scenery, and a person
/// who clicks past the setup window should reach whatever is actually behind
/// it rather than a pane of glass they cannot see.
pub fn present_dim(window: &WebviewWindow) {
    let handle = match window.ns_window() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("[nessa] could not place the setup dim: {error}");
            return;
        }
    };
    let native = unsafe { &*handle.cast::<NSWindow>() };
    // One above the menu bar, so the setup window at two above sits over it.
    native.setLevel(NSMainMenuWindowLevel + 1);
    if let Some(screen) = native.screen() {
        native.setFrame_display(screen.frame(), true);
    }
    native.setCollectionBehavior(
        native.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    native.setIgnoresMouseEvents(true);
}

/// Put the setup window over the dim, and give it the keyboard.
///
/// Nessa is an accessory app, so it is not active until something makes it
/// active, and an inactive app spends the first click on becoming active
/// rather than on what was clicked — which is why every button needed pressing
/// twice. Activating the app and making this window key is the same act as
/// putting it on screen, so it belongs in the same place.
pub fn present_setup(window: &WebviewWindow) {
    let handle = match window.ns_window() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("[nessa] could not focus setup: {error}");
            return;
        }
    };
    let native = unsafe { &*handle.cast::<NSWindow>() };
    // Above the dim, which is one above the menu bar.
    native.setLevel(NSMainMenuWindowLevel + 2);
    native.setCollectionBehavior(
        native.collectionBehavior() | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );
    if let Some(marker) = MainThreadMarker::new() {
        NSApplication::sharedApplication(marker).activate();
    }
    native.makeKeyAndOrderFront(None);
}

/// Lift the panel over the setup overlay, or put it back among ordinary
/// floating windows.
///
/// Setup covers the menu bar, which puts it above everything the panel
/// normally floats over — so the shortcut lesson summoned Nessa *underneath*
/// the window teaching the shortcut, which looks like the shortcut not
/// working. While setup is on screen the panel goes one level higher still,
/// which is also the truthful picture: Nessa arrives over whatever is in front
/// of you, and setup is no exception.
pub fn set_above_overlay(window: &WebviewWindow, above: bool) {
    let handle = match window.ns_window() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("[nessa] could not restack the panel: {error}");
            return;
        }
    };
    let native = unsafe { &*handle.cast::<NSWindow>() };
    native.setLevel(if above {
        // Over setup, which is two above the menu bar.
        NSMainMenuWindowLevel + 3
    } else {
        // `NSFloatingWindowLevel`, which is where `always_on_top` leaves it:
        // over ordinary windows and under the menu bar. It is written out
        // because objc2 does not export the named constant.
        3
    });
}
