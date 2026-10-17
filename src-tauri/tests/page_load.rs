//! The no-navigation WKWebView case from #916. Run with:
//! `cargo test -p nessa-app --no-default-features --test page_load`.
//! On macOS this creates an empty WKWebView on the main thread, establishes
//! that its native URL is nil, and checks the diagnostic state controls. Other
//! hosts report unsupported; separate portable and production unit tests cover
//! the state machine.
//! This is not a full native app/bootstrap or normal-server acceptance test.

#[cfg(target_os = "macos")]
#[path = "../src/page_load/state.rs"]
mod state;

fn main() {
    #[cfg(target_os = "macos")]
    native::run();
    #[cfg(not(target_os = "macos"))]
    println!("page_load: native nil-URL qualification requires macOS (not run)");
}

#[cfg(target_os = "macos")]
mod native {
    use objc2::MainThreadOnly;
    use objc2_app_kit::NSApplication;
    use objc2_foundation::{MainThreadMarker, NSRect};
    use objc2_web_kit::{WKWebView, WKWebViewConfiguration};

    use super::state::Loads;

    pub fn run() {
        let main = MainThreadMarker::new().expect("AppKit runs on the main thread");
        let _app = NSApplication::sharedApplication(main);
        // The typed WebKit bindings link the framework in this standalone test;
        // AppKit alone does not register WKWebView's Objective-C classes.
        let view = unsafe {
            let configuration = WKWebViewConfiguration::new(main);
            WKWebView::initWithFrame_configuration(
                WKWebView::alloc(main),
                NSRect::ZERO,
                &configuration,
            )
        };
        // Unlike Wry 0.55.1's accessor, this query represents nil explicitly.
        let url = unsafe { view.URL() };
        assert!(url.is_none(), "the regression needs a webview with no URL");

        let loads = Loads::new();
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
}
