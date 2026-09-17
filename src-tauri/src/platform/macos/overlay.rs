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

/// Cover the screen the window is on, above the menu bar, and take focus.
///
/// Focus is part of the same job rather than an extra: Nessa is an accessory
/// app, so it is not active until something makes it active, and an inactive
/// app's window spends the first click on being activated. Setup is a window
/// full of buttons, and every one of them would have needed pressing twice.
pub fn present(window: &WebviewWindow) {
    let handle = match window.ns_window() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("[nessa] could not place setup over the screen: {error}");
            return;
        }
    };
    // Runs on the main thread during setup; the live WebviewWindow owns this
    // NSWindow, so it is only borrowed for this configuration.
    let native = unsafe { &*handle.cast::<NSWindow>() };

    // The level goes first, and that ordering is the whole trick: AppKit
    // constrains an ordinary window's frame to the screen's *visible* area —
    // the screen minus the menu bar and the Dock — so a frame set while the
    // window is still at an ordinary level is clipped back to exactly the
    // shape this is trying to escape. Above the menu bar there is nothing to
    // constrain it to.
    //
    // One level above the bar: enough to cover it, and not so high that setup
    // sits over a screen saver or a security prompt.
    native.setLevel(NSMainMenuWindowLevel + 1);
    if let Some(screen) = native.screen() {
        native.setFrame_display(screen.frame(), true);
    }
    native.setCollectionBehavior(
        native.collectionBehavior()
            | NSWindowCollectionBehavior::CanJoinAllSpaces
            | NSWindowCollectionBehavior::FullScreenAuxiliary,
    );

    if let Some(marker) = MainThreadMarker::new() {
        // Without this the first click anywhere in setup is spent activating
        // an accessory app rather than pressing what was clicked.
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
        NSMainMenuWindowLevel + 2
    } else {
        // `NSFloatingWindowLevel`, which is where `always_on_top` leaves it:
        // over ordinary windows and under the menu bar. It is written out
        // because objc2 does not export the named constant.
        3
    });
}
