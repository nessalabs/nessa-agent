//! The no-navigation WKWebView case from #916. Run with:
//! `cargo test -p nessa-app --no-default-features --test page_load`.
//! The state machine tests run on every host; macOS also creates an empty
//! WKWebView on the main thread and establishes that its native URL is nil.
//! This is not a full native app/bootstrap or normal-server acceptance test.

#[cfg(target_os = "macos")]
#[path = "../src/page_load/state.rs"]
mod state;

fn main() {
    #[cfg(target_os = "macos")]
    missing_native_url();
    #[cfg(not(target_os = "macos"))]
    println!("page_load: native nil-URL qualification requires macOS (not run)");
}

#[cfg(target_os = "macos")]
fn missing_native_url() {
    use objc2::rc::{Allocated, Retained};
    use objc2::runtime::AnyObject;
    use objc2::{class, msg_send, MainThreadOnly};
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSRect, NSURL};

    let main = MainThreadMarker::new().expect("AppKit runs on the main thread");
    let _app = NSApplication::sharedApplication(main);
    let view: Retained<AnyObject> = unsafe {
        let configuration: Retained<AnyObject> = msg_send![class!(WKWebViewConfiguration), new];
        let allocated: Allocated<AnyObject> = msg_send![class!(WKWebView), alloc];
        msg_send![allocated, initWithFrame: NSRect::ZERO, configuration: &*configuration]
    };
    // Unlike Wry 0.55.1's accessor, this query represents nil explicitly.
    let url: Option<Retained<NSURL>> = unsafe { msg_send![&*view, URL] };
    assert!(url.is_none(), "the regression needs a webview with no URL");

    let loads = state::Loads::new();
    let configured = "http://localhost:1498/desktop.html";
    assert_eq!(
        loads.unserved_page("desktop", configured).as_deref(),
        Some(configured)
    );
    loads.observe(
        "desktop",
        "nessa-status://localhost/load-failure?page=original",
    );
    assert_eq!(loads.unserved_page("desktop", configured), None);
    loads.observe("main", "http://localhost:1420/index.html");
    assert_eq!(loads.unserved_page("main", configured), None);
    println!("page_load: native URL=nil; configured diagnostic retained; failure-page and normal-origin fact controls passed");
}
