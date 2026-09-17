//! Putting the setup window over the whole screen.
//!
//! Setup dims what is behind it, and a dim that stops at the menu bar is not a
//! dim — it is a grey rectangle on a bright desktop. A maximized window is
//! fitted to the *visible* frame, which is the screen minus the menu bar and
//! the Dock, so covering everything means setting the frame from the screen
//! itself and raising the window above the bar that would otherwise draw over
//! it.

use objc2_app_kit::{
    NSApplication, NSMainMenuWindowLevel, NSWindow, NSWindowButton, NSWindowCollectionBehavior,
    NSWindowStyleMask, NSWindowTitleVisibility,
};
use objc2_foundation::MainThreadMarker;
use objc2_foundation::NSPoint;
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
    install_controls(native);
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

/*
 * The window's own close, minimise and zoom buttons, with none of the window
 * chrome they normally come attached to.
 *
 * They are the real controls rather than three circles that look like them: a
 * drawn one has to reimplement every state the system already draws — hover,
 * press, window-inactive, the symbols that appear when the pointer is over the
 * group — and it will be wrong the moment any of that changes. Making the
 * titlebar transparent and its title invisible, over a content view that runs
 * the full height of the window, leaves the buttons and nothing else.
 */
fn install_controls(native: &NSWindow) {
    native.setStyleMask(
        native.styleMask()
            | NSWindowStyleMask::Titled
            | NSWindowStyleMask::Closable
            | NSWindowStyleMask::Miniaturizable
            | NSWindowStyleMask::FullSizeContentView,
    );
    native.setTitlebarAppearsTransparent(true);
    native.setTitleVisibility(NSWindowTitleVisibility::Hidden);
}

/**
 * Put the window controls at a point in the window, rather than in its corner.
 *
 * This window is the whole screen, so the corner AppKit would put them in is
 * the corner of the *display* — stranded in the dimmed area, nowhere near the
 * surface they close. The page knows where it drew that surface and says so.
 *
 * They are moved into the content view to get there: a standard button lives
 * in the title bar's own view, which is a strip too short to hold one several
 * hundred points further down.
 */
pub fn place_controls(window: &WebviewWindow, left: f64, top: f64) {
    let handle = match window.ns_window() {
        Ok(handle) => handle,
        Err(error) => {
            eprintln!("[nessa] could not place the window controls: {error}");
            return;
        }
    };
    let native = unsafe { &*handle.cast::<NSWindow>() };
    let Some(content) = native.contentView() else {
        return;
    };
    let height = content.frame().size.height;
    let mut x = left;
    for button in [
        NSWindowButton::CloseButton,
        NSWindowButton::MiniaturizeButton,
        NSWindowButton::ZoomButton,
    ] {
        let Some(view) = native.standardWindowButton(button) else {
            continue;
        };
        let size = view.frame().size;
        view.removeFromSuperview();
        content.addSubview(&view);
        // AppKit measures from the bottom left; the page measures from the top.
        view.setFrameOrigin(NSPoint::new(x, height - top - size.height));
        // The spacing the system uses between the three.
        x += 20.0;
    }
}
