//! The desktop window's traffic lights stay on the titlebar row.
//!
//! The page owns the row: `--desktop-titlebar-height` in
//! `src/desktop/styles.css`. The build script reads it from there for the host
//! (`titlebar_geometry.rs`), so the host keeps no copy. Everywhere, this checks
//! that reader against the stylesheet and that `tauri.conf.json` names no
//! `trafficLightPosition` to move the buttons a second way. On macOS it then
//! opens a real window, the way `tauri.conf.json` styles the desktop one, and
//! measures the buttons after placing them and after each thing that makes
//! AppKit lay the titlebar out again. The browser half, the page's controls on
//! the same 24px line, is `verification/desktop/scripts/columns.mjs --only
//! titlebar-row`.
//!
//! Its own `main` (`harness = false` in Cargo.toml): AppKit windows are made
//! and moved on the process's main thread only.

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/platform/macos/traffic_lights.rs"]
mod traffic_lights;

#[path = "../titlebar_geometry.rs"]
mod titlebar_geometry;

include!(concat!(env!("OUT_DIR"), "/titlebar.rs"));

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    the_host_reads_the_page(root);
    nothing_else_places_the_buttons(root);
    #[cfg(target_os = "macos")]
    on_a_real_window::run();
    println!("traffic_lights: ok");
}

fn the_host_reads_the_page(root: &Path) {
    let css = std::fs::read_to_string(root.join(titlebar_geometry::STYLES)).unwrap();
    assert_eq!(
        titlebar_geometry::titlebar_height(&css),
        Ok(TITLEBAR_HEIGHT),
        "the host was built with a titlebar {TITLEBAR_HEIGHT}pt tall"
    );
    // The page's row, as `columns.mjs` measures it.
    assert_eq!(TITLEBAR_HEIGHT, 48.0);
    // The same stylesheet with the height built another way is refused, not
    // read as the old formula.
    let changed = css.replace(
        "2 * var(--desktop-control-edge) + var(--desktop-control-size)",
        "var(--desktop-control-edge) + var(--desktop-control-size)",
    );
    assert!(titlebar_geometry::titlebar_height(&changed).is_err());
    // Nor is the old formula read from a comment beside a new live one.
    let commented = changed.replace(
        "--desktop-titlebar-height: calc(",
        "/* --desktop-titlebar-height: calc(\n    2 * var(--desktop-control-edge) + var(--desktop-control-size)\n  ); */\n  --desktop-titlebar-height: calc(",
    );
    assert!(titlebar_geometry::titlebar_height(&commented).is_err());
}

fn nothing_else_places_the_buttons(root: &Path) {
    let config = std::fs::read_to_string(root.join("tauri.conf.json")).unwrap();
    assert!(
        !config.contains("trafficLightPosition"),
        "tauri.conf.json sets trafficLightPosition: traffic_lights.rs is the one owner of the buttons' place"
    );
}

