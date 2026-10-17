//! Compile the production facts with cfg(test) but without a libtest harness,
//! matching the macOS main-thread integration binary. No native API is used.
extern crate self as tauri;
pub use url::Url;

#[path = "page_load/state.rs"]
mod state;

use state::Loads;

fn main() {
    let loads = Loads::new();
    assert_eq!(
        loads.unserved_page("desktop", "configured").as_deref(),
        Some("configured")
    );
    loads.observe("desktop", "http://localhost:1420/desktop.html");
    assert_eq!(loads.unserved_page("desktop", "configured"), None);
    println!("page_load_state: standalone harness passed");
}
