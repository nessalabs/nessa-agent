//! The desktop window's traffic lights stay on the titlebar row.
//!
//! The row's centre is written twice: by the page, as `--desktop-titlebar-height`
//! in `src/desktop/styles.css`, and by the host, as `ROW_CENTRE` in
//! `src/platform/macos/traffic_lights.rs`. Everywhere, this checks the two
//! agree and that `tauri.conf.json` names no `trafficLightPosition` to move the
//! buttons a second way. On macOS it then opens a real window, the way
//! `tauri.conf.json` styles the desktop one, and measures the buttons after
//! placing them and after each thing that makes AppKit lay the titlebar out
//! again. The browser half, the page's controls on the same 24px line, is
//! `verification/desktop/scripts/columns.mjs --only titlebar-row`.
//!
//! Its own `main` (`harness = false` in Cargo.toml): AppKit windows are made
//! and moved on the process's main thread only.

#[cfg(target_os = "macos")]
#[allow(dead_code)]
#[path = "../src/platform/macos/traffic_lights.rs"]
mod traffic_lights;

use std::path::Path;

fn main() {
    let root = Path::new(env!("CARGO_MANIFEST_DIR"));
    page_and_host_agree(root);
    nothing_else_places_the_buttons(root);
    #[cfg(target_os = "macos")]
    on_a_real_window::run();
    println!("traffic_lights: ok");
}

/// A `--name: 8px;` declaration's number, from the first place it is set.
fn css_px(css: &str, name: &str) -> f64 {
    let start = css
        .find(&format!("{name}:"))
        .unwrap_or_else(|| panic!("styles.css no longer sets {name}"));
    let value = &css[start + name.len() + 1..];
    let value = value[..value.find(';').expect("a declaration ends")].trim();
    value
        .strip_suffix("px")
        .and_then(|px| px.parse().ok())
        .unwrap_or_else(|| panic!("{name} is {value}, not a length in px"))
}

fn page_and_host_agree(root: &Path) {
    // A Windows checkout ends lines with CRLF.
    let css = std::fs::read_to_string(root.join("../src/desktop/styles.css"))
        .unwrap()
        .replace("\r\n", "\n");
    // `--desktop-titlebar-height: calc(2 * var(--desktop-control-edge) + var(--desktop-control-size))`,
    // with the edge the gutter plus the control's inset.
    assert!(
        css.contains("--desktop-titlebar-height: calc(\n    2 * var(--desktop-control-edge) + var(--desktop-control-size)\n  );"),
        "--desktop-titlebar-height is no longer two control edges and a control; update this test and ROW_CENTRE together"
    );
    let edge = css_px(&css, "--desktop-gutter") + css_px(&css, "--desktop-control-inset");
    let titlebar = 2.0 * edge + css_px(&css, "--desktop-control-size");

    let host = std::fs::read_to_string(root.join("src/platform/macos/traffic_lights.rs")).unwrap();
    let row_centre: f64 = host
        .split("pub const ROW_CENTRE: f64 = ")
        .nth(1)
        .and_then(|rest| rest.split(';').next())
        .and_then(|value| value.parse().ok())
        .expect("traffic_lights.rs declares ROW_CENTRE");
    assert_eq!(
        row_centre * 2.0,
        titlebar,
        "the host centres the traffic lights at {row_centre}pt, but the page's titlebar row is {titlebar}px tall"
    );
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

    use std::sync::atomic::Ordering;

    use super::traffic_lights::{buttons, watch, LAYOUTS, LEFT, ROW_CENTRE};

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

        // Placing the buttons must not make AppKit lay the titlebar out again:
        // left alone, the window settles.
        let settled = LAYOUTS.load(Ordering::Relaxed);
        spin();
        let since = LAYOUTS.load(Ordering::Relaxed) - settled;
        assert!(
            since <= 1,
            "the frame view laid out {since} times in a quarter second with nothing changing"
        );

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
                    (left - LEFT).abs() <= 0.5,
                    "{when}: the close button starts at {left:.1}pt, not {LEFT}"
                );
            }
        }
        println!("traffic_lights: {when}: {found:?}");
    }
}