#[cfg(target_os = "macos")]
mod on_a_real_window {
    use objc2::rc::Retained;
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, MainThreadOnly};
    use objc2_app_kit::{NSApplication, NSWindow};
    use objc2_foundation::{MainThreadMarker, NSPoint, NSRect, NSSize, NSString};

    use super::traffic_lights::{buttons, lead, watch, LEFT, ROW_CENTRE};

    // NSWindowStyleMask, as `"titleBarStyle": "Overlay"` builds the window.
    const TITLED: usize = 1;
    const CLOSABLE: usize = 1 << 1;
    const MINIATURIZABLE: usize = 1 << 2;
    const RESIZABLE: usize = 1 << 3;
    const FULL_SIZE_CONTENT_VIEW: usize = 1 << 15;
    const BUFFERED: usize = 2;
    const HIDDEN_TITLE: isize = 1;

    pub fn run() {
        let main = MainThreadMarker::new().expect("the test runs on the main thread");
        let _app = NSApplication::sharedApplication(main);
        let window: Retained<NSWindow> = unsafe {
            msg_send![
                NSWindow::alloc(main),
                initWithContentRect: NSRect::new(NSPoint::new(200.0, 200.0), NSSize::new(1100.0, 700.0)),
                styleMask: TITLED | CLOSABLE | MINIATURIZABLE | RESIZABLE | FULL_SIZE_CONTENT_VIEW,
                backing: BUFFERED,
                defer: false
            ]
        };
        unsafe {
            let _: () = msg_send![&window, setReleasedWhenClosed: false];
            let _: () = msg_send![&window, setTitlebarAppearsTransparent: true];
            let _: () = msg_send![&window, setTitleVisibility: HIDDEN_TITLE];
        }
        window.orderFrontRegardless();
        spin();
        let before = describe(&window);

        watch(window.clone(), main).expect("the buttons are placed");
        expect_on_row(&window, "placed", &before);

        let larger = NSRect::new(NSPoint::new(120.0, 120.0), NSSize::new(1300.0, 820.0));
        window.setFrame_display(larger, true);
        spin();
        expect_on_row(&window, "after a resize", &before);

        unsafe {
            let _: () = msg_send![&window, setTitle: &*NSString::from_str("Nessa")];
        }
        spin();
        expect_on_row(&window, "after the title changed", &before);
        unsafe {
            let _: () = msg_send![&window, setTitleVisibility: 0isize];
            let _: () = msg_send![&window, setTitleVisibility: HIDDEN_TITLE];
        }
        spin();
        expect_on_row(&window, "after the title showed and hid", &before);

        for name in ["NSAppearanceNameAqua", "NSAppearanceNameDarkAqua"] {
            unsafe {
                let appearance: Option<Retained<AnyObject>> = msg_send![
                    class!(NSAppearance),
                    appearanceNamed: &*NSString::from_str(name)
                ];
                let _: () = msg_send![&window, setAppearance: appearance.as_deref()];
            }
            spin();
            expect_on_row(&window, &format!("after {name}"), &before);
        }

        unsafe {
            let frame: Option<Retained<AnyObject>> = msg_send![&window, contentView];
            if let Some(content) = frame {
                let theme: Option<Retained<AnyObject>> = msg_send![&content, superview];
                if let Some(theme) = theme {
                    let _: () = msg_send![&theme, setNeedsLayout: true];
                    let _: () = msg_send![&theme, layoutSubtreeIfNeeded];
                }
            }
        }
        spin();
        expect_on_row(&window, "after the frame view laid out again", &before);

        // The side rail opens: the page's first column starts 48pt in, and the
        // buttons follow it into that column's corner and stay there.
        lead(&window, 48.0, main).expect("the buttons follow the column");
        spin();
        expect_at(&window, "led by the rail", &before, LEFT + 48.0);
        window.setFrame_display(
            NSRect::new(NSPoint::new(160.0, 160.0), NSSize::new(1200.0, 760.0)),
            true,
        );
        spin();
        expect_at(
            &window,
            "led by the rail, after a resize",
            &before,
            LEFT + 48.0,
        );
        assert!(
            lead(&window, f64::NAN, main).is_err(),
            "a lead that is not a distance is refused"
        );
        assert!(
            lead(&window, -1.0, main).is_err(),
            "a negative lead is refused"
        );
        spin();
        expect_at(&window, "after a refused lead", &before, LEFT + 48.0);
        lead(&window, 0.0, main).expect("the buttons return to the window's edge");
        spin();
        expect_on_row(&window, "the rail closed again", &before);

        window.close();
    }

    /// Lets AppKit post and lay out, and the main queue run what it was handed.
    fn spin() {
        unsafe {
            let date: Retained<AnyObject> =
                msg_send![class!(NSDate), dateWithTimeIntervalSinceNow: 0.25f64];
            let run_loop: Retained<AnyObject> = msg_send![class!(NSRunLoop), currentRunLoop];
            let _: () = msg_send![&run_loop, runUntilDate: &*date];
        }
    }

    /// Each button's centre below the window's top, and its left edge.
    fn measure(window: &NSWindow) -> Vec<(f64, f64)> {
        let height = window.frame().size.height;
        buttons(window)
            .iter()
            .map(|button| {
                let rect = button.convertRect_toView(button.bounds(), None);
                (
                    height - (rect.origin.y + rect.size.height / 2.0),
                    rect.origin.x,
                )
            })
            .collect()
    }

    fn describe(window: &NSWindow) -> String {
        format!("{:?}", measure(window))
    }

    fn expect_on_row(window: &NSWindow, when: &str, before: &str) {
        expect_at(window, when, before, LEFT)
    }

    /// On the row, with the close button's left edge `left_edge` points in.
    fn expect_at(window: &NSWindow, when: &str, before: &str, left_edge: f64) {
        let found = measure(window);
        assert_eq!(
            found.len(),
            3,
            "{when}: the window has {} buttons",
            found.len()
        );
        for (index, (centre, left)) in found.iter().enumerate() {
            assert!(
                (centre - ROW_CENTRE).abs() <= 0.5,
                "{when}: button {index} is centred {centre:.1}pt below the top, not {ROW_CENTRE} \
                 (AppKit's own placement was {before}; now {found:?})"
            );
            if index == 0 {
                assert!(
                    (left - left_edge).abs() <= 0.5,
                    "{when}: the close button starts at {left:.1}pt, not {left_edge}"
                );
            }
        }
        println!("traffic_lights: {when}: {found:?}");
    }
}
