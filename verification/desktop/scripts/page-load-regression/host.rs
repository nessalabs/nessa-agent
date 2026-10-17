#![allow(dead_code)]
extern crate self as tauri;
pub use http;
use std::sync::{Arc, Mutex};
pub use url::Url;
#[derive(Clone)]
pub struct AppHandle {
    config: Arc<Config>,
    windows: Arc<Vec<Window>>,
}
pub struct Config {
    pub build: Build,
}
pub struct Build {
    pub dev_url: Option<Url>,
}
impl AppHandle {
    pub fn config(&self) -> &Config {
        &self.config
    }
    pub fn run_on_main_thread(&self, run: impl FnOnce()) -> Result<(), String> {
        run();
        Ok(())
    }
}
pub trait Manager {
    fn get_webview_window(&self, label: &str) -> Option<Window>;
}
impl Manager for AppHandle {
    fn get_webview_window(&self, label: &str) -> Option<Window> {
        self.windows
            .iter()
            .find(|window| window.label == label)
            .cloned()
    }
}
#[derive(Clone)]
pub struct Window {
    label: &'static str,
    url: Option<Url>,
    navigated: Arc<Mutex<Vec<Url>>>,
    refuse: bool,
}
impl Window {
    // Synthetic Wry 0.55.1 failure: panic before producing a Result.
    pub fn url(&self) -> Result<Url, String> {
        Ok(self
            .url
            .as_ref()
            .expect("synthetic WKWebView.URL is nil")
            .clone())
    }
    pub fn navigate(&self, url: Url) -> Result<(), String> {
        if self.refuse {
            return Err("synthetic navigation failure".into());
        }
        self.navigated.lock().unwrap().push(url);
        Ok(())
    }
}
mod panel {
    pub const MAIN_WINDOW: &str = "main";
    pub const SETUP_WINDOW: &str = "setup";
}
mod desktop_window {
    pub const DESKTOP_WINDOW: &str = "desktop";
}
mod host_log {
    pub fn warn(_: String) {}
}
mod host_refusal {
    fn copies() -> serde_json::Value {
        serde_json::from_str(include_str!("../../src/host/startup-refusals.json")).unwrap()
    }
    pub fn fill(key: &str, pairs: &[(&str, &str)]) -> String {
        let mut sentence = copies()[key].as_str().unwrap().to_owned();
        for (key, value) in pairs {
            sentence = sentence.replace(&format!("{{{key}}}"), value);
        }
        sentence
    }
    pub fn line() -> String {
        copies()["line"].as_str().unwrap().into()
    }
    pub fn code(key: &str) -> String {
        copies()["code"][key].as_str().unwrap().into()
    }
}
mod page_load;

// The real links adapter consumes this public function; this fixture compiles
// only page_load, so keep that contract used without substituting its policy.
const _: fn(&Url, bool) -> bool = page_load::is_load_failure_page;
